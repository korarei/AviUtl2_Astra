use super::preprocess::{self, Action, Conditions, Position};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

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
        output: String::with_capacity(src.len()),
    };

    builder.process(src, Path::new(target.path()))?;
    Ok(builder.output)
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    output: String,
}

impl Builder<'_> {
    fn process(&mut self, content: &str, file: &Path) -> anyhow::Result<()> {
        let curr_dir = file.parent().unwrap_or(Path::new("."));
        let file: Arc<Path> = file.into();
        let mut conditions = Conditions::default();

        for (i, line) in content.split_inclusive('\n').enumerate() {
            let is_newline = line.ends_with('\n');
            let line = line.strip_suffix('\n').unwrap_or(line);
            let comment = line
                .trim_start()
                .strip_prefix(';')
                .filter(|comment| comment.starts_with('#'))
                .map(|comment| (comment, false, false));

            let locate = |at| Position {
                file: Arc::clone(&file),
                line: i + 1,
                col: line[..at].chars().count() + 1,
            };
            let locate_in = |text: &str, at| locate(text.as_ptr() as usize - line.as_ptr() as usize + at);
            let pos = locate(line.len() - line.trim_start().len() + usize::from(comment.is_some()));

            match preprocess::resolve(comment, &mut conditions, self.vars.as_ref(), &pos)? {
                Action::Keep => {
                    self.output.push_str(&Self::expand(line, self.vars.as_ref(), locate)?);
                    if is_newline {
                        self.output.push('\n');
                    }
                }
                Action::Drop => {}
                Action::PragmaOnce => {
                    self.once_files
                        .insert(std::fs::canonicalize(file.as_ref()).unwrap_or_else(|_| file.as_ref().to_path_buf()));
                }
                Action::Define(key, val) => {
                    if RESERVED_VARIABLES.contains(&key) {
                        bail!("{pos}: cannot define reserved variable '{key}'");
                    }

                    let vars = self.vars.to_mut();
                    vars.insert(
                        key.to_owned(),
                        Self::expand(val, vars, |at| locate_in(val, at))?.into_owned(),
                    );
                }
                Action::Undef(key) => {
                    self.vars.to_mut().shift_remove(key);
                }
                Action::Include(include, is_quoted) => {
                    let include = Self::expand(&include, self.vars.as_ref(), |at| match &include {
                        Cow::Borrowed(text) => locate_in(text, at),
                        Cow::Owned(_) => pos.clone(),
                    })?;
                    let file = is_quoted
                        .then(|| curr_dir.join(include.as_ref()))
                        .filter(|file| file.is_file())
                        .or_else(|| {
                            self.include_dirs
                                .iter()
                                .map(|dir| dir.join(include.as_ref()))
                                .find(|file| file.is_file())
                        })
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "{pos}: include file '{include}' not found from '{}'",
                                curr_dir.display()
                            )
                        })?;

                    self.load_include(&file, is_newline)?;
                }
            }
        }

        conditions.finish()
    }

    fn load_include(&mut self, file: &Path, is_newline: bool) -> anyhow::Result<()> {
        let file = std::fs::canonicalize(file)
            .with_context(|| format!("failed to canonicalize include file '{}'", file.display()))?;
        if self.once_files.contains(&file) {
            return Ok(());
        }

        if self.include_stack.contains(&file) {
            bail!("circular include detected: '{}'", file.display());
        }

        let st = self.output.len();
        self.include_stack.push(file.clone());
        let res = crate::fs::read_file(&file, self.target.encoding()).and_then(|src| self.process(&src, &file));
        let _ = self.include_stack.pop();
        res?;

        if is_newline && self.output.len() > st && !self.output.ends_with('\n') {
            self.output.push('\n');
        }

        Ok(())
    }

    fn expand<'text>(
        text: &'text str,
        vars: &impl crate::vars::Vars,
        locate: impl Fn(usize) -> Position,
    ) -> anyhow::Result<Cow<'text, str>> {
        if !text.contains('$') {
            return Ok(Cow::Borrowed(text));
        }

        let mut output = None;
        let mut curr = 0;
        let mut scan = 0;

        while let Some(rel) = text[scan..].find('$') {
            let st = scan + rel;
            if let Some((key, ed)) =
                preprocess::placeholder(text, st).map_err(|err| anyhow::anyhow!("{}: {err}", locate(st)))?
            {
                let val = vars
                    .get(key)
                    .ok_or_else(|| anyhow::anyhow!("{}: variable '{key}' not found", locate(st)))?;
                let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                output.push_str(&text[curr..st]);
                output.push_str(val);
                curr = ed;
                scan = ed;
            } else {
                scan = st + 1;
            }
        }

        if let Some(mut output) = output {
            output.push_str(&text[curr..]);
            Ok(Cow::Owned(output))
        } else {
            Ok(Cow::Borrowed(text))
        }
    }
}
