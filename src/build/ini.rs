use super::preprocess::{self, Action, Parser};
use super::source::{self, Position};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    map: source::Map,
}

impl Builder<'_> {
    #[allow(clippy::too_many_lines)]
    fn process(&mut self, src: &str, file: &Path) -> anyhow::Result<()> {
        let map = source::Map::new(
            src,
            Position {
                file: file.into(),
                line: 1,
                col: 1,
            },
        );
        let mut parser = Parser::new(preprocess::Lexer::new(src));
        let mut omit = None;
        let mut repl = None;

        let mut is_skip = false;

        for line in src.split_inclusive('\n') {
            let is_newline = line.ends_with('\n');
            let line = line.strip_suffix('\n').unwrap_or(line);
            let directive = line
                .trim_start()
                .strip_prefix(';')
                .filter(|content| content.starts_with('#'));
            let is_section = line
                .trim_start()
                .strip_prefix('[')
                .and_then(|line| line.split_once(']'))
                .is_some_and(|(_, rest)| rest.trim().is_empty() || rest.trim_start().starts_with(';'));

            if is_skip {
                if !is_section {
                    continue;
                }

                is_skip = false;
            }

            let st = line.as_ptr() as usize - src.as_ptr() as usize;
            let locate = |at| map.locate(st + at);

            let pos = locate(line.len() - line.trim_start().len() + usize::from(directive.is_some()));

            if let Some(pos) = &omit {
                if directive.is_some() {
                    bail!("{pos}: directive between #omit and its line");
                }

                if !line.trim().is_empty() && !line.trim_start().starts_with(';') {
                    is_skip = is_section;
                    omit = None;
                }

                continue;
            }

            if repl.is_some() && directive.is_some() {
                bail!("{pos}: directive between #replace and its entry");
            }

            let action = parser.process(
                directive.map(|content| (content, false)),
                self.vars.as_ref(),
                &pos,
                self.include_dirs,
            )?;

            match action {
                Action::Keep => {
                    if repl.is_some() && !line.trim().is_empty() && !line.trim_start().starts_with(';') {
                        let (val, pos) = repl.take().unwrap();
                        let Some((key, rest)) = line.split_once('=') else {
                            bail!("{pos}: #replace must precede an INI entry with a value");
                        };

                        if is_section || key.trim().is_empty() || rest.trim().is_empty() {
                            bail!("{pos}: #replace must precede an INI entry with a value");
                        }

                        let val_st = key.len() + 1 + rest.len() - rest.trim_start().len();
                        let val_ed = key.len() + 1 + rest.trim_end().len();
                        let prefix = preprocess::expand(&line[..val_st], self.vars.as_ref())
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;

                        prefix.append(&mut self.map, &map, st..st + val_st);
                        self.map.generate(val, &map, st + val_st..st + val_ed);
                        self.map.append(&map, st + val_ed..st + line.len());

                        if is_newline {
                            self.map.append(&map, st + line.len()..st + line.len() + 1);
                        }

                        continue;
                    }

                    let expanded = preprocess::expand(line, self.vars.as_ref())
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    let ed = st + line.len();
                    expanded.append(&mut self.map, &map, st..ed);

                    if is_newline {
                        self.map.append(&map, ed..ed + 1);
                    }
                }
                Action::Omit => omit = Some(pos),
                Action::Replace(val) => {
                    if val.contains('\n') {
                        bail!("{pos}: INI #replace value cannot contain a newline");
                    }

                    repl = Some((val, pos));
                }
                Action::Drop => {}
                Action::PragmaOnce => {
                    self.once_files
                        .insert(std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf()));
                }
                Action::Define(key, val) => {
                    if RESERVED_VARIABLES.contains(&key) {
                        bail!("{pos}: cannot define reserved variable '{key}'");
                    }

                    let vars = self.vars.to_mut();
                    vars.insert(
                        key.to_owned(),
                        preprocess::expand(val, vars)
                            .map_err(|err| {
                                anyhow::anyhow!(
                                    "{}: {err}",
                                    locate(val.as_ptr() as usize - line.as_ptr() as usize + err.offset)
                                )
                            })?
                            .text
                            .into_owned(),
                    );
                }
                Action::Undef(key) => {
                    self.vars.to_mut().shift_remove(key);
                }
                Action::Include(file) => self.include(&file, is_newline, &pos)?,
            }
        }

        if let Some(pos) = omit {
            bail!("{pos}: missing line after #omit");
        }

        if let Some((_, pos)) = repl {
            bail!("{pos}: missing entry after #replace");
        }

        parser.finish()
    }

    fn include(&mut self, file: &Path, is_newline: bool, pos: &Position) -> anyhow::Result<()> {
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
            .and_then(|src| self.process(&src, file));
        let _ = self.include_stack.pop();

        result?;

        if is_newline && self.map.len() > st && !self.map.ends_with('\n') {
            self.map
                .generate("\n", &self.map.slice(self.map.len()..self.map.len()), 0..0);
        }

        Ok(())
    }
}

pub fn build(
    src: &str,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
) -> anyhow::Result<String> {
    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        include_stack: std::fs::canonicalize(target.path()).into_iter().collect(),
        once_files: HashSet::new(),
        map: source::Map::default(),
    };

    builder.process(src, Path::new(target.path()))?;
    Ok(builder.map.text().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(src: &str, vars: &IndexMap<String, String>) -> anyhow::Result<source::Map> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.ini" }))?;
        let mut builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Borrowed(vars),
            include_stack: Vec::new(),
            once_files: HashSet::new(),
            map: source::Map::default(),
        };
        builder.process(src, Path::new("input.ini"))?;
        Ok(builder.map)
    }

    #[test]
    fn preserve_lines() {
        for src in ["", "key=1", "key=1\n", "; comment\n\n[section]\n key = 1 "] {
            let map = process(src, &IndexMap::new()).unwrap();
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
            assert_eq!(process(src, &IndexMap::new()).unwrap().text(), expected, "{src}");
        }

        for src in [";#omit", ";#omit\n; annotation\n", ";#omit\n;#define VALUE 1\nkey=0"] {
            assert!(process(src, &IndexMap::new()).is_err(), "{src}");
        }
    }

    #[test]
    fn preserve_replacement_positions() {
        let vars = IndexMap::from([("VALUE".to_owned(), "日本語".to_owned())]);
        let src = ";#replace VALUE\n  key = 0  \nnext=1";
        let map = process(src, &vars).unwrap();
        let text = map.text();
        assert_eq!(text, "  key = 日本語  \nnext=1");
        assert_eq!(map.locate(text.find("日本語").unwrap()).to_string(), "input.ini:2:9");
        assert_eq!(map.locate(text.find("next").unwrap()).to_string(), "input.ini:3:1");
    }

    #[test]
    fn retain_variable_state() {
        let vars = IndexMap::from([("VALUE".to_owned(), "original".to_owned())]);
        let src = ";#define VALUE changed\nfirst=${VALUE}\n;#undef VALUE\n;#ifndef VALUE\nsecond=1\n;#endif\n";
        assert_eq!(process(src, &vars).unwrap().text(), "first=changed\nsecond=1\n");
        assert_eq!(vars["VALUE"], "original");
        assert!(process(";#define BUILD_DIR changed", &vars).is_err());
    }
}
