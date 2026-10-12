use super::super::hlsl;
use std::fmt::Write as _;

struct Packing {
    preprocessor: hlsl::Preprocessor,
    aliases: std::collections::HashMap<String, Option<Alias>>,
    result: Vec<(tracing::Level, hlsl::Span, String)>,
    offset: Option<(usize, bool)>,
    numbers: Vec<std::ops::Range<usize>>,
    prefix_ed: Option<usize>,
    has_explicit: bool,
    has_automatic: bool,
}

impl Packing {
    fn process(&mut self, token: &hlsl::Token<'_>) -> Result<Option<hlsl::Action>, hlsl::Error> {
        let action = self.preprocessor.process(token)?;

        match action {
            Some(hlsl::Action::Unknown) => self.result.push((
                tracing::Level::DEBUG,
                token.span,
                "cannot determine HLSL conditional expression".into(),
            )),
            Some(hlsl::Action::Include) => {
                for val in self.aliases.values_mut() {
                    *val = None;
                }

                self.result.push((
                    tracing::Level::DEBUG,
                    token.span,
                    "cannot track HLSL definitions and matrix packing through #include".into(),
                ));
            }
            _ => {}
        }

        Ok(action)
    }

    fn alias(&self, ty: hlsl::Type<'_>) -> Option<Alias> {
        let ty = match ty {
            hlsl::Type::Scalar(scalar) => hlsl::Type::Scalar(scalar),
            hlsl::Type::Vector { scalar, cols } => hlsl::Type::Vector { scalar, cols },
            hlsl::Type::Matrix { scalar, rows, cols } => hlsl::Type::Matrix { scalar, rows, cols },
            hlsl::Type::Named(name) => return self.aliases.get(name)?.clone(),
        };
        Some(Alias {
            ty,
            dims: Vec::new(),
            is_row_major: None,
            has_packing: false,
        })
    }

    fn dims(&self, src: &str, tokens: &[hlsl::Token<'_>], member: &hlsl::Member<'_>) -> Vec<Option<usize>> {
        let mut result = Vec::new();
        let mut curr = tokens.partition_point(|token| token.span.st < member.span.ed);
        while tokens
            .get(curr)
            .is_some_and(|token| token.kind == hlsl::TokenKind::LBracket)
        {
            let Some(ed) = hlsl::Parser::find_group(tokens, curr) else {
                result.push(None);
                break;
            };
            result.push(
                self.preprocessor
                    .evaluate(&src[tokens[curr].span.ed..tokens[ed].span.st])
                    .and_then(|val| usize::try_from(val).ok())
                    .filter(|&val| val > 0),
            );
            curr = ed + 1;
        }
        result
    }

    fn collect_aliases(&mut self, src: &str, tokens: &[hlsl::Token<'_>]) {
        let Some(text) = self
            .preprocessor
            .expand(&src[tokens[0].span.st..tokens.last().unwrap().span.ed])
        else {
            return;
        };
        let Ok(tokens) = hlsl::Lexer::new(&text)
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
        else {
            return;
        };
        let Ok(Some(declaration)) = hlsl::Parser::parse_declaration(&text, &tokens) else {
            return;
        };
        let alias = self.alias(declaration.ty);
        let is_row_major = declaration.modifiers.iter().rev().find_map(|&modifier| match modifier {
            "row_major" => Some(true),
            "column_major" => Some(false),
            _ => None,
        });

        for member in &declaration.members {
            let val = alias.clone().map(|mut alias| {
                let mut dims = self.dims(&text, &tokens, member);
                dims.extend(alias.dims);
                alias.dims = dims;

                if is_row_major.is_some() {
                    alias.is_row_major = is_row_major;
                    alias.has_packing = true;
                } else if !alias.has_packing && self.preprocessor.has_packing {
                    alias.is_row_major = self.preprocessor.is_row_major;
                    alias.has_packing = true;
                }
                alias
            });
            self.aliases.insert(member.name.to_owned(), val);
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
struct Layout {
    size: usize,
    alignment: usize,
    is_min_precision: bool,
    numbers: Vec<std::ops::Range<usize>>,
}

#[derive(Clone)]
struct Alias {
    ty: hlsl::Type<'static>,
    dims: Vec<Option<usize>>,
    is_row_major: Option<bool>,
    has_packing: bool,
}

#[allow(clippy::too_many_lines)]
pub(super) fn validate_cbuffers(
    src: &str,
    tokens: &[hlsl::Token<'_>],
) -> Result<Vec<(tracing::Level, hlsl::Span, String)>, hlsl::Error> {
    let mut packing = Packing {
        preprocessor: hlsl::Preprocessor::new(),
        aliases: std::collections::HashMap::new(),
        result: Vec::new(),
        offset: Some((0, false)),
        numbers: Vec::new(),
        prefix_ed: None,
        has_explicit: false,
        has_automatic: false,
    };
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
            let st = curr + 1;
            let mut ed = st;

            while ed < tokens.len() && tokens[ed].kind != hlsl::TokenKind::Semicolon {
                if matches!(
                    tokens[ed].kind,
                    hlsl::TokenKind::LBrace | hlsl::TokenKind::LParen | hlsl::TokenKind::LBracket
                ) {
                    ed = hlsl::Parser::find_group(tokens, ed).unwrap_or(ed);
                }

                ed += 1;
            }

            if st < ed {
                packing.collect_aliases(src, &tokens[st..ed]);
            }

            curr = ed + usize::from(ed < tokens.len());
            continue;
        }

        if tokens[curr].kind != hlsl::TokenKind::Keyword("cbuffer") {
            match tokens[curr].kind {
                hlsl::TokenKind::LBrace => depth += 1,
                hlsl::TokenKind::RBrace if depth != 0 => depth -= 1,
                _ => {}
            }
            curr += 1;
            continue;
        }

        let span = tokens[curr].span;
        let is_active = packing.preprocessor.is_active();
        curr += 1;
        let name = tokens
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
            continue;
        }

        let Some(ed) = hlsl::Parser::find_group(tokens, curr) else {
            packing.result.push((
                tracing::Level::DEBUG,
                span,
                format!("cannot analyze unclosed cbuffer '{name}'"),
            ));
            break;
        };

        if is_active == Some(true) {
            validate_buffer(src, &tokens[curr + 1..ed], name, span, &mut packing)?;
        } else {
            if is_active.is_none() {
                packing.result.push((
                    tracing::Level::DEBUG,
                    span,
                    format!("cannot determine active declarations in cbuffer '{name}'"),
                ));
            }

            for token in &tokens[curr + 1..ed] {
                let _ = packing.process(token)?;
            }
        }

        curr = ed + 1;
    }

    packing.preprocessor.finish()?;
    Ok(packing.result)
}

pub(super) fn has_entry(src: &str, tokens: &[hlsl::Token<'_>], name: &str, kind: &str) -> Result<bool, hlsl::Error> {
    Ok(hlsl::Parser::new(src, tokens)
        .parse()?
        .functions
        .into_iter()
        .any(|func| {
            func.name == name
                && match kind {
                    "computeshader" => func.has_numthreads && func.is_void,
                    _ => !func.has_numthreads,
                }
        }))
}

#[allow(clippy::too_many_lines)]
fn validate_buffer(
    src: &str,
    tokens: &[hlsl::Token<'_>],
    name: &str,
    span: hlsl::Span,
    packing: &mut Packing,
) -> Result<(), hlsl::Error> {
    packing.offset = Some((0, false));
    packing.numbers.clear();
    packing.prefix_ed = None;
    packing.has_explicit = false;
    packing.has_automatic = false;
    let mut st = 0;

    for (index, token) in tokens.iter().enumerate() {
        let is_active = packing.preprocessor.is_active();

        if let Some(action) = packing.process(token)? {
            if packing.preprocessor.is_active().is_none()
                || matches!(action, hlsl::Action::Include) && packing.preprocessor.is_active() != Some(false)
                || st != index && is_active != Some(false)
            {
                if packing.prefix_ed.is_none() {
                    packing.prefix_ed = Some(packing.offset.map_or(0, |(ed, _)| ed));
                }
                packing.offset = None;
            }
            st = index + 1;
        } else if token.kind == hlsl::TokenKind::Semicolon {
            if st < index && packing.preprocessor.is_active() == Some(true) {
                validate_declaration(src, &tokens[st..index], name, packing);
            } else if packing.preprocessor.is_active().is_none() {
                packing.prefix_ed.get_or_insert(packing.offset.map_or(0, |(ed, _)| ed));
                packing.offset = None;
            }
            st = index + 1;
        }
    }

    if st < tokens.len() && packing.preprocessor.is_active() == Some(true) {
        validate_declaration(src, &tokens[st..], name, packing);
    }

    if packing.has_explicit && packing.has_automatic {
        packing.result.push((
            tracing::Level::DEBUG,
            span,
            format!("cannot determine layout of cbuffer '{name}': mixed packoffset and automatic declarations"),
        ));
        return Ok(());
    }

    packing
        .numbers
        .sort_unstable_by_key(|range: &std::ops::Range<usize>| range.start);
    let limit = packing
        .prefix_ed
        .unwrap_or_else(|| packing.numbers.last().map_or(0, |range| range.end));
    let mut pattern = String::new();
    let mut curr = 0;
    let mut has_padding = false;
    let push = |pattern: &mut String, label: &str, count: usize| {
        if count == 0 {
            return;
        }

        if !pattern.is_empty() {
            pattern.push_str(", ");
        }
        pattern.push_str(label);
        if count > 1 {
            write!(pattern, " x {count}").unwrap();
        }
    };

    for range in packing.numbers.iter().filter(|range| range.start < limit) {
        if range.start < curr {
            packing.result.push((
                tracing::Level::DEBUG,
                span,
                format!("cannot determine number ordering in cbuffer '{name}': overlapping member locations"),
            ));
            return Ok(());
        }

        if range.start > curr {
            has_padding = true;
            push(&mut pattern, "padding(number)", (range.start - curr) / 4);
        }

        let ed = range.end.min(limit);

        if ed > curr {
            push(&mut pattern, "number", (ed - curr.max(range.start)) / 4);
            curr = ed;
        }
    }

    if has_padding {
        packing.result.push((
            tracing::Level::INFO,
            span,
            format!(
                "cbuffer '{name}' {}layout (number slots 1..{}): {pattern}",
                if packing.prefix_ed.is_some() {
                    "known prefix "
                } else {
                    ""
                },
                limit / 4
            ),
        ));
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_declaration(src: &str, tokens: &[hlsl::Token<'_>], name: &str, packing: &mut Packing) {
    let span = tokens[0].span;
    let unknown = |packing: &mut Packing, message| {
        packing.prefix_ed.get_or_insert(packing.offset.map_or(0, |(ed, _)| ed));
        packing.offset = None;
        packing.result.push((
            tracing::Level::DEBUG,
            span,
            format!("cannot determine layout of cbuffer '{name}': {message}"),
        ));
    };
    let Some(text) = packing
        .preprocessor
        .expand(&src[span.st..tokens.last().unwrap().span.ed])
    else {
        unknown(packing, "unresolved macro");
        return;
    };

    let Ok(expanded) = hlsl::Lexer::new(&text)
        .filter(|token| {
            token.as_ref().map_or(true, |token| {
                !matches!(
                    token.kind,
                    hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()
    else {
        unknown(packing, "unresolved declaration");
        return;
    };

    let Ok(Some(declaration)) = hlsl::Parser::parse_declaration(&text, &expanded) else {
        unknown(packing, "unresolved declaration");
        return;
    };

    if declaration
        .modifiers
        .iter()
        .any(|&modifier| matches!(modifier, "static" | "groupshared"))
    {
        return;
    }

    let Some(alias) = packing.alias(declaration.ty) else {
        if let hlsl::Type::Named(ty) = declaration.ty
            && matches!(
                hlsl::Lexer::new(ty).next(),
                Some(Ok(hlsl::Token {
                    kind: hlsl::TokenKind::Keyword(_),
                    ..
                }))
            )
        {
            packing.result.push((
                tracing::Level::WARN,
                span,
                format!("cbuffer '{name}' uses non-float type '{ty}'; AviUtl2 expects float values"),
            ));
        }

        unknown(packing, "unresolved type");
        return;
    };

    let is_row_major = declaration
        .modifiers
        .iter()
        .rev()
        .find_map(|&modifier| match modifier {
            "row_major" => Some(true),
            "column_major" => Some(false),
            _ => None,
        })
        .or(if alias.has_packing {
            alias.is_row_major
        } else {
            packing.preprocessor.is_row_major
        });

    if !match alias.ty {
        hlsl::Type::Scalar(scalar) | hlsl::Type::Vector { scalar, .. } | hlsl::Type::Matrix { scalar, .. } => {
            matches!(scalar.name, "float" | "half" | "min16float" | "min10float")
        }
        hlsl::Type::Named(_) => false,
    } {
        packing.result.push((
            tracing::Level::WARN,
            span,
            format!(
                "cbuffer '{name}' uses non-float type '{}'; AviUtl2 expects float values",
                declaration.ty
            ),
        ));
    }

    let Some(layout) = (match is_row_major {
        Some(is_row_major) => to_layout(alias.ty, is_row_major),
        None => to_layout(alias.ty, false).filter(|layout| Some(layout) == to_layout(alias.ty, true).as_ref()),
    }) else {
        unknown(packing, "unresolved matrix packing");
        return;
    };

    for member in &declaration.members {
        let mut dims = packing.dims(&text, &expanded, member);
        dims.extend(alias.dims.iter().copied());
        let mut layout = Some(std::borrow::Cow::Borrowed(&layout));

        for len in dims.into_iter().rev() {
            layout = layout.zip(len).and_then(|(layout, len)| {
                let stride = layout.size.checked_add(15)? / 16 * 16;
                let size = stride.checked_mul(len.checked_sub(1)?)?.checked_add(layout.size)?;
                if size > 65_536 {
                    return None;
                }

                let mut numbers = Vec::new();

                for index in 0..len {
                    let st = stride * index;
                    numbers.extend(layout.numbers.iter().map(|range| st + range.start..st + range.end));
                }
                Some(std::borrow::Cow::Owned(Layout {
                    size,
                    alignment: 16,
                    is_min_precision: layout.is_min_precision,
                    numbers,
                }))
            });
        }

        let Some(layout) = layout else {
            unknown(packing, "unresolved or excessive array length");
            continue;
        };

        let annotation = member.annotation.filter(|annotation| annotation.name == "packoffset");
        let at = if let Some(annotation) = annotation {
            packing.has_explicit = true;
            annotation
                .args
                .and_then(parse_pack_offset)
                .filter(|&at| at % layout.alignment == 0 && (layout.alignment >= 16 || at % 16 + layout.size <= 16))
        } else {
            packing.has_automatic = true;
            packing.offset.and_then(|(st, is_min_precision)| {
                let at = st.checked_add(layout.alignment - 1)? / layout.alignment * layout.alignment;
                if st % 16 != 0 && is_min_precision != layout.is_min_precision
                    || layout.alignment < 16 && at % 16 + layout.size > 16
                {
                    Some(at.checked_add(15)? / 16 * 16)
                } else {
                    Some(at)
                }
            })
        };

        let Some(at) = at.filter(|&at| at.checked_add(layout.size).is_some_and(|ed| ed <= 65_536)) else {
            unknown(
                packing,
                if annotation.is_some() {
                    "unresolved packoffset"
                } else {
                    "unresolved member offset"
                },
            );
            continue;
        };

        let ed = at + layout.size;
        packing.offset = Some((ed, layout.is_min_precision));
        packing
            .numbers
            .extend(layout.numbers.iter().map(|range| at + range.start..at + range.end));

        if packing.prefix_ed.is_some() && annotation.is_some() {
            packing.result.push((
                tracing::Level::INFO,
                span,
                format!(
                    "cbuffer '{name}' member '{}' has known byte range {at}..{ed}; surrounding layout is unknown",
                    member.name
                ),
            ));
        }
    }
}

fn parse_pack_offset(src: &str) -> Option<usize> {
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
        .ok()?;
    let hlsl::TokenKind::Ident(register) = tokens.first()?.kind else {
        return None;
    };
    let register = register.strip_prefix('c')?.parse::<usize>().ok()?;
    let component = match tokens.as_slice() {
        [_] => 0,
        [
            _,
            hlsl::Token {
                kind: hlsl::TokenKind::Dot,
                ..
            },
            hlsl::Token {
                kind: hlsl::TokenKind::Ident(component),
                ..
            },
        ] => match *component {
            "x" => 0,
            "y" => 4,
            "z" => 8,
            "w" => 12,
            _ => return None,
        },
        _ => return None,
    };
    register
        .checked_mul(16)?
        .checked_add(component)
        .filter(|&at| at < 65_536)
}

fn to_layout(ty: hlsl::Type<'_>, is_row_major: bool) -> Option<Layout> {
    let (scalar, len, width) = match ty {
        hlsl::Type::Scalar(scalar) => (scalar, 1, 1),
        hlsl::Type::Vector { scalar, cols } => (scalar, 1, cols),
        hlsl::Type::Matrix { scalar, rows, cols } if is_row_major => (scalar, rows, cols),
        hlsl::Type::Matrix { scalar, rows, cols } => (scalar, cols, rows),
        hlsl::Type::Named(_) => return None,
    };
    let width = width.checked_mul(scalar.size)?;
    let stride = width.checked_add(15)? / 16 * 16;
    Some(Layout {
        size: len.checked_sub(1)?.checked_mul(stride)?.checked_add(width)?,
        alignment: if len > 1 { 16 } else { scalar.size },
        is_min_precision: matches!(
            scalar.name,
            "min16float" | "min10float" | "min16int" | "min12int" | "min16uint"
        ),
        numbers: (0..len).map(|index| index * stride..index * stride + width).collect(),
    })
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
        for (text, expected) in [
            ("c0", 0),
            ("c1.x", 16),
            ("c2.y", 36),
            ("c2.z", 40),
            ("c4095.w", 65532),
            ("c2 /* gap */ . w", 44),
        ] {
            assert_eq!(parse_pack_offset(text), Some(expected), "{text}");
        }

        for text in ["", "c", "c-1", "c4096", "c0.q", "c0.xy", "c0.", "c0.x extra", "t0"] {
            assert_eq!(parse_pack_offset(text), None, "{text}");
        }

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
    }
}
