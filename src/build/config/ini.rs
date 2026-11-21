use crate::build::lang::astra::{self, Action, Preprocessor};
use crate::build::source::{self, Pos};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

enum Pending {
    Omit(Pos),
    Replace { val: String, pos: Pos },
    SkipSection,
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    map: source::Map,
}

impl Builder<'_> {
    fn preprocess(&mut self, map: &source::Map) -> anyhow::Result<()> {
        let src = map.text();
        let mut preprocessor = Preprocessor::new();
        let mut pending = None;
        let mut curr = 0;
        let mut lines = src.split_inclusive('\n');

        while let Some(line) = lines.next() {
            let st = curr;
            curr += line.len();

            if !matches!(pending, Some(Pending::SkipSection))
                && let Some((spans, next)) = preprocessor
                    .parse_verbatim(&src, st, |line| {
                        astra::split_sigil(line.trim().strip_prefix(';')?).map(|(_, body)| body)
                    })
                    .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?
            {
                if pending.is_some() {
                    bail!("{}: verbatim directive before pending INI entry", map.locate(st));
                }

                for span in spans {
                    self.map.copy(map, span);
                }

                curr = next;
                lines = src[next..].split_inclusive('\n');
                continue;
            }

            let has_newline = line.ends_with('\n');
            let line = line.trim_suffix('\n');
            let content = line.trim_start();
            let has_content = !content.is_empty() && !content.starts_with(';');
            let directive = content.strip_prefix(';').and_then(astra::split_sigil);
            let section_comment_st = find_section_comment(content, line.len());
            let is_section = section_comment_st.is_some();

            if matches!(pending, Some(Pending::SkipSection)) {
                if !is_section {
                    continue;
                }

                pending = None;
            }

            let directive_st = st + line.len() - directive.map_or(content.len(), |(at_sigil, _)| at_sigil.len());
            let body_st = st + line.len() - directive.map_or(content.len(), |(_, body)| body.len());
            let pos = map.locate(directive_st);

            match &pending {
                Some(Pending::Omit(pos)) => {
                    if directive.is_some() {
                        bail!("{pos}: directive between #omit and its line");
                    }

                    if has_content {
                        pending = is_section.then_some(Pending::SkipSection);
                    }
                    continue;
                }
                Some(Pending::Replace { .. }) if directive.is_some() => {
                    bail!("{pos}: directive between #replace and its entry");
                }
                Some(Pending::SkipSection) => unreachable!(),
                Some(Pending::Replace { .. }) | None => {}
            }

            let kind = directive
                .map(|(_, body)| astra::parse_directive(body, false))
                .transpose()
                .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
                .flatten();
            let action = preprocessor
                .process(kind, self.vars.as_ref(), directive_st, body_st)
                .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;

            match action {
                Action::Keep => self.emit_line(
                    &Line {
                        map,
                        st,
                        text: line,
                        has_newline,
                    },
                    section_comment_st,
                    &mut pending,
                )?,
                action => self.apply_action(
                    &Line {
                        map,
                        st,
                        text: line,
                        has_newline,
                    },
                    pos,
                    action,
                    &mut pending,
                )?,
            }
        }

        match pending {
            Some(Pending::Omit(pos)) => bail!("{pos}: missing line after #omit"),
            Some(Pending::Replace { pos, .. }) => bail!("{pos}: missing entry after #replace"),
            Some(Pending::SkipSection) | None => {}
        }

        preprocessor
            .finish()
            .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))
    }

    fn emit_line(
        &mut self,
        line: &Line<'_>,
        section_comment_st: Option<usize>,
        pending: &mut Option<Pending>,
    ) -> anyhow::Result<()> {
        let Line {
            map,
            st,
            text,
            has_newline,
        } = *line;
        let ed = st + text.len();
        let content = text.trim_start();
        let has_content = !content.is_empty() && !content.starts_with(';');
        let locate = |at| map.locate(st + at);

        if has_content && let Some(Pending::Replace { val, pos }) = pending.take() {
            let Some((key, rest)) = text.split_once('=') else {
                bail!("{pos}: #replace must precede an INI entry with a value");
            };

            if section_comment_st.is_some() || key.trim().is_empty() || rest.trim().is_empty() {
                bail!("{pos}: #replace must precede an INI entry with a value");
            }

            let val_st = key.len() + 1 + rest.len() - rest.trim_start().len();
            let val_ed = key.len() + 1 + rest.trim_end().len();
            let prefix = astra::expand(&text[..val_st], self.vars.as_ref(), astra::Escape::None)
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            if let Some(at) = prefix.text.find('\n') {
                bail!("{}: INI key cannot contain a newline", locate(prefix.to_src_offset(at)));
            }

            self.map.apply(map, st..st + val_st, prefix.to_edits(st));
            self.map.emit(val, map, st + val_st..st + val_ed);
            self.map.copy(map, st + val_ed..ed);
        } else if content.starts_with(';') {
            self.map.copy(map, st..ed);
        } else if let Some(comment_st) = section_comment_st {
            let expansion = astra::expand(&text[..comment_st], self.vars.as_ref(), astra::Escape::None)
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            if let Some(at) = expansion.text.find('\n') {
                bail!(
                    "{}: INI section name cannot contain a newline",
                    locate(expansion.to_src_offset(at))
                );
            }
            self.map.apply(map, st..st + comment_st, expansion.to_edits(st));
            if comment_st < text.len() {
                self.map.copy(map, st + comment_st..ed);
            }
        } else {
            let is_entry = text.split_once('=').is_some_and(|(key, _)| !key.trim().is_empty());
            let expansion = astra::expand(text, self.vars.as_ref(), astra::Escape::None)
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            if is_entry && let Some(at) = expansion.text.find('\n') {
                bail!(
                    "{}: INI entry cannot contain a newline",
                    locate(expansion.to_src_offset(at))
                );
            }
            self.map.apply(map, st..ed, expansion.to_edits(st));
        }

        if has_newline {
            self.map.copy(map, ed..=ed);
        }

        Ok(())
    }

    fn apply_action(
        &mut self,
        line: &Line<'_>,
        pos: Pos,
        action: Action<'_>,
        pending: &mut Option<Pending>,
    ) -> anyhow::Result<()> {
        let Line {
            map,
            st,
            text,
            has_newline,
        } = *line;

        match action {
            Action::Keep => unreachable!(),
            Action::Omit => *pending = Some(Pending::Omit(pos)),
            Action::Replace(val) => {
                if val.contains('\n') {
                    bail!("{pos}: INI #replace value cannot contain a newline");
                }

                *pending = Some(Pending::Replace { val, pos });
            }
            Action::Drop => {}
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
                                map.locate(st + (val.as_ptr() as usize - text.as_ptr() as usize) + err.offset)
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
                let file = source::resolve_include(&path, is_quoted, &pos, self.include_dirs)?;
                self.include_file(&file, &pos, has_newline)?;
            }
        }

        Ok(())
    }

    fn include_file(&mut self, file: &Path, pos: &Pos, has_newline: bool) -> anyhow::Result<()> {
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

        let st = self.map.len();
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

        result?;

        if has_newline && self.map.len() > st && !self.map.ends_with('\n') {
            self.map.emit_at_end("\n");
        }

        Ok(())
    }
}

struct Line<'a> {
    map: &'a source::Map,
    st: usize,
    text: &'a str,
    has_newline: bool,
}

fn find_section_comment(content: &str, line_len: usize) -> Option<usize> {
    content
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .and_then(|(_, rest)| {
            let tail = rest.trim_start();
            if tail.is_empty() {
                Some(line_len)
            } else if tail.starts_with(';') {
                Some(line_len - tail.len())
            } else {
                None
            }
        })
}

pub fn build(
    src: &str,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
) -> anyhow::Result<source::Map> {
    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        include_stack: std::fs::canonicalize(target.path()).into_iter().collect(),
        once_files: HashSet::new(),
        map: source::Map::default(),
    };

    builder.preprocess(&source::Map::new(
        src,
        Pos {
            file: Path::new(target.path()).into(),
            line: 1,
            col: 1,
        },
    ))?;
    Ok(builder.map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preprocess(src: &str, vars: &IndexMap<String, String>) -> anyhow::Result<source::Map> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.ini" }))?;
        let mut builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Borrowed(vars),
            include_stack: Vec::new(),
            once_files: HashSet::new(),
            map: source::Map::default(),
        };
        builder.preprocess(&source::Map::new(
            src,
            Pos {
                file: Path::new("input.ini").into(),
                line: 1,
                col: 1,
            },
        ))?;
        Ok(builder.map)
    }

    #[test]
    fn pending_transitions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "7".to_owned())]);
        for (src, expected) in [
            (
                ";#omit\n[removed]\nx=0\n[next]\n;#replace VALUE\n; note\nx=0\n",
                "[next]\n; note\nx=7\n",
            ),
            (
                ";#replace VALUE\n\n; comment\nx=0\n;#omit\ny=0\nz=1\n",
                "\n; comment\nx=7\nz=1\n",
            ),
        ] {
            assert_eq!(preprocess(src, &vars).unwrap().text(), expected);
        }
    }

    #[test]
    fn preserve_lines() {
        for src in ["", "key=1", "key=1\n", "; comment\n\n[section]\n key = 1 "] {
            let map = preprocess(src, &IndexMap::new()).unwrap();
            assert_eq!(map.text(), src);
            if !src.is_empty() {
                assert_eq!(map.locate(0).to_string(), "input.ini:1:1");
            }
        }
    }

    #[test]
    fn omit_targets() {
        for (src, expected) in [
            (";#omit\n; annotation\n\nkey=0\nnext=1\n", "next=1\n"),
            (";#omit\n[removed]\nkey=0\n[next]\nkey=1\n", "[next]\nkey=1\n"),
            (";#omit\n[not a heading] extra\nnext=1\n", "next=1\n"),
            (";#if false\n;#omit\n;#endif\nkey=1\n", "key=1\n"),
        ] {
            assert_eq!(preprocess(src, &IndexMap::new()).unwrap().text(), expected, "{src}");
        }

        for src in [";#omit", ";#omit\n; annotation\n", ";#omit\n;#define VALUE 1\nkey=0"] {
            assert!(preprocess(src, &IndexMap::new()).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_replacement_positions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "日本語".to_owned())]);
        let src = ";#replace VALUE\n  key = 0  \nnext=1";
        let map = preprocess(src, &vars).unwrap();
        let text = map.text();
        assert_eq!(text, "  key = 日本語  \nnext=1");
        assert_eq!(map.locate(text.find("日本語").unwrap()).to_string(), "input.ini:2:9");
        assert_eq!(map.locate(text.find("next").unwrap()).to_string(), "input.ini:3:1");
    }

    #[test]
    fn retain_variable_state() {
        let vars = IndexMap::from([("VALUE".to_owned(), "original".to_owned())]);
        let src = ";#define VALUE changed\nfirst=${VALUE}\n;#undef VALUE\n;#ifndef VALUE\nsecond=1\n;#endif\n";
        assert_eq!(preprocess(src, &vars).unwrap().text(), "first=changed\nsecond=1\n");
        assert_eq!(vars["VALUE"], "original");
        assert!(preprocess(";#define BUILD_DIR changed", &vars).is_err());
    }

    #[test]
    fn skips_comments_and_normalizes_newlines() {
        let vars = IndexMap::from([
            ("SEC".to_owned(), "main".to_owned()),
            ("VAL".to_owned(), "a\r\nb\rc".to_owned()),
        ]);
        let src = "; ${MISSING}\n[${SEC}] ; ${MISSING}\n${VAL}\n";
        assert_eq!(
            preprocess(src, &vars).unwrap().text(),
            "; ${MISSING}\n[main] ; ${MISSING}\na\nb\nc\n"
        );
        for err_src in [
            "[${VAL}]\n",
            "${VAL}=1\n",
            "key=${VAL}\n",
            "key=x;${VAL}\n",
            ";#replace VAL\nkey=0\n",
            ";#replace SEC\n${VAL}=0\n",
        ] {
            assert!(preprocess(err_src, &vars).is_err(), "{err_src}");
        }
    }
}
