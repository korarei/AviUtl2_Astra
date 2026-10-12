use super::hlsl;
use super::preprocess::{self, Action, Parser};
use super::source::{self, Position};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct Output {
    pub text: String,
}

pub(super) struct Builder<'a, 'state> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: &'state mut Cow<'a, IndexMap<String, String>>,
    include_stack: &'state mut Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    map: source::Map,
    repls: Vec<String>,
}

impl Builder<'_, '_> {
    pub(super) fn build<'a>(
        map: &source::Map,
        target: &'a BuildTarget,
        include_dirs: &'a [PathBuf],
        vars: &mut Cow<'a, IndexMap<String, String>>,
        include_stack: &mut Vec<PathBuf>,
    ) -> anyhow::Result<source::Map> {
        let mut builder = Builder {
            target,
            include_dirs,
            vars,
            include_stack,
            once_files: HashSet::new(),
            map: source::Map::default(),
            repls: Vec::new(),
        };

        builder.process(map)?;
        builder.replace_vals()?;
        Ok(builder.map)
    }

    #[allow(clippy::too_many_lines)]
    fn process(&mut self, map: &source::Map) -> anyhow::Result<()> {
        let src = map.text();
        let lines = map.lines();

        let locate = |at| map.locate(at);

        let mut lexer = hlsl::Lexer::new(&src).peekable();
        let omit_tokens = std::cell::OnceCell::new();
        let mut omit_parser = None;
        let mut omit_ed = 0;
        let mut blocked_ed = 0;
        let mut repl = None;
        let mut parser = Parser::new(preprocess::Lexer::new(&src));

        let mut i = 0;

        while lines.get(i).is_some_and(|&st| st < src.len()) {
            let ed = lines.get(i + 1).map_or(src.len(), |st| st - 1);

            if omit_ed != 0 && ed <= omit_ed {
                i += 1;
                continue;
            }

            let st = lines[i].max(omit_ed);
            let line = &src[st..ed];
            let mut blocked = blocked_ed;
            let mut first = None;
            let mut repls = Vec::new();
            let mut has_code = false;

            while let Some(token) = lexer.next_if(|token| token.as_ref().map_or(true, |token| token.span.st < ed)) {
                let token = token.map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;
                has_code |= !matches!(
                    token.kind,
                    hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                );

                if token.span.st >= st
                    && let hlsl::TokenKind::Comment(comment) = &token.kind
                    && comment.bracket.is_some()
                    && comment.content.split_whitespace().next() == Some("#replace")
                    && let Some(preprocess::Directive::Replace(_)) =
                        Parser::parse_directive(comment.content.trim_start(), true)
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(token.span.st)))?
                {
                    repls.push((token.span, comment.content.trim_start()));
                }

                if token.span.st >= st && token.span.ed > ed {
                    blocked = blocked.max(token.span.ed);
                }

                if first.is_none()
                    && token.span.st >= st
                    && !matches!(token.kind, hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline)
                {
                    first = Some(token);
                }
            }

            let comment = first.as_ref().and_then(|token| match token.kind {
                hlsl::TokenKind::Comment(comment) => Some(comment),
                _ => None,
            });
            let is_block = comment.is_some_and(|comment| comment.bracket.is_some());
            let directive = if blocked_ed > st {
                None
            } else {
                comment.and_then(|comment| {
                    let content = if is_block {
                        comment.content.trim_start()
                    } else {
                        comment.content
                    };
                    content.starts_with('#').then_some(content)
                })
            };

            blocked_ed = blocked;

            if let Some(pos) = &repl {
                if directive.is_some() {
                    bail!("{pos}: directive between #replace and its statement");
                }

                if has_code {
                    repl = None;
                }
            }

            let pos = locate(directive.map_or(st + line.len() - line.trim_start().len(), |content| {
                content.as_ptr() as usize - src.as_ptr() as usize
            }));

            let action = parser.process(
                directive.map(|content| (content, is_block)),
                self.vars.as_ref(),
                &pos,
                self.include_dirs,
            )?;
            let action = match action {
                Action::Replace(val) => {
                    self.repls.push(val);
                    repls.retain(|(span, _)| first.as_ref().is_none_or(|token| token.span != *span));

                    if !is_block {
                        repl = Some(pos.clone());
                    }
                    Action::Keep
                }
                action => action,
            };

            if matches!(action, Action::Keep) {
                for (span, content) in repls {
                    if let Action::Replace(val) = parser.process(
                        Some((content, true)),
                        self.vars.as_ref(),
                        &locate(span.st),
                        self.include_dirs,
                    )? {
                        self.repls.push(val);
                    }
                }
            }

            match &action {
                Action::Keep => {
                    let expanded = preprocess::expand(line, self.vars.as_ref())
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(st + err.offset)))?;
                    expanded.append(&mut self.map, map, st..ed);

                    if ed < src.len() {
                        self.map.append(map, ed..ed + 1);
                    } else {
                        self.map.generate("\n", map, ed..ed);
                    }
                }
                Action::Omit => {
                    let tokens = omit_tokens
                        .get_or_init(|| hlsl::Lexer::new(&src).collect::<Result<Vec<_>, _>>())
                        .as_ref()
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    let stmt = omit_parser
                        .get_or_insert_with(|| hlsl::Parser::new(&src, tokens))
                        .parse_stmt(ed + usize::from(ed < src.len()))
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;

                    for token in tokens[tokens.partition_point(|token| token.span.st <= ed)..]
                        .iter()
                        .take_while(|token| token.span.st < stmt.st)
                    {
                        if let hlsl::TokenKind::Comment(comment) = &token.kind
                            && comment.content.trim_start().starts_with('#')
                        {
                            bail!("{pos}: directive between #omit and its statement");
                        }
                    }

                    omit_ed = stmt.ed;
                }
                Action::Drop => {}
                Action::Replace(_) => bail!("{pos}: invalid #replace placement"),
                Action::PragmaOnce => {
                    let file = map.locate(0).file;
                    self.once_files
                        .insert(std::fs::canonicalize(file.as_ref()).unwrap_or_else(|_| file.to_path_buf()));
                }
                Action::Define(key, val) => {
                    if RESERVED_VARIABLES.contains(key) {
                        bail!("{pos}: cannot define reserved variable '{key}'");
                    }

                    let vars = self.vars.to_mut();
                    vars.insert(
                        (*key).to_owned(),
                        preprocess::expand(val, vars)
                            .map_err(|err| {
                                anyhow::anyhow!(
                                    "{}: {err}",
                                    locate(val.as_ptr() as usize - src.as_ptr() as usize + err.offset)
                                )
                            })?
                            .text
                            .into_owned(),
                    );
                }
                Action::Undef(key) => {
                    self.vars.to_mut().shift_remove(*key);
                }
                Action::Include(file) => self.include(file, &pos)?,
            }

            i = if (is_block || directive.is_some()) && !matches!(&action, Action::Keep) {
                first
                    .as_ref()
                    .map_or(i + 1, |token| lines.partition_point(|&st| st < token.span.ed))
            } else {
                i + 1
            };
        }

        if let Some(pos) = repl {
            bail!("{pos}: missing statement after #replace");
        }

        parser.finish()
    }

    #[allow(clippy::too_many_lines)]
    fn replace_vals(&mut self) -> anyhow::Result<()> {
        if self.repls.is_empty() {
            return Ok(());
        }

        let src = self.map.text();
        let tokens = hlsl::Lexer::new(&src)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
        let mut parser = hlsl::Parser::new(&src, &tokens);
        let mut vals = std::mem::take(&mut self.repls).into_iter();
        let mut result = source::Map::default();
        let mut curr = 0;

        for token in &tokens {
            let hlsl::TokenKind::Comment(comment) = &token.kind else {
                continue;
            };

            if comment.content.split_whitespace().next() != Some("#replace") {
                continue;
            }

            let pos = self.map.locate(token.span.st);

            if token.span.st < curr {
                bail!("{pos}: overlapping #replace expressions");
            }

            let val = vals
                .next()
                .ok_or_else(|| anyhow::anyhow!("{pos}: #replace was not resolved during preprocessing"))?;
            let span = if comment.bracket.is_some() {
                parser
                    .parse_expr(token.span.ed)
                    .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?
            } else {
                let stmt = parser
                    .parse_stmt(token.span.ed)
                    .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
                let st = tokens.partition_point(|next| next.span.st < stmt.st);
                let ed = tokens.partition_point(|next| next.span.st < stmt.ed);
                let mut depth = 0;
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

                let assign = assign.ok_or_else(|| {
                    anyhow::anyhow!("{pos}: #replace must precede a declaration or assignment with one value")
                })?;
                let prefix = tokens[st..assign]
                    .iter()
                    .filter(|next| {
                        !matches!(
                            next.kind,
                            hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                        )
                    })
                    .cloned()
                    .collect::<Vec<_>>();

                if let Ok(Some(declaration)) = hlsl::Parser::parse_declaration(&src, &prefix) {
                    if declaration.members.len() != 1 {
                        bail!("{pos}: #replace requires one target and one value");
                    }
                } else {
                    let target = hlsl::Parser::new(&src, &prefix)
                        .parse_expr(stmt.st)
                        .map_err(|_| anyhow::anyhow!("{pos}: #replace requires one assignment target"))?;
                    if prefix
                        .first()
                        .is_none_or(|next| !matches!(next.kind, hlsl::TokenKind::Ident(_)))
                        || prefix.last().is_none_or(|next| {
                            next.span.ed != target.ed
                                || !matches!(next.kind, hlsl::TokenKind::Ident(_) | hlsl::TokenKind::RBracket)
                        })
                    {
                        bail!("{pos}: #replace requires one assignment target");
                    }
                }

                let span = parser
                    .parse_expr(tokens[assign].span.ed)
                    .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;

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
                span
            };

            let val_tokens = hlsl::Lexer::new(&val)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;
            let val_span = hlsl::Parser::new(&val, &val_tokens)
                .parse_expr(0)
                .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;

            if val_tokens.iter().any(|token| {
                token.span.st >= val_span.ed
                    && !matches!(
                        token.kind,
                        hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                    )
            }) {
                bail!("{pos}: #replace value must contain one expression");
            }

            if val_tokens.iter().any(|token| {
                matches!(&token.kind, hlsl::TokenKind::Comment(comment) if comment.bracket.is_none())
                    && !val[token.span.ed..].contains('\n')
            }) {
                bail!("{pos}: #replace value cannot end with a line comment");
            }

            drop(val_tokens);

            let marker = if comment.bracket.is_some() {
                token.span.st..token.span.ed
            } else {
                src[..token.span.st].rfind('\n').map_or(0, |at| at + 1)
                    ..token.span.ed + usize::from(src[token.span.ed..].starts_with('\n'))
            };

            result.append(&self.map, curr..marker.start);
            result.append(&self.map, marker.end..span.st);

            if token.span.ed == span.st
                && let Some(prev) = src[..token.span.st].chars().next_back()
                && let Some(first) = val.chars().next()
                && ((prev.is_alphanumeric() || prev == '_') && (first.is_alphanumeric() || first == '_')
                    || matches!((prev, first), ('+', '+') | ('-', '-') | ('/' | '*', '/')))
            {
                result.generate(" ", &self.map, token.span.st..token.span.ed);
            }

            result.generate(val, &self.map, span.st..span.ed);
            curr = span.ed;
        }

        if vals.next().is_some() {
            bail!("unmatched #replace values after preprocessing");
        }

        result.append(&self.map, curr..self.map.len());
        drop(parser);
        drop(tokens);
        drop(src);
        self.map = result;
        Ok(())
    }

    fn include(&mut self, file: &Path, pos: &Position) -> anyhow::Result<()> {
        let file = std::fs::canonicalize(file)
            .with_context(|| format!("{pos}: failed to canonicalize include file '{}'", file.display()))?;

        if self.once_files.contains(&file) {
            return Ok(());
        }

        if self.include_stack.contains(&file) {
            bail!("{pos}: circular include detected: '{}'", file.display());
        }

        if self.include_stack.len() >= 64 {
            bail!("{pos}: include nesting exceeds 64 files");
        }

        self.include_stack.push(file.clone());

        let result = crate::fs::read_file(&file, self.target.encoding())
            .with_context(|| format!("{pos}: failed to read include file '{}'", file.display()))
            .and_then(|src| {
                self.process(&source::Map::new(
                    src,
                    Position {
                        file: file.as_path().into(),
                        line: 1,
                        col: 1,
                    },
                ))
            });
        let _ = self.include_stack.pop();

        result
    }
}

pub fn build(
    src: &str,
    file: &Path,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
) -> anyhow::Result<Output> {
    let mut vars = Cow::Borrowed(vars);
    let mut include_stack = std::fs::canonicalize(file).into_iter().collect();
    let map = Builder::build(
        &source::Map::new(
            src,
            Position {
                file: file.into(),
                line: 1,
                col: 1,
            },
        ),
        target,
        include_dirs,
        &mut vars,
        &mut include_stack,
    )?;

    Ok(Output {
        text: map.text().into_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(src: &str, vars: &IndexMap<String, String>) -> anyhow::Result<source::Map> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.hlsl" }))?;
        let map = source::Map::new(
            src,
            Position {
                file: Path::new("input.hlsl").into(),
                line: 1,
                col: 1,
            },
        );
        Builder::build(&map, &target, &[], &mut Cow::Borrowed(vars), &mut Vec::new())
    }

    #[test]
    fn distinguish_comments() {
        let src =
            "/*#if false*/\nfloat a;\nconst char* text = \"//#omit\";\nfloat b; //#omit\n#define MACRO /*#omit*/ 1\n";
        assert_eq!(process(src, &IndexMap::new()).unwrap().text(), src);
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
            assert_eq!(process(src, &IndexMap::new()).unwrap().text(), expected, "{src}");
        }

        for src in [
            "//#omit",
            "//#omit\n// comment",
            "//#omit\nfloat value",
            "//#omit\n//#define VALUE 1\nfloat value;",
        ] {
            assert!(process(src, &IndexMap::new()).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_replacement_positions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "first + second".to_owned())]);
        let src = "float value = /*#replace VALUE*/ 0;\nfloat next;";
        let map = process(src, &vars).unwrap();
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
            assert!(process(src, &vars).is_err(), "{src}");
        }
    }
}
