use crate::build::lang::astra::{self, Action, Preprocessor};
use crate::build::lang::hlsl;
use crate::build::source::{self, Pos};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpandKind {
    Raw,
    Skip,
    ShortString,
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    map: source::Map,
    replacements: Vec<Replacement>,
}

struct BlockState {
    ed: usize,
    kind: ExpandKind,
}

struct LineScan<'a> {
    first: Option<hlsl::Token<'a>>,
    has_code: bool,
}

struct Marker<'a> {
    st: usize,
    sigil_st: usize,
    body_st: usize,
    kind: astra::Directive<'a>,
}

impl Builder<'_> {
    fn preprocess(&mut self, map: &source::Map) -> anyhow::Result<()> {
        let src = map.text();
        let lines = map.lines();

        let locate = |at| map.locate(at);

        let mut lexer = hlsl::Lexer::new(&src).peekable();
        let omit_tokens = std::cell::OnceCell::new();
        let mut omit_parser = None;
        let mut omit_ed = 0;
        let mut block = BlockState {
            ed: 0,
            kind: ExpandKind::Raw,
        };
        let mut pending = None;
        let mut preprocessor = Preprocessor::new();
        let mut markers = Vec::new();
        let mut segments = Vec::new();
        let mut line_replacements = Vec::new();

        let mut i = 0;

        while lines.get(i).is_some_and(|&st| st < src.len()) {
            let ed = lines.get(i + 1).map_or(src.len(), |st| st - 1);

            if omit_ed != 0 && ed <= omit_ed {
                i += 1;
                continue;
            }

            let st = lines[i].max(omit_ed);

            if block.ed <= st
                && let Some((spans, next)) = preprocessor
                    .parse_verbatim(&src, st, |line| {
                        astra::split_sigil(line.trim().strip_prefix("//")?).map(|(_, body)| body)
                    })
                    .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?
            {
                if let Some(pos) = &pending {
                    bail!("{pos}: directive between #replace and its statement");
                }

                for span in spans {
                    self.map.copy(map, span);
                }

                lexer = hlsl::Lexer::new_at(&src, next)
                    .map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?
                    .peekable();
                i = lines.partition_point(|&st| st < next);
                continue;
            }

            let was_blocked = block.ed > st;
            markers.clear();
            segments.clear();
            line_replacements.clear();

            let scan = scan_line(&src, st..ed, &mut lexer, &mut block, &mut markers, &mut segments, map)?;

            let comment = scan.first.as_ref().and_then(|token| match token.kind {
                hlsl::TokenKind::Comment(comment) => Some(comment),
                _ => None,
            });
            let is_block = comment.is_some_and(|comment| comment.bracket.is_some());
            let directive = if was_blocked {
                None
            } else {
                comment.and_then(|comment| astra::split_sigil(comment.content))
            };

            if let Some(pos) = &pending {
                if directive.is_some() {
                    bail!("{pos}: directive between #replace and its statement");
                }

                if scan.has_code {
                    pending = None;
                }
            }

            let line = &src[st..ed];
            let sigil_st = directive.map_or(st + line.len() - line.trim_start().len(), |(at_sigil, _)| {
                at_sigil.as_ptr() as usize - src.as_ptr() as usize
            });
            let body_st = directive.map_or(sigil_st, |(_, body)| body.as_ptr() as usize - src.as_ptr() as usize);
            let pos = locate(sigil_st);

            let kind = if is_block
                && markers
                    .first()
                    .is_some_and(|marker| scan.first.as_ref().is_some_and(|token| token.span.st == marker.st))
            {
                None
            } else {
                directive
                    .map(|(_, body)| astra::parse_directive(body, is_block))
                    .transpose()
                    .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
                    .flatten()
            };
            let action = preprocessor
                .process(kind, self.vars.as_ref(), directive.map_or(st, |_| sigil_st), body_st)
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            let action = match action {
                Action::Replace(val) => {
                    line_replacements.push((scan.first.as_ref().unwrap().span.st, val));
                    pending = Some(pos.clone());
                    Action::Keep
                }
                action => action,
            };

            if matches!(action, Action::Keep) {
                for &Marker {
                    st,
                    sigil_st,
                    body_st,
                    kind,
                } in &markers
                {
                    if let Action::Replace(val) = preprocessor
                        .process(Some(kind), self.vars.as_ref(), sigil_st, body_st)
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?
                    {
                        line_replacements.push((st, val));
                    }
                }
            }

            let next_line = if (is_block || directive.is_some()) && !matches!(action, Action::Keep) {
                scan.first
                    .as_ref()
                    .map_or(i + 1, |token| lines.partition_point(|&st| st < token.span.ed))
            } else {
                i + 1
            };

            match action {
                Action::Keep => {
                    self.emit_line(map, &src, ed, &segments, &mut line_replacements)?;
                }
                Action::Omit => {
                    let tokens = omit_tokens
                        .get_or_init(|| hlsl::Lexer::new(&src).collect::<Result<Vec<_>, _>>())
                        .as_ref()
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    let stmt = omit_parser
                        .get_or_insert_with(|| hlsl::Parser::new(&src, tokens))
                        .parse_stmt_at(ed + usize::from(ed < src.len()))
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;

                    for token in tokens[tokens.partition_point(|token| token.span.st <= ed)..]
                        .iter()
                        .take_while(|token| token.span.st < stmt.st)
                    {
                        if let hlsl::TokenKind::Comment(comment) = &token.kind
                            && astra::split_sigil(comment.content).is_some()
                        {
                            bail!("{pos}: directive between #omit and its statement");
                        }
                    }

                    omit_ed = stmt.ed;
                }
                action => self.apply_action(map, &src, &pos, action)?,
            }

            i = next_line;
        }

        if let Some(pos) = pending {
            bail!("{pos}: missing statement after #replace");
        }

        preprocessor
            .finish()
            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))
    }

    fn emit_line(
        &mut self,
        map: &source::Map,
        src: &str,
        ed: usize,
        segments: &[(std::ops::Range<usize>, ExpandKind)],
        replacements: &mut [(usize, String)],
    ) -> anyhow::Result<()> {
        for (span, kind) in segments {
            let output_st = self.map.len();
            match kind {
                ExpandKind::Skip => {
                    for (marker, val) in replacements.iter_mut() {
                        if span.contains(marker) {
                            self.replacements.push(Replacement {
                                st: output_st + (*marker - span.start),
                                val: std::mem::take(val),
                            });
                        }
                    }
                    self.map.copy(map, span.clone());
                }
                kind => {
                    let escape = match kind {
                        ExpandKind::Raw => astra::Escape::None,
                        ExpandKind::ShortString => astra::Escape::Hlsl,
                        ExpandKind::Skip => unreachable!(),
                    };
                    let expansion = astra::expand(&src[span.clone()], self.vars.as_ref(), escape)
                        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(span.start + err.offset)))?;
                    self.map.apply(map, span.clone(), expansion.to_edits(span.start));
                }
            }
        }

        if ed < src.len() {
            self.map.copy(map, ed..=ed);
        } else {
            self.map.emit("\n", map, ed..ed);
        }

        Ok(())
    }

    fn apply_action(&mut self, map: &source::Map, src: &str, pos: &Pos, action: Action<'_>) -> anyhow::Result<()> {
        match action {
            Action::Keep | Action::Omit => unreachable!(),
            Action::Drop => {}
            Action::Replace(_) => bail!("{pos}: invalid #replace placement"),
            Action::PragmaOnce => {
                let file = map.locate(0).file;
                self.once_files
                    .insert(std::fs::canonicalize(file.as_ref()).unwrap_or_else(|_| file.to_path_buf()));
            }
            Action::Define { key, val } => {
                if RESERVED_VARIABLES.contains(&key) {
                    bail!("{pos}: cannot define reserved variable '{key}'");
                }

                let vars = self.vars.to_mut();
                vars.insert(
                    key.to_owned(),
                    astra::expand(val, vars, astra::Escape::None)
                        .map_err(|err| {
                            anyhow::anyhow!(
                                "{}: {err}",
                                map.locate(val.as_ptr() as usize - src.as_ptr() as usize + err.offset)
                            )
                        })?
                        .text
                        .into_owned(),
                );
            }
            Action::Undef(key) => {
                self.vars.to_mut().shift_remove(key);
            }
            Action::Include { path, is_quoted } => {
                let file = source::resolve_include(&path, is_quoted, pos, self.include_dirs)?;
                self.include_file(&file, pos)?;
            }
        }

        Ok(())
    }

    fn replace_vals(&mut self) -> anyhow::Result<()> {
        if self.replacements.is_empty() {
            return Ok(());
        }

        let replacements = std::mem::take(&mut self.replacements);
        let src = self.map.text();
        let tokens = hlsl::Lexer::new(&src)
            .map(|token| {
                let token = token.map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
                if let hlsl::TokenKind::Comment(comment) = &token.kind
                    && replacements
                        .binary_search_by_key(&token.span.st, |replacement| replacement.st)
                        .is_err()
                    && astra::split_sigil(comment.content)
                        .map(|(_, body)| astra::parse_replace(body))
                        .transpose()
                        .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(token.span.st)))?
                        .flatten()
                        .is_some()
                {
                    bail!(
                        "{}: #replace was not resolved during preprocessing",
                        self.map.locate(token.span.st)
                    );
                }
                Ok(token)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut parser = hlsl::Parser::new(&src, &tokens);
        let mut edits = Vec::new();
        let mut curr = 0;

        for Replacement { st, val } in replacements {
            let pos = self.map.locate(st);
            let index = tokens
                .binary_search_by_key(&st, |token| token.span.st)
                .map_err(|_| anyhow::anyhow!("{pos}: #replace marker was changed during preprocessing"))?;
            let token = &tokens[index];
            let hlsl::TokenKind::Comment(comment) = &token.kind else {
                bail!("{pos}: #replace marker was changed during preprocessing");
            };
            if st < curr {
                bail!("{pos}: overlapping #replace expressions");
            }

            let span = find_replace_span(
                &src,
                &tokens,
                &mut parser,
                &pos,
                token.span.ed,
                comment.bracket.is_some(),
                &self.map,
            )?;

            validate_replace_val(&pos, &val)?;

            let marker = if comment.bracket.is_some() {
                token.span.st..token.span.ed
            } else {
                src[..token.span.st].rfind('\n').map_or(0, |at| at + 1)
                    ..token.span.ed + usize::from(src[token.span.ed..].starts_with('\n'))
            };

            let should_separate = token.span.ed == span.st
                && src[..token.span.st]
                    .chars()
                    .next_back()
                    .zip(val.chars().next())
                    .is_some_and(|(prev, first)| {
                        (prev.is_alphanumeric() || prev == '_') && (first.is_alphanumeric() || first == '_')
                            || matches!((prev, first), ('+', '+') | ('-', '-') | ('/' | '*', '/'))
                    });
            edits.push(source::Edit {
                span: marker,
                text: should_separate.then(|| " ".into()),
            });
            edits.push(source::Edit {
                span: span.st..span.ed,
                text: Some(val.into()),
            });
            curr = span.ed;
        }

        let mut dst = source::Map::default();
        dst.apply(&self.map, 0..self.map.len(), edits);
        drop(tokens);
        drop(src);
        self.map = dst;
        Ok(())
    }

    fn include_file(&mut self, file: &Path, pos: &Pos) -> anyhow::Result<()> {
        let key = std::fs::canonicalize(file)
            .with_context(|| format!("{pos}: failed to canonicalize include file '{}'", file.display()))?;

        if self.once_files.contains(&key) {
            return Ok(());
        }

        if self.include_stack.contains(&key) {
            bail!("{pos}: circular include detected: '{}'", file.display());
        }

        if self.include_stack.len() >= 64 {
            bail!("{pos}: include nesting exceeds 64 files");
        }

        self.include_stack.push(key);

        let result = crate::fs::read_file(file, self.target.encoding())
            .with_context(|| format!("{pos}: failed to read include file '{}'", file.display()))
            .and_then(|src| {
                self.preprocess(&source::Map::new(
                    src,
                    Pos {
                        file: file.into(),
                        line: 1,
                        col: 1,
                    },
                ))
            });
        self.include_stack
            .pop()
            .expect("include stack must contain current file");

        result
    }
}

struct Replacement {
    st: usize,
    val: String,
}

pub fn build(
    map: &source::Map,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
) -> anyhow::Result<source::Map> {
    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        include_stack: std::fs::canonicalize(map.locate(0).file.as_ref()).into_iter().collect(),
        once_files: HashSet::new(),
        map: source::Map::default(),
        replacements: Vec::new(),
    };
    builder.preprocess(map)?;
    builder.replace_vals()?;
    Ok(builder.map)
}

fn scan_line<'a>(
    src: &'a str,
    line: std::ops::Range<usize>,
    lexer: &mut std::iter::Peekable<hlsl::Lexer<'a>>,
    block: &mut BlockState,
    markers: &mut Vec<Marker<'a>>,
    segments: &mut Vec<(std::ops::Range<usize>, ExpandKind)>,
    map: &source::Map,
) -> anyhow::Result<LineScan<'a>> {
    let std::ops::Range { start: st, end: ed } = line;
    let mut next_block = BlockState {
        ed: block.ed,
        kind: block.kind,
    };
    let mut first = None;
    let mut has_code = false;
    let mut curr = st;

    if block.ed > st {
        let prefix_ed = ed.min(block.ed);
        segments.push((st..prefix_ed, block.kind));
        curr = prefix_ed;
    }

    while let Some(token) = lexer.next_if(|token| token.as_ref().map_or(true, |token| token.span.st < ed)) {
        let token = token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
        has_code |= !token.kind.is_trivia();

        let kind = match &token.kind {
            hlsl::TokenKind::Comment(_) => ExpandKind::Skip,
            hlsl::TokenKind::String => ExpandKind::ShortString,
            _ => ExpandKind::Raw,
        };
        let tok_st = token.span.st.max(curr);
        if tok_st > curr {
            push_segment(segments, curr..tok_st, ExpandKind::Raw);
        }
        let tok_ed = token.span.ed.min(ed);
        if tok_ed > tok_st {
            push_segment(segments, tok_st..tok_ed, kind);
            curr = tok_ed;
        }

        if token.span.st >= st
            && let hlsl::TokenKind::Comment(comment) = &token.kind
            && comment.bracket.is_some()
            && let Some((at_sigil, body)) = astra::split_sigil(comment.content)
            && let Some(kind) =
                astra::parse_replace(body).map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(token.span.st)))?
        {
            markers.push(Marker {
                st: token.span.st,
                sigil_st: at_sigil.as_ptr() as usize - src.as_ptr() as usize,
                body_st: body.as_ptr() as usize - src.as_ptr() as usize,
                kind,
            });
        }

        if token.span.st >= st && token.span.ed > next_block.ed {
            next_block = BlockState {
                ed: token.span.ed,
                kind,
            };
        }

        if first.is_none()
            && token.span.st >= st
            && !matches!(token.kind, hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline)
        {
            first = Some(token);
        }
    }

    if curr < ed {
        push_segment(segments, curr..ed, ExpandKind::Raw);
    }

    *block = next_block;
    Ok(LineScan { first, has_code })
}

fn find_replace_span(
    src: &str,
    tokens: &[hlsl::Token<'_>],
    parser: &mut hlsl::Parser<'_, '_>,
    pos: &Pos,
    after: usize,
    is_block: bool,
    map: &source::Map,
) -> anyhow::Result<hlsl::Span> {
    if is_block {
        return parser
            .parse_expr_at(after)
            .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)));
    }

    let stmt = parser
        .parse_stmt_at(after)
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;
    let st = tokens.partition_point(|next| next.span.st < stmt.st);
    let ed = tokens.partition_point(|next| next.span.st < stmt.ed);
    let mut depth = 0usize;
    let mut assign = None;

    for (index, next) in tokens[st..ed].iter().enumerate() {
        match next.kind {
            hlsl::TokenKind::LParen | hlsl::TokenKind::LBracket | hlsl::TokenKind::LBrace => depth += 1,
            hlsl::TokenKind::RParen | hlsl::TokenKind::RBracket | hlsl::TokenKind::RBrace => depth -= 1,
            hlsl::TokenKind::Assign if depth == 0 => {
                assign = Some(st + index);
                break;
            }
            _ => {}
        }
    }

    let assign = assign
        .ok_or_else(|| anyhow::anyhow!("{pos}: #replace must precede a declaration or assignment with one value"))?;
    let prefix = &tokens[st..assign];

    if let Ok(Some(decl)) = hlsl::Parser::parse_decl(src, prefix) {
        if decl.members.len() != 1 {
            bail!("{pos}: #replace requires one target and one value");
        }
    } else {
        let target = hlsl::Parser::new(src, prefix)
            .parse_expr_at(stmt.st)
            .map_err(|_| anyhow::anyhow!("{pos}: #replace requires one assignment target"))?;
        if prefix
            .iter()
            .find(|token| !token.kind.is_trivia())
            .is_none_or(|next| !matches!(next.kind, hlsl::TokenKind::Ident(_)))
            || prefix.iter().rfind(|token| !token.kind.is_trivia()).is_none_or(|next| {
                next.span.ed != target.ed || !matches!(next.kind, hlsl::TokenKind::Ident(_) | hlsl::TokenKind::RBracket)
            })
        {
            bail!("{pos}: #replace requires one assignment target");
        }
    }

    let span = parser
        .parse_expr_at(tokens[assign].span.ed)
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;

    if tokens[st..ed].iter().any(|next| {
        next.span.st >= span.ed
            && !matches!(
                next.kind,
                hlsl::TokenKind::Whitespace
                    | hlsl::TokenKind::Newline
                    | hlsl::TokenKind::Comment(_)
                    | hlsl::TokenKind::Semicolon
            )
    }) {
        bail!("{pos}: #replace requires one target and one value");
    }

    Ok(span)
}

fn validate_replace_val(pos: &Pos, val: &str) -> anyhow::Result<()> {
    let tokens = hlsl::Lexer::new(val)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;
    let span = hlsl::Parser::new(val, &tokens)
        .parse_expr_at(0)
        .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;

    if tokens
        .iter()
        .any(|token| token.span.st >= span.ed && !token.kind.is_trivia())
    {
        bail!("{pos}: #replace value must contain one expression");
    }

    if tokens.iter().any(|token| {
        matches!(&token.kind, hlsl::TokenKind::Comment(comment) if comment.bracket.is_none())
            && !val[token.span.ed..].contains('\n')
    }) {
        bail!("{pos}: #replace value cannot end with a line comment");
    }

    Ok(())
}

fn push_segment(
    segments: &mut Vec<(std::ops::Range<usize>, ExpandKind)>,
    span: std::ops::Range<usize>,
    kind: ExpandKind,
) {
    if span.is_empty() {
        return;
    }

    if let Some((prev, prev_kind)) = segments.last_mut()
        && *prev_kind == kind
        && prev.end == span.start
        && kind != ExpandKind::Skip
    {
        prev.end = span.end;
    } else {
        segments.push((span, kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preprocess(src: &str, vars: &IndexMap<String, String>) -> anyhow::Result<source::Map> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.hlsl" }))?;
        let map = source::Map::new(
            src,
            Pos {
                file: Path::new("input.hlsl").into(),
                line: 1,
                col: 1,
            },
        );
        build(&map, &target, &[], vars)
    }

    #[test]
    fn replacement_offsets() {
        let vars = IndexMap::from([
            ("PREFIX".to_owned(), "123456".to_owned()),
            ("VALUE".to_owned(), "1".to_owned()),
            ("OTHER".to_owned(), "2".to_owned()),
            ("SOURCE".to_owned(), "float before;\nfloat pad;\n".to_owned()),
        ]);
        let src = "float x = ${PREFIX} + /*#replace VALUE*/0, y = /*#replace OTHER*/0;\n";
        let map = preprocess(src, &vars).unwrap();
        let text = map.text();
        assert_eq!(text, "float x = 123456 + 1, y = 2;\n");
        let st = src.find("*/0").unwrap() + 2;
        let pos = map.locate(text.find("1, y = 2").unwrap());
        assert_eq!((pos.line, pos.col), (1, src[..st].chars().count() + 1));
        assert_eq!(pos.file.as_ref(), Path::new("input.hlsl"));

        for (src, expected) in [
            (
                "const char* label=\"$${LABEL}\"; float x=/*#replace VALUE*/0;\n",
                "const char* label=\"${LABEL}\"; float x=1;\n",
            ),
            (
                "${SOURCE}float x=/*#replace VALUE*/0;\n",
                "float before;\nfloat pad;\nfloat x=1;\n",
            ),
        ] {
            assert_eq!(preprocess(src, &vars).unwrap().text(), expected);
        }
    }

    #[test]
    fn reject_introduced_replacement() {
        let vars = IndexMap::from([
            ("MARKER".to_owned(), "/*#replace VALUE*/".to_owned()),
            ("VALUE".to_owned(), "1".to_owned()),
        ]);
        let src = "float x=${MARKER}0; float y=/*#replace VALUE*/0;\n";
        assert!(preprocess(src, &vars).is_err());
    }

    #[test]
    fn leading_inline_replacement() {
        let vars = IndexMap::from([("VALUE".to_owned(), "1".to_owned())]);
        let src = "/*#replace VALUE*/0;\n//#define VALUE 2\n//#replace VALUE\nfloat x=0;\n";
        assert_eq!(preprocess(src, &vars).unwrap().text(), "1;\nfloat x=2;\n");
    }

    #[test]
    fn distinguish_comments() {
        let src =
            "/*#if false*/\nfloat a;\nconst char* text = \"//#omit\";\nfloat b; //#omit\n#define MACRO /*#omit*/ 1\n";
        assert_eq!(preprocess(src, &IndexMap::new()).unwrap().text(), src);
    }

    #[test]
    fn omit_targets() {
        for (src, expected) in [
            (
                "//#omit\n// annotation\n\nfloat removed;\nfloat kept;\n",
                "float kept;\n",
            ),
            (
                "//#omit\nfloat f() { if (true) { return 0; } return 1; }\nfloat kept;\n",
                "float kept;\n",
            ),
            ("//#omit\nif (true) f(); else g();\nfloat kept;\n", "float kept;\n"),
        ] {
            assert_eq!(preprocess(src, &IndexMap::new()).unwrap().text(), expected, "{src}");
        }

        for src in [
            "//#omit",
            "//#omit\n// comment",
            "//#omit\nfloat value",
            "//#omit\n//#define VALUE 1\nfloat value;",
        ] {
            assert!(preprocess(src, &IndexMap::new()).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_replacement_positions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "first + second".to_owned())]);
        let src = "float value = /*#replace VALUE*/ 0;\nfloat next;";
        let map = preprocess(src, &vars).unwrap();
        let text = map.text();
        assert_eq!(text, "float value =  first + second;\nfloat next;\n");
        assert_eq!(map.locate(text.find("first").unwrap()).to_string(), "input.hlsl:1:34");
        assert_eq!(map.locate(text.find("next").unwrap()).to_string(), "input.hlsl:2:7");
    }

    #[test]
    fn reject_replacements() {
        let vars = IndexMap::from([("VALUE".to_owned(), "1".to_owned())]);

        for src in [
            "//#replace VALUE",
            "//#replace VALUE\nfloat a=0,b=0;",
            "float a = /*#replace VALUE*/ ;",
            "float a; /*#replace VALUE*/",
            "//#replace VALUE\n//#define OTHER 2\nfloat a=0;",
        ] {
            assert!(preprocess(src, &vars).is_err(), "{src}");
        }
    }

    #[test]
    fn skips_comments_and_escapes_strings() {
        let vars = IndexMap::from([
            ("VAL".to_owned(), "2".to_owned()),
            ("TEXT".to_owned(), "a\tb\r\nc".to_owned()),
        ]);
        let src =
            "// ${MISSING}\n/*\n${MISSING}\n*/ float x = ${VAL}; /* ${MISSING} */\nconst char* s = \"${TEXT}\";\n";
        assert_eq!(
            preprocess(src, &vars).unwrap().text(),
            "// ${MISSING}\n/*\n${MISSING}\n*/ float x = 2; /* ${MISSING} */\nconst char* s = \"a\\tb\\nc\";\n"
        );
    }
}
