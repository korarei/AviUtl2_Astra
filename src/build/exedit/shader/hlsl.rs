use super::layout::{Buffer, Issue, apply_dims, resolve_layout};
use crate::build::lang::hlsl;
use crate::build::source::{self, Pos};
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashMap;

pub(super) fn validate_body(pos: &Pos, kind: &str, name: &str, body: &source::Map) -> anyhow::Result<()> {
    let text = body.text();
    let st = text.len() - text.trim_start().len();
    let src = text[st..].trim_end();
    let locate = |offset| body.locate(st + offset);
    let mut tokens = hlsl::Lexer::new(src)
        .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;
    let issues = validate_cbuffers(src, &tokens);
    tokens.retain(|token| !matches!(token.kind, hlsl::TokenKind::Directive(_)));
    let has_entry = has_entry(src, &tokens, name, kind)
        .map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;
    let issues = issues.map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;

    for (level, span, msg) in issues {
        let pos = locate(span.st);
        match level {
            tracing::Level::WARN => tracing::warn!("{pos}: {msg}"),
            tracing::Level::INFO => tracing::info!("{pos}: {msg}"),
            _ => tracing::debug!("{pos}: {msg}"),
        }
    }

    if !has_entry {
        bail!("{pos}: entry point '{name}' is not defined in '{kind}' block");
    }

    Ok(())
}

struct Packing {
    preprocessor: hlsl::Preprocessor,
    aliases: HashMap<String, Alias>,
    issues: Vec<Issue>,
}

impl Packing {
    fn process(&mut self, token: &hlsl::Token<'_>) -> Result<Option<hlsl::Action>, hlsl::Error> {
        let action = self.preprocessor.process(token)?;

        match action {
            Some(hlsl::Action::Unknown) => self.issues.push((
                tracing::Level::DEBUG,
                token.span,
                "cannot determine HLSL conditional expression".into(),
            )),
            Some(hlsl::Action::Include) => {
                self.aliases.clear();
                self.issues.push((
                    tracing::Level::DEBUG,
                    token.span,
                    "cannot track HLSL definitions and matrix packing through #include".into(),
                ));
            }
            _ => {}
        }

        Ok(action)
    }

    fn process_typedef(&mut self, src: &str, tokens: &[hlsl::Token<'_>], curr: usize) -> usize {
        let st = curr + 1;
        let mut ed = st;

        while ed < tokens.len() && tokens[ed].kind != hlsl::TokenKind::Semicolon {
            if matches!(
                tokens[ed].kind,
                hlsl::TokenKind::LBrace | hlsl::TokenKind::LParen | hlsl::TokenKind::LBracket
            ) {
                ed = hlsl::find_pair(tokens, ed).unwrap_or(ed);
            }

            ed += 1;
        }

        if st < ed {
            self.collect_aliases(src, &tokens[st..ed]);
        }

        ed + usize::from(ed < tokens.len())
    }

    fn process_cbuffer(
        &mut self,
        src: &str,
        tokens: &[hlsl::Token<'_>],
        mut curr: usize,
        layout: &mut Buffer,
    ) -> Result<Option<usize>, hlsl::Error> {
        let span = tokens[curr].span;
        let is_active = self.preprocessor.is_active();
        curr += 1;
        let buffer = tokens
            .get(curr)
            .map_or("<unnamed>", |token| &src[token.span.st..token.span.ed]);

        while curr < tokens.len() && !matches!(tokens[curr].kind, hlsl::TokenKind::LBrace | hlsl::TokenKind::Semicolon)
        {
            curr += 1;
        }

        if tokens
            .get(curr)
            .is_none_or(|token| token.kind != hlsl::TokenKind::LBrace)
        {
            return Ok(Some(curr + usize::from(curr < tokens.len())));
        }

        let Some(ed) = hlsl::find_pair(tokens, curr) else {
            self.issues.push((
                tracing::Level::DEBUG,
                span,
                format!("cannot analyze unclosed cbuffer '{buffer}'"),
            ));
            return Ok(None);
        };

        if is_active == Some(true) {
            validate_buffer(src, &tokens[curr + 1..ed], buffer, span, self, layout)?;
        } else {
            if is_active.is_none() {
                self.issues.push((
                    tracing::Level::DEBUG,
                    span,
                    format!("cannot determine active declarations in cbuffer '{buffer}'"),
                ));
            }

            for token in &tokens[curr + 1..ed] {
                let _ = self.process(token)?;
            }
        }

        Ok(Some(ed + 1))
    }

    fn collect_aliases(&mut self, src: &str, tokens: &[hlsl::Token<'_>]) {
        let _ = with_decl(&self.preprocessor, src, tokens, |text, tokens, decl| {
            let alias = resolve_alias(&self.aliases, decl.ty).map(Cow::into_owned);
            let is_row_major = find_row_major(&decl.modifiers);

            for member in &decl.members {
                let Some(alias) = &alias else {
                    self.aliases.remove(member.name);
                    continue;
                };

                let mut dims = parse_dims(&self.preprocessor, text, tokens, member);
                dims.extend_from_slice(&alias.dims);
                let (is_row_major, has_packing) = if is_row_major.is_some() {
                    (is_row_major, true)
                } else if !alias.has_packing && self.preprocessor.has_packing {
                    (self.preprocessor.is_row_major, true)
                } else {
                    (alias.is_row_major, alias.has_packing)
                };

                self.aliases.insert(
                    member.name.to_owned(),
                    Alias {
                        ty: alias.ty,
                        dims,
                        is_row_major,
                        has_packing,
                    },
                );
            }
        });
    }
}

#[derive(Clone)]
struct Alias {
    ty: hlsl::Type<'static>,
    dims: Vec<Option<usize>>,
    is_row_major: Option<bool>,
    has_packing: bool,
}

enum DeclError {
    Macro,
    Syntax,
}

fn validate_cbuffers(src: &str, tokens: &[hlsl::Token<'_>]) -> Result<Vec<Issue>, hlsl::Error> {
    let mut packing = Packing {
        preprocessor: hlsl::Preprocessor::new(),
        aliases: HashMap::new(),
        issues: Vec::new(),
    };
    let mut layout = Buffer::default();
    let mut curr = 0;
    let mut depth = 0;

    while curr < tokens.len() {
        if packing.process(&tokens[curr])?.is_some() {
            curr += 1;
            continue;
        }

        if tokens[curr].kind == hlsl::TokenKind::Keyword("typedef")
            && packing.preprocessor.is_active() == Some(true)
            && depth == 0
        {
            curr = packing.process_typedef(src, tokens, curr);
            continue;
        }

        if tokens[curr].kind == hlsl::TokenKind::Keyword("cbuffer") {
            let Some(next) = packing.process_cbuffer(src, tokens, curr, &mut layout)? else {
                break;
            };
            curr = next;
            continue;
        }

        match tokens[curr].kind {
            hlsl::TokenKind::LBrace => depth += 1,
            hlsl::TokenKind::RBrace if depth != 0 => depth -= 1,
            _ => {}
        }
        curr += 1;
    }

    packing.preprocessor.finish()?;
    Ok(packing.issues)
}

fn has_entry(src: &str, tokens: &[hlsl::Token<'_>], name: &str, kind: &str) -> Result<bool, hlsl::Error> {
    Ok(hlsl::Parser::new(src, tokens).parse()?.funcs.into_iter().any(|func| {
        func.name == name
            && match kind {
                "computeshader" => func.has_numthreads && func.is_void,
                _ => !func.has_numthreads,
            }
    }))
}

fn validate_buffer(
    src: &str,
    tokens: &[hlsl::Token<'_>],
    buffer: &str,
    span: hlsl::Span,
    packing: &mut Packing,
    layout: &mut Buffer,
) -> Result<(), hlsl::Error> {
    layout.reset();
    let mut st = 0;

    for (index, token) in tokens.iter().enumerate() {
        let is_active = packing.preprocessor.is_active();

        if let Some(action) = packing.process(token)? {
            if packing.preprocessor.is_active().is_none()
                || matches!(action, hlsl::Action::Include) && packing.preprocessor.is_active() != Some(false)
                || st != index && is_active != Some(false)
            {
                layout.invalidate();
            }
            st = index + 1;
        } else if token.kind == hlsl::TokenKind::Semicolon {
            if st < index && packing.preprocessor.is_active() == Some(true) {
                validate_decl(src, &tokens[st..index], buffer, packing, layout);
            } else if packing.preprocessor.is_active().is_none() {
                layout.invalidate();
            }
            st = index + 1;
        }
    }

    if st < tokens.len() && packing.preprocessor.is_active() == Some(true) {
        validate_decl(src, &tokens[st..], buffer, packing, layout);
    }

    layout.report(buffer, span, &mut packing.issues);
    Ok(())
}

fn validate_decl(src: &str, tokens: &[hlsl::Token<'_>], buffer: &str, packing: &mut Packing, layout: &mut Buffer) {
    let span = tokens[0].span;
    let push_unknown = |issues: &mut Vec<Issue>, layout: &mut Buffer, message: &str| {
        layout.invalidate();
        issues.push((
            tracing::Level::DEBUG,
            span,
            format!("cannot determine layout of cbuffer '{buffer}': {message}"),
        ));
    };
    let push_non_float = |issues: &mut Vec<Issue>, ty: hlsl::Type<'_>| {
        issues.push((
            tracing::Level::WARN,
            span,
            format!("cbuffer '{buffer}' uses non-float type '{ty}'; AviUtl2 expects float values"),
        ));
    };

    let result = with_decl(&packing.preprocessor, src, tokens, |text, tokens, decl| {
        if decl
            .modifiers
            .iter()
            .any(|&modifier| matches!(modifier, "static" | "groupshared"))
        {
            return;
        }

        let Some(alias) = resolve_alias(&packing.aliases, decl.ty) else {
            let is_keyword = matches!(decl.ty, hlsl::Type::Named(_))
                && tokens
                    .iter()
                    .find(|token| !matches!(token.kind, hlsl::TokenKind::Modifier(_)))
                    .is_some_and(|token| matches!(token.kind, hlsl::TokenKind::Keyword(_)));
            if is_keyword {
                push_non_float(&mut packing.issues, decl.ty);
            }

            push_unknown(&mut packing.issues, layout, "unresolved type");
            return;
        };

        let is_row_major = find_row_major(&decl.modifiers).or(if alias.has_packing {
            alias.is_row_major
        } else {
            packing.preprocessor.is_row_major
        });

        if !is_float_type(alias.ty) {
            push_non_float(&mut packing.issues, decl.ty);
        }

        let Some(base) = resolve_layout(alias.ty, is_row_major) else {
            push_unknown(&mut packing.issues, layout, "unresolved matrix packing");
            return;
        };

        for member in &decl.members {
            let dims = parse_dims(&packing.preprocessor, text, tokens, member);
            let Some(member_layout) = apply_dims(&base, dims.into_iter().chain(alias.dims.iter().copied())) else {
                push_unknown(&mut packing.issues, layout, "unresolved or excessive array length");
                continue;
            };

            layout.place_member(buffer, span, member, &member_layout, &mut packing.issues);
        }
    });

    match result {
        Ok(()) => {}
        Err(DeclError::Macro) => push_unknown(&mut packing.issues, layout, "unresolved macro"),
        Err(DeclError::Syntax) => push_unknown(&mut packing.issues, layout, "unresolved declaration"),
    }
}

fn with_decl<R>(
    preprocessor: &hlsl::Preprocessor,
    src: &str,
    tokens: &[hlsl::Token<'_>],
    f: impl FnOnce(&str, &[hlsl::Token<'_>], hlsl::Decl<'_>) -> R,
) -> Result<R, DeclError> {
    let text = preprocessor
        .expand(&src[tokens[0].span.st..tokens.last().unwrap().span.ed])
        .ok_or(DeclError::Macro)?;
    let tokens = hlsl::Lexer::new(&text)
        .filter(|token| token.as_ref().map_or(true, |token| !token.kind.is_trivia()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DeclError::Syntax)?;
    let decl = hlsl::Parser::parse_decl(&text, &tokens)
        .ok()
        .flatten()
        .ok_or(DeclError::Syntax)?;

    Ok(f(&text, &tokens, decl))
}

fn resolve_alias<'a>(aliases: &'a HashMap<String, Alias>, ty: hlsl::Type<'_>) -> Option<Cow<'a, Alias>> {
    let ty = match ty {
        hlsl::Type::Scalar(scalar) => hlsl::Type::Scalar(scalar),
        hlsl::Type::Vec { scalar, cols } => hlsl::Type::Vec { scalar, cols },
        hlsl::Type::Mat { scalar, rows, cols } => hlsl::Type::Mat { scalar, rows, cols },
        hlsl::Type::Named(name) => return aliases.get(name).map(Cow::Borrowed),
    };
    Some(Cow::Owned(Alias {
        ty,
        dims: Vec::new(),
        is_row_major: None,
        has_packing: false,
    }))
}

fn parse_dims(
    preprocessor: &hlsl::Preprocessor,
    src: &str,
    tokens: &[hlsl::Token<'_>],
    member: &hlsl::Member<'_>,
) -> Vec<Option<usize>> {
    if member.dims.is_empty() {
        return Vec::new();
    }

    let mut dst = Vec::with_capacity(member.dims.len());
    let mut curr = tokens.partition_point(|token| token.span.st < member.span.ed);

    while tokens
        .get(curr)
        .is_some_and(|token| token.kind == hlsl::TokenKind::LBracket)
    {
        let Some(ed) = hlsl::find_pair(tokens, curr) else {
            dst.push(None);
            break;
        };

        let dim = if ed == curr + 2
            && let hlsl::TokenKind::Number(text) = tokens[curr + 1].kind
            && text.bytes().all(|byte| byte.is_ascii_digit())
            && (text == "0" || !text.starts_with('0'))
        {
            text.parse::<i32>()
                .ok()
                .and_then(|val| usize::try_from(val).ok())
                .filter(|&val| val > 0)
        } else {
            preprocessor
                .evaluate(&src[tokens[curr].span.ed..tokens[ed].span.st])
                .and_then(|val| usize::try_from(val).ok())
                .filter(|&val| val > 0)
        };
        dst.push(dim);
        curr = ed + 1;
    }

    dst
}

fn find_row_major(modifiers: &[&str]) -> Option<bool> {
    modifiers.iter().rev().find_map(|&modifier| match modifier {
        "row_major" => Some(true),
        "column_major" => Some(false),
        _ => None,
    })
}

fn is_float_type(ty: hlsl::Type<'_>) -> bool {
    match ty {
        hlsl::Type::Scalar(scalar) | hlsl::Type::Vec { scalar, .. } | hlsl::Type::Mat { scalar, .. } => {
            matches!(scalar.name, "float" | "half" | "min16float" | "min10float")
        }
        hlsl::Type::Named(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn validate(src: &str) -> Vec<(tracing::Level, hlsl::Span, String)> {
        let tokens = hlsl::Lexer::new(src)
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        validate_cbuffers(src, &tokens).unwrap()
    }

    #[test]
    fn pack_single_rows() {
        for ty in ["row_major float1x3", "column_major float3x1"] {
            let messages = validate(&format!("cbuffer Values {{ float head; {ty} value; float4 tail; }};"));
            assert!(messages.is_empty(), "{ty}: {messages:?}");
        }
    }

    #[test]
    fn preserve_alias_packing() {
        let messages = validate(
            "#pragma pack_matrix(row_major)\n\
             typedef float2x3 Rows;\n\
             #pragma pack_matrix(column_major)\n\
             cbuffer Values { Rows value; float tail; };",
        );
        assert!(
            messages.iter().any(|(level, _, message)| *level == tracing::Level::INFO
                && message.contains("number slots 1..8")
                && message.contains("number x 3, padding(number), number x 3, number")),
            "{messages:?}"
        );
    }

    #[test]
    fn isolate_aliases() {
        let messages = validate("void helper() { typedef int Scalar; }\ncbuffer Values { Scalar value; };");
        assert!(
            messages
                .iter()
                .any(|(_, _, message)| message.contains("unresolved type"))
        );
        assert!(messages.iter().all(|(level, _, _)| *level == tracing::Level::DEBUG));
    }

    #[test]
    fn report_padding() {
        for (src, slots, pattern) in [
            (
                "float a[2]; float3 b;",
                8,
                "number, padding(number) x 3, number, number x 3",
            ),
            (
                "float3x3 a; float3 b;",
                15,
                "number x 3, padding(number), number x 3, padding(number), number x 3, padding(number), number x 3",
            ),
            (
                "row_major float2x3 a; float b;",
                8,
                "number x 3, padding(number), number x 3, number",
            ),
            (
                "column_major float2x3 a; float2 b;",
                12,
                "number x 2, padding(number) x 2, number x 2, padding(number) x 2, number x 2, number x 2",
            ),
            (
                "float a[2][2]; float3 b;",
                16,
                "number, padding(number) x 3, number, padding(number) x 3, \
                 number, padding(number) x 3, number, number x 3",
            ),
            ("float a; float4 b;", 8, "number, padding(number) x 3, number x 4"),
        ] {
            let messages = validate(&format!("cbuffer Values {{ {src} }};"));
            assert_eq!(messages.len(), 1, "{src}: {messages:?}");
            assert_eq!(messages[0].0, tracing::Level::INFO);
            assert_eq!(
                messages[0].2,
                format!("cbuffer 'Values' layout (number slots 1..{slots}): {pattern}")
            );
        }
    }

    #[test]
    fn omit_tail_padding() {
        for src in [
            "float a;",
            "float3 a;",
            "float4 a;",
            "float4 a[2];",
            "float3 a; float b;",
        ] {
            assert!(validate(&format!("cbuffer Values {{ {src} }};")).is_empty(), "{src}");
        }
    }

    #[test]
    fn warn_types() {
        for ty in [
            "int",
            "uint",
            "bool",
            "double",
            "int3",
            "uint2x2",
            "min16int",
            "min12int",
            "min16uint",
        ] {
            let messages = validate(&format!("cbuffer Values {{ {ty} value; }};"));
            assert_eq!(
                messages
                    .iter()
                    .filter(|(level, _, _)| *level == tracing::Level::WARN)
                    .count(),
                1,
                "{ty}: {messages:?}"
            );
            assert!(
                messages
                    .iter()
                    .any(|(_, _, message)| message.contains("AviUtl2 expects float values"))
            );
        }

        for ty in ["float", "float4", "half", "min16float", "min10float"] {
            assert!(
                validate(&format!("cbuffer Values {{ {ty} value; }};")).is_empty(),
                "{ty}"
            );
        }

        let messages = validate("cbuffer Values { float head; min16float value; float tail; };");
        assert!(
            messages
                .iter()
                .any(|(level, _, message)| *level == tracing::Level::INFO && message.contains("padding(number)"))
        );
        assert!(!messages.iter().any(|(level, _, _)| *level == tracing::Level::WARN));
    }

    #[test]
    fn resolve_aliases() {
        let src = "#define COUNT (1 + 1)\n\
            #define TYPE float\n\
            typedef TYPE Scalar;\n\
            typedef Scalar Values[COUNT];\n\
            typedef Values MoreValues;\n\
            typedef int Integer;\n\
            cbuffer Buffer { MoreValues a; float3 b; Integer c; };";
        let messages = validate(src);
        assert_eq!(
            messages
                .iter()
                .filter(|(level, _, _)| *level == tracing::Level::WARN)
                .count(),
            1
        );
        assert!(
            messages
                .iter()
                .any(|(level, _, message)| *level == tracing::Level::INFO && message.contains("padding(number)"))
        );
        assert!(
            !messages.iter().any(|(level, _, _)| *level == tracing::Level::DEBUG),
            "{messages:?}"
        );
    }

    #[test]
    fn resolve_conditionals() {
        let src = "#define FLAG 0\n\
            cbuffer Values {\n\
            #if FLAG\n\
            int ignored;\n\
            #elif 2 > 1\n\
            float3 a;\n\
            #else\n\
            bool ignored;\n\
            #endif\n\
            float3 b;\n\
            };\n\
            #undef FLAG\n\
            #ifdef FLAG\n\
            cbuffer Ignored { int value; };\n\
            #endif";
        let messages = validate(src);
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].0, tracing::Level::INFO);
        let src = "cbuffer Values {\n#if UNKNOWN\nfloat a;\n#else\nfloat2 a;\n#endif\nint b;\n};";
        let messages = validate(src);
        assert!(messages.iter().any(|(level, _, _)| *level == tracing::Level::DEBUG));
        assert!(messages.iter().any(|(level, _, _)| *level == tracing::Level::WARN));
        assert!(!messages.iter().any(|(level, _, _)| *level == tracing::Level::INFO));
    }

    #[test]
    fn preserve_unknown_layouts() {
        for src in [
            "cbuffer Values { Missing value; };",
            "cbuffer Values { float value[COUNT]; };",
            "cbuffer Values { float value[0]; };",
            "cbuffer Values { float value[-1]; };",
            "cbuffer Values { float4 value[4097]; };",
            "#include \"external.hlsl\"\ncbuffer Values { float2x3 value; };",
        ] {
            let messages = validate(src);
            assert!(!messages.is_empty(), "{src}");
            assert!(
                messages.iter().all(|(level, _, _)| *level == tracing::Level::DEBUG),
                "{src}: {messages:?}"
            );
        }

        let messages = validate(
            "#include \"external.hlsl\"\n#pragma pack_matrix(column_major)\ncbuffer Values { float2x3 a; float2 b; };",
        );
        assert!(messages.iter().any(|(level, _, _)| *level == tracing::Level::INFO));
    }

    #[test]
    fn resolve_offsets() {
        let messages = validate("cbuffer Values { float3 a : packoffset(c1); float3 b : packoffset(c0); };");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, tracing::Level::INFO);
        assert!(messages[0].2.ends_with("number x 3, padding(number), number x 3"));

        for src in [
            "float a; float b : packoffset(c1);",
            "float3 a : packoffset(c0.z);",
            "float2 a : packoffset(c0); float2 b : packoffset(c0.y);",
        ] {
            let messages = validate(&format!("cbuffer Values {{ {src} }};"));
            assert!(
                messages.iter().any(|(level, _, _)| *level == tracing::Level::DEBUG),
                "{src}: {messages:?}"
            );
            assert!(
                !messages.iter().any(|(level, _, _)| *level == tracing::Level::INFO),
                "{src}: {messages:?}"
            );
        }
    }

    #[test]
    fn preserve_known_regions() {
        let messages = validate(
            "cbuffer Values { float a; float4 b; Missing c; float3 d : packoffset(c2); float e : packoffset(c2.w); };",
        );
        assert!(
            messages.iter().any(
                |(level, _, message)| *level == tracing::Level::INFO && message.contains("known byte range 32..44")
            )
        );
        assert!(
            messages.iter().any(
                |(level, _, message)| *level == tracing::Level::INFO && message.contains("known byte range 44..48")
            )
        );
        assert!(messages.iter().any(|(level, _, _)| *level == tracing::Level::DEBUG));
        assert!(
            !messages
                .iter()
                .any(|(_, _, message)| message.contains("layout (number slots"))
        );
    }

    #[test]
    fn find_entries() {
        for (src, kind, is_expected) in [
            ("float4 main() : SV_Target { return 0; }", "pixelshader", true),
            ("void main(out float4 color) {}", "pixelshader", true),
            ("[numthreads(1,1,1)] void main() {}", "computeshader", true),
            (
                "[numthreads(1,1,1)] float4 main() { return 0; }",
                "computeshader",
                false,
            ),
            ("[numthreads(1,1,1)] void main() {}", "pixelshader", false),
            ("void main() {}", "computeshader", false),
            ("float4 main();", "pixelshader", false),
            ("float4 main_value() { return 0; }", "pixelshader", false),
            ("float main;", "pixelshader", false),
            (
                "// float4 main() {}\nfloat4 other() { return 0; }",
                "pixelshader",
                false,
            ),
        ] {
            let tokens = hlsl::Lexer::new(src)
                .filter(|token| {
                    token.as_ref().map_or(true, |token| {
                        !matches!(
                            token.kind,
                            hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                        )
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert_eq!(has_entry(src, &tokens, "main", kind).unwrap(), is_expected, "{src}");
        }
    }

    #[test]
    fn ignore_inactive_buffers() {
        let messages = validate(
            "#if 0\n\
             cbuffer Disabled { int value; };\n\
             #endif\n\
             /* cbuffer Comment { int value; }; */\n\
             cbuffer Visible { float4 value; };\n\
             cbuffer Other { float4 value; };",
        );
        assert!(messages.is_empty(), "{messages:?}");
        let messages = validate(
            "#if 0\n\
             #define VALUE int\n\
             #include \"missing.hlsl\"\n\
             #pragma pack_matrix(row_major)\n\
             #endif\n\
             cbuffer Values { float4 value; };",
        );
        assert!(messages.is_empty(), "{messages:?}");
    }

    #[test]
    fn resolve_macro_state() {
        for prefix in [
            "#define FLAG 1\n#if defined FLAG\n",
            "#define FLAG 1\n#ifdef FLAG\n",
            "#define FLAG 0\n#undef FLAG\n#ifndef FLAG\n",
            "#define FLAG 0\n#undef FLAG\n#if !defined(FLAG)\n",
            "#define FLAG 1\n#if 0\n#elif FLAG\n",
            "#define FLAG 1\n#if 0\n#else\n",
        ] {
            let messages = validate(&format!("{prefix}cbuffer Selected {{ int value; }};\n#endif"));
            assert_eq!(messages.len(), 1, "{prefix}: {messages:?}");
            assert_eq!(messages[0].0, tracing::Level::WARN);
            assert!(messages[0].2.contains("Selected"));
        }

        let messages = validate(
            "#define FLAG 1\n\
             #if FLAG\n\
             cbuffer First { int value; };\n\
             #elif 1\n\
             cbuffer Other { bool value; };\n\
             #else\n\
             cbuffer Last { uint value; };\n\
             #endif",
        );
        assert_eq!(messages.len(), 1);
        assert!(messages[0].2.contains("First"));
    }

    #[test]
    fn preserve_known_prefix() {
        let messages = validate("cbuffer Values { float a; float4 b; Missing c; float4 d; };");
        assert!(messages.iter().any(|(level, _, message)| *level == tracing::Level::INFO
            && message
                == "cbuffer 'Values' known prefix layout (number slots 1..8): \
                number, padding(number) x 3, number x 4"));
        assert!(messages.iter().any(|(level, _, _)| *level == tracing::Level::DEBUG));
        assert!(!messages.iter().any(|(_, _, message)| message.contains("1..12")));
    }

    #[test]
    fn resolve_matrix_defaults() {
        for prefix in [
            "#pragma pack_matrix(row_major)\ntypedef float2x3 Matrix;\n",
            "typedef float2x3 Matrix;\n#pragma pack_matrix(row_major)\n",
            "typedef row_major float2x3 Matrix;\n#pragma pack_matrix(column_major)\n",
        ] {
            let messages = validate(&format!("{prefix}cbuffer Values {{ Matrix a; float b; }};"));
            assert_eq!(messages.len(), 1, "{prefix}: {messages:?}");
            assert_eq!(messages[0].0, tracing::Level::INFO);
            assert!(messages[0].2.contains("number slots 1..8"));
        }

        let messages = validate("#include \"external.hlsl\"\ncbuffer Values { float2x2 a; float b; };");
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(
            messages
                .iter()
                .any(|(level, _, message)| *level == tracing::Level::INFO && message.contains("number slots 1..7"))
        );
    }

    #[test]
    fn resolve_array_dims() {
        let messages = validate("cbuffer Values { float a[010]; float4 b; };");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].0, tracing::Level::INFO);
        assert!(messages[0].2.contains("number slots 1..36"), "{messages:?}");

        let messages = validate("cbuffer Values { float a[2L]; float3 b; };");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].0, tracing::Level::INFO);
        assert!(messages[0].2.contains("number slots 1..8"), "{messages:?}");

        let messages = validate("cbuffer Values { float a[3000000000]; };");
        assert_eq!(messages.len(), 1, "{messages:?}");
        assert_eq!(messages[0].0, tracing::Level::DEBUG);
        assert!(
            messages[0].2.contains("unresolved or excessive array length"),
            "{messages:?}"
        );
    }
}
