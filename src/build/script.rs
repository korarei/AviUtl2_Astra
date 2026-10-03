use super::hlsl;
use super::lua::{self, TokenKind};
use super::preprocess;
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use regex::Regex;
use std::borrow::Cow;
use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

pub fn build(
    src: &str,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    is_bundled: bool,
    suffix: &str,
) -> anyhow::Result<Output> {
    Builder::new(target, include_dirs, vars, is_bundled, suffix).build(src)
}

#[derive(Debug, Default)]
pub struct Output {
    pub script: String,
    pub l10n: String,
}

impl Output {
    pub fn push_str(&mut self, output: &Self) {
        self.script.push_str(&output.script);
        self.l10n.push_str(&output.l10n);
    }

    pub fn replace(&mut self, from: &str, to: &str) {
        self.script = self.script.replace(from, to);
        self.l10n = self.l10n.replace(from, to);
    }
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    is_bundled: bool,
    suffix: &'a str,
    include_stack: Vec<PathBuf>,
}

struct Prop<'a> {
    pos: &'a preprocess::Position,
    kind: &'a str,
    var: Option<&'a str>,
    key: &'a str,
    rest: Option<&'a str>,
}

struct Source {
    text: String,
    positions: Vec<preprocess::Position>,
}

impl<'a> Builder<'a> {
    fn build(mut self, src: &str) -> anyhow::Result<Output> {
        let src = self.process(src, Path::new(self.target.path()))?;
        let mut src = self.resolve_header(src)?;

        if self.suffix.ends_with('2') && !self.suffix.eq_ignore_ascii_case(".tra2") {
            src = Self::normalize_props(src)?;
        }

        let props = Self::extract_props(&src)?;
        let l10n = if self.suffix.eq_ignore_ascii_case(".tra2") {
            Self::validate_tra_props(&props)?;
            Self::validate_tra2_props(&props)?;
            self.collect_tra2_props(&props)?
        } else if self.suffix.ends_with('2') {
            Self::validate_legacy_props(&props)?;
            Self::validate_modern_props(&props)?;
            Self::validate_shaders(&src)?;
            self.collect_props(&props)?
        } else {
            if self.suffix.eq_ignore_ascii_case(".tra") {
                Self::validate_tra_props(&props)?;
            } else {
                Self::validate_legacy_props(&props)?;
            }
            String::new()
        };

        Ok(Output { l10n, script: src.text })
    }

    fn new(
        target: &'a BuildTarget,
        include_dirs: &'a [PathBuf],
        vars: &'a IndexMap<String, String>,
        is_bundled: bool,
        suffix: &'a str,
    ) -> Self {
        let mut include_stack = Vec::new();
        if let Ok(file) = std::fs::canonicalize(target.path()) {
            include_stack.push(file);
        }

        Self {
            target,
            include_dirs,
            vars: Cow::Borrowed(vars),
            is_bundled,
            suffix,
            include_stack,
        }
    }

    fn process(&mut self, content: &str, file: &Path) -> anyhow::Result<Source> {
        self.process_at(content, file, 0, 0)
    }

    #[allow(clippy::too_many_lines)]
    fn process_at(
        &mut self,
        content: &str,
        file: &Path,
        line_offset: usize,
        col_offset: usize,
    ) -> anyhow::Result<Source> {
        let curr_dir = file.parent().unwrap_or(Path::new("."));
        let file: Arc<Path> = file.into();
        let starts = std::iter::once(0)
            .chain(content.match_indices('\n').map(|(st, _)| st + 1))
            .collect::<Vec<_>>();
        let mut output = String::with_capacity(content.len());
        let mut positions = Vec::new();
        let mut includes = Vec::new();

        let locate = |at| {
            let i = starts.partition_point(|&st| st <= at).saturating_sub(1);
            preprocess::Position {
                file: Arc::clone(&file),
                line: line_offset + i + 1,
                col: content[starts[i]..at].chars().count() + 1 + if i == 0 { col_offset } else { 0 },
            }
        };

        let mut has_output = false;
        let mut lexer = lua::Lexer::new(content).peekable();
        let mut blocked_ed = 0;
        let mut conditions = preprocess::Conditions::default();
        let mut i = 0;

        while starts.get(i).is_some_and(|&st| st < content.len()) {
            let st = starts[i];
            let next_st = starts.get(i + 1).copied();
            let ed = next_st.map_or(content.len(), |st| st - 1);
            let line = &content[st..ed];
            let mut col = line.len() - line.trim_start().len();

            let is_blocked = blocked_ed > st;
            let mut first = None;
            while let Some(token) = lexer.next_if(|token| token.span.st < ed) {
                if token.span.st >= st
                    && matches!(&token.kind, TokenKind::Comment { is_block: false, .. })
                    && !line.trim_start().starts_with('@')
                {
                    col = token.span.st - st;
                }
                if token.span.st >= st && token.span.ed > ed {
                    blocked_ed = blocked_ed.max(token.span.ed);
                }

                if first.is_none()
                    && token.span.st >= st
                    && !matches!(&token.kind, TokenKind::Whitespace | TokenKind::Newline)
                {
                    first = Some(token);
                }
            }

            let is_block = first.as_ref().is_some_and(|token| {
                matches!(
                    &token.kind,
                    TokenKind::Comment { is_block: true, .. } | TokenKind::UnclosedComment(_)
                )
            });

            let comment = if is_blocked {
                None
            } else {
                first.as_ref().and_then(|token| match &token.kind {
                    TokenKind::Comment { content, is_block } if content.starts_with('#') => {
                        Some((*content, *is_block, false))
                    }
                    TokenKind::UnclosedComment(content) if content.starts_with('#') => Some((*content, true, true)),
                    _ => None,
                })
            };

            let pos = locate(
                st + if comment.is_some() {
                    line.find('#').unwrap_or(col)
                } else {
                    col
                },
            );

            let action = preprocess::resolve(comment, &mut conditions, self.vars.as_ref(), &pos)?;
            let next_line = |i| {
                first
                    .as_ref()
                    .map_or(i + 1, |token| starts.partition_point(|&st| st < token.span.ed))
            };

            if matches!(&action, preprocess::Action::Drop | preprocess::Action::PragmaOnce) {
                i = if is_block { next_line(i) } else { i + 1 };
                continue;
            }

            if let Some(token) = first.as_ref()
                && let TokenKind::Comment {
                    content: body,
                    is_block: true,
                } = &token.kind
                && (body.starts_with("pixelshader@") || body.starts_with("computeshader@"))
            {
                let body_st = body.as_ptr() as usize - content.as_ptr() as usize;
                let body_ed = body_st + body.len();
                let origin = locate(body_st);
                let mut nested = self.process_at(body, file.as_ref(), origin.line - 1, origin.col - 1)?;
                let end_line = starts.partition_point(|&st| st <= token.span.ed).saturating_sub(1);
                let end = starts.get(end_line + 1).map_or(content.len(), |st| st - 1);

                if body.ends_with('\n') && !nested.text.ends_with('\n') {
                    nested.text.push('\n');
                    nested.positions.push(locate(body_ed));
                }

                let suffix = Self::expand(&content[body_ed..end], self.vars.as_ref(), |at| locate(body_ed + at))?;
                if has_output {
                    output.push('\n');
                }

                output.push_str(&content[st..body_st]);
                output.push_str(&nested.text);
                output.push_str(suffix.as_ref());
                positions.extend(nested.positions);
                positions.extend(std::iter::repeat_n(locate(body_ed), suffix.matches('\n').count()));

                has_output = true;
                i = end_line + 1;
                continue;
            }

            if let preprocess::Action::Define(key, val) = &action {
                if RESERVED_VARIABLES.contains(key) {
                    bail!("{pos}: cannot define reserved variable '{key}'");
                }

                let vars = self.vars.to_mut();
                vars.insert(
                    (*key).to_owned(),
                    Self::expand(val, vars, |at| {
                        locate(val.as_ptr() as usize - content.as_ptr() as usize + at)
                    })?
                    .into_owned(),
                );
                i = if is_block { next_line(i) } else { i + 1 };
                continue;
            }

            if let preprocess::Action::Undef(key) = &action {
                if self.vars.contains_key(*key) {
                    self.vars.to_mut().shift_remove(*key);
                }

                i = if is_block { next_line(i) } else { i + 1 };
                continue;
            }

            if let preprocess::Action::Include(include, is_quoted) = &action {
                let include = Self::expand(include.as_ref(), self.vars.as_ref(), |at| match include {
                    Cow::Borrowed(text) => locate(text.as_ptr() as usize - content.as_ptr() as usize + at),
                    Cow::Owned(_) => pos.clone(),
                })?;
                let indent = first.as_ref().map_or("", |token| &line[..token.span.st - st]);
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

                let Source {
                    text: nested,
                    positions: mut locations,
                } = self.load_include(&file)?;

                if locations.is_empty() {
                    locations.push(pos.clone());
                }

                if has_output {
                    output.push('\n');
                }
                let st = output.len();
                output.push_str(&textwrap::indent(&nested, indent));
                if file
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
                {
                    includes.push((st, output.len(), positions.len(), file, include.into_owned(), nested));
                }
                positions.extend(locations);

                has_output = true;
                i += 1;
                continue;
            }

            if has_output {
                output.push('\n');
            }

            let line = Self::expand(line, self.vars.as_ref(), |at| locate(st + at))?;
            positions.extend(std::iter::repeat_n(pos, line.matches('\n').count() + 1));
            output.push_str(line.as_ref());
            has_output = true;
            i += 1;
        }

        conditions.finish()?;

        if includes.is_empty() {
            return Ok(Source {
                text: output,
                positions,
            });
        }

        let text = output;
        let locations = positions;
        let mut output = String::with_capacity(text.len());
        let mut positions = Vec::with_capacity(locations.len());
        let mut curr = 0;
        let mut line = 0;

        for (i, (st, ed, st_line, file, include, nested)) in includes.iter().enumerate() {
            let Some(found) = lua::find_require(&text[..includes.get(i + 1).map_or(text.len(), |next| next.0)], *ed)
            else {
                continue;
            };

            let ed_line = st_line + nested.matches('\n').count();
            let req_line = ed_line + text[*ed..found.st].matches('\n').count();
            let mut pos = locations[req_line].clone();
            let req_st = text[..found.st].rfind('\n').map_or(0, |st| st + 1);
            pos.col = text[req_st..found.st].chars().count() + 1;

            let name = std::str::from_utf8(&found.name).map_err(|_| {
                anyhow::anyhow!("{pos}: require module name is not valid UTF-8 after include '{include}'")
            })?;

            let stem = name.rsplit(['.', '/', '\\']).next().unwrap_or(name);
            let file_stem = file.file_stem().and_then(|s| s.to_str());
            if file_stem != Some(stem)
                && !(file_stem == Some("init")
                    && file.parent().and_then(|d| d.file_name()).and_then(|s| s.to_str()) == Some(stem))
            {
                bail!("{pos}: require module '{name}' does not match included file '{include}'");
            }

            let indent = &text[req_st
                ..req_st
                    + text[req_st..found.st]
                        .bytes()
                        .take_while(|&b| matches!(b, b' ' | b'\t'))
                        .count()];

            let prefix = &text[*ed..found.st];
            let is_newline = prefix.starts_with('\n');
            let prefix = prefix.strip_prefix('\n').unwrap_or(prefix);

            output.push_str(&text[curr..*st]);

            let _ = write!(
                output,
                "{prefix}(function()\n{}\n{indent}end)()",
                textwrap::indent(nested, &format!("{indent}    "))
            );

            positions.extend_from_slice(&locations[line..*st_line]);
            positions.extend_from_slice(&locations[ed_line + usize::from(is_newline)..=req_line]);
            positions.extend_from_slice(&locations[*st_line..=ed_line]);
            positions.push(pos);

            curr = found.ed;
            line = req_line + text[found.st..found.ed].matches('\n').count() + 1;
        }

        output.push_str(&text[curr..]);
        positions.extend_from_slice(&locations[line..]);

        Ok(Source {
            text: output,
            positions,
        })
    }

    fn resolve_header(&self, src: Source) -> anyhow::Result<Source> {
        static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?x)
                ^ -{2,} [^\S\n]* @ (?P<header>.*)
                ",
            )
            .unwrap()
        });

        let text = src.text.trim();
        if text.is_empty() {
            return Ok(Source {
                text: String::new(),
                positions: Vec::new(),
            });
        }

        let mut positions = src
            .text
            .lines()
            .zip(src.positions)
            .skip_while(|(line, _)| line.trim().is_empty())
            .take(text.lines().count())
            .map(|(_, pos)| pos)
            .collect::<Vec<_>>();

        let (first, tail) = match text.split_once('\n') {
            Some((first, tail)) => (first, Some(tail)),
            None => (text, None),
        };

        let (header, body) = if first.starts_with('@') {
            if !self.is_bundled {
                let pos = &positions[0];
                bail!("{pos}: single target cannot have '@' header");
            }
            (Some(first.trim_end().to_owned()), tail)
        } else if let Some(caps) = PATTERN.captures(first) {
            if self.is_bundled {
                (Some(format!("@{}", caps["header"].trim())), tail)
            } else {
                tracing::warn!(
                    "{}: single target has '--@' on the first line; leaving it as comment",
                    positions[0]
                );
                (Some(first.to_owned()), tail)
            }
        } else if self.is_bundled {
            let name = self.target.name();

            if name.is_empty() {
                bail!("failed to determine target name for target '{}'", self.target.path());
            }

            positions.insert(0, positions[0].clone());
            (Some(format!("@{name}")), Some(text))
        } else {
            (None, Some(text))
        };

        let mut result = header.unwrap_or_default();
        if let Some(body) = body {
            if !result.is_empty() {
                result.push('\n');
            }

            result.push_str(body);
        }

        if !result.is_empty() {
            result.push('\n');
            positions.push(positions.last().unwrap().clone());
        }

        Ok(Source {
            text: result,
            positions,
        })
    }

    #[allow(clippy::too_many_lines)]
    fn normalize_props(mut src: Source) -> anyhow::Result<Source> {
        static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?mx)
                ^ [^\S\n]* ;? [^\S\n]*
                (?: local\s+ )?
                (?P<name>
                    [a-zA-Z_]\w*
                    (?: \.[a-zA-Z_]\w* | \[[^\]\n]+\] )*
                )
                [^\S\n]*
                (?: = [^\S\n]* (?P<val>.*?[^\s-] ) [^\S\n]* )?
                --(?P<kind>[a-zA-Z_]\w*)@
                (?P<ws0>[^\S\n]*)
                (?P<var>[a-zA-Z_]\w*)
                (?P<ws1>[^\S\n]*)
                :
                (?P<ws2>[^\S\n]*)
                (?P<rest>[^\n]*)
                ",
            )
            .unwrap()
        });

        let text = &src.text;
        let comments = lua::Lexer::new(text)
            .filter_map(|token| {
                matches!(&token.kind, TokenKind::Comment { is_block: false, .. }).then_some(token.span.st)
            })
            .collect::<Vec<_>>();
        let mut err = None;
        let mut merges = Vec::new();

        let result = PATTERN.replace_all(text, |caps: &regex::Captures| {
            if err.is_some() {
                return String::new();
            }

            let m = caps.get(0).unwrap();
            if comments
                .binary_search(&(caps.name("kind").unwrap().start() - 2))
                .is_err()
            {
                return m.as_str().to_owned();
            }

            let pos = &src.positions[text[..caps.name("kind").unwrap().start()].matches('\n').count()];
            let st = text[..m.start()].matches('\n').count();
            let ed = text[..m.end()].matches('\n').count();
            if st != ed {
                merges.push((st, ed, pos.clone()));
            }

            let kind = caps["kind"].to_ascii_lowercase();
            if kind == "data" || kind == "hide" {
                err = Some(anyhow::anyhow!(
                    "{pos}: '{kind}' annotation cannot be attached to variable declaration: '{}'",
                    m.as_str().trim()
                ));
                return String::new();
            }

            let name = &caps["name"];
            if name.contains(['.', '[']) {
                err = Some(anyhow::anyhow!(
                    "{pos}: property annotation cannot be used on table field assignment: '{}'",
                    m.as_str().trim()
                ));
                return String::new();
            }

            let var = &caps["var"];
            if var != "_" && name != var {
                err = Some(anyhow::anyhow!(
                    "{pos}: variable name '{var}' does not match assignment target '{name}' in '{}'",
                    m.as_str().trim()
                ));
                return String::new();
            }

            let mut rest = caps["rest"].to_owned();
            if let Some(val) = caps.name("val").map(|m| m.as_str().trim()) {
                match assign_props(&kind, val, rest) {
                    Ok(value) => rest = value,
                    Err(value) => {
                        err = Some(anyhow::anyhow!("{pos}: {value} in '{}'", m.as_str().trim()));
                        return String::new();
                    }
                }
            }

            format!(
                "--{kind}@{}{}{}:{}{rest}",
                &caps["ws0"],
                if var == "_" { name } else { var },
                &caps["ws1"],
                &caps["ws2"]
            )
        });

        if let Some(err) = err {
            return Err(err);
        }

        src.text = result.into_owned();
        for (st, ed, pos) in merges.into_iter().rev() {
            drop(src.positions.splice(st..=ed, std::iter::once(pos)));
        }

        Ok(src)
    }

    fn extract_props(src: &Source) -> anyhow::Result<Vec<Prop<'_>>> {
        static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
                r"(?x)
                ^ -- [^\S\n]* (?P<kind>[a-zA-Z_]\w*)
                [^\S\n]* (?: @ [^\S\n]* (?P<var>[^:\n]*) )?
                [^\S\n]* : [^\S\n]*
                (?P<key>[^,\n]*)
                (?: , [^\S\n]* (?P<rest>[^\n]* ) )?
                ",
            )
            .unwrap()
        });

        let text = &src.text;
        let mut props = Vec::new();
        let mut i = 0;
        let mut curr = 0;

        for token in lua::Lexer::new(text) {
            if !matches!(&token.kind, TokenKind::Comment { is_block: false, .. }) {
                continue;
            }

            i += text[curr..token.span.st].matches('\n').count();
            curr = token.span.st;
            if !text[..curr].rsplit('\n').next().unwrap_or("").trim().is_empty() {
                continue;
            }

            let Some(caps) = PATTERN.captures(&text[token.span.st..token.span.ed]) else {
                continue;
            };

            if caps.get(0).unwrap().as_str().chars().any(char::is_control) {
                bail!("{}: annotation cannot contain control characters", src.positions[i]);
            }

            props.push(Prop {
                pos: &src.positions[i],
                kind: caps.name("kind").unwrap().as_str(),
                var: caps.name("var").map(|m| m.as_str().trim()),
                key: caps.name("key").unwrap().as_str(),
                rest: caps.name("rest").map(|m| m.as_str()),
            });
        }

        Ok(props)
    }

    fn validate_legacy_props(props: &[Prop<'_>]) -> anyhow::Result<()> {
        for prop in props {
            if prop.var.is_some() {
                continue;
            }

            match prop.kind.to_ascii_lowercase().as_str() {
                "track0" | "track1" | "track2" | "track3" => validate_track(prop)?,
                "check0" => {
                    if !matches!(prop.rest.map(str::trim), Some("0" | "1")) {
                        bail!(
                            "{}: argument of 'check0' must be '0' or '1', got '{}'",
                            prop.pos,
                            prop.rest.unwrap_or("").trim()
                        );
                    }
                }
                "color" => {
                    if prop.rest.is_some_and(|rest| !rest.trim().is_empty()) {
                        bail!(
                            "{}: argument of 'color' must be empty, got '{}'",
                            prop.pos,
                            prop.rest.unwrap_or("").trim()
                        );
                    }

                    if prop.key.trim() != "nil"
                        && !parse_int(prop.key.trim()).is_some_and(|val| (0..=0xff_ffff).contains(&val))
                    {
                        bail!(
                            "{}: argument of 'color' must be 'nil' or an integer \
                             between 0x000000 and 0xffffff, got '{}'",
                            prop.pos,
                            prop.key.trim()
                        );
                    }
                }
                "file" => {
                    if prop.rest.is_some_and(|rest| !rest.trim().is_empty()) {
                        bail!(
                            "{}: argument of 'file' must be empty, got '{}'",
                            prop.pos,
                            prop.rest.unwrap_or("").trim()
                        );
                    }
                }
                "param" => {
                    if prop.rest.is_some_and(|rest| !rest.trim().is_empty()) {
                        bail!(
                            "{}: argument of 'param' must be empty, got '{}'",
                            prop.pos,
                            prop.rest.unwrap_or("").trim()
                        );
                    }

                    for item in prop.key.split(';') {
                        let Some((var, _)) = item.split_once('=') else {
                            bail!("{}: item of 'param' must contain '=', got '{item}'", prop.pos);
                        };

                        let var = var.trim();
                        if !is_var(var) {
                            bail!(
                                "{}: variable name of 'param' must be a valid Lua identifier, got '{var}'",
                                prop.pos
                            );
                        }
                    }
                }
                "dialog" => {
                    for item in prop.rest.unwrap_or("").split(';') {
                        let item = item.split_once(',').map_or(item, |(_, item)| item);
                        let Some((var, _)) = item.split_once('=') else {
                            bail!("{}: item of 'dialog' must contain '=', got '{item}'", prop.pos);
                        };

                        let var = var.trim();
                        if !is_var(var) {
                            bail!(
                                "{}: variable name of 'dialog' must be a valid Lua identifier, got '{var}'",
                                prop.pos
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_lines)]
    fn validate_modern_props(props: &[Prop<'_>]) -> anyhow::Result<()> {
        for prop in props {
            let kind = prop.kind.to_ascii_lowercase();
            let pos = prop.pos;

            let Some(var) = prop.var else {
                match kind.as_str() {
                    "track0" | "track1" | "track2" | "track3" | "check0" | "color" | "file" | "param" | "dialog" => {
                        tracing::warn!("{pos}: '{kind}' uses legacy syntax");
                    }
                    "group" => {
                        if let Some(rest) = prop.rest.map(str::trim)
                            && !rest.is_empty()
                        {
                            let args = rest.split(',').map(str::trim).collect::<Vec<_>>();
                            if !matches!(args[0], "true" | "false") {
                                bail!(
                                    "{pos}: argument 1 of '{kind}' must be 'true' or 'false', got '{}'",
                                    args[0]
                                );
                            }

                            if args.len() >= 2 {
                                tracing::warn!(
                                    "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                                    args.len()
                                );
                            }
                        }
                    }
                    "script" => {
                        if let Some(rest) = prop.rest.map(str::trim)
                            && !rest.is_empty()
                        {
                            tracing::warn!("{pos}: '{kind}' expects 0 arguments, got {}", rest.split(',').count());
                        }

                        match prop.key.trim().to_ascii_lowercase().as_str() {
                            "luajit" => {}
                            "lua" => tracing::warn!("{pos}: argument of '{kind}' is 'Lua'"),
                            _ => {
                                bail!(
                                    "{pos}: argument of '{kind}' must be 'LuaJIT' or 'Lua', got '{}'",
                                    prop.key.trim()
                                );
                            }
                        }
                    }
                    _ => {}
                }
                continue;
            };

            let validate = || -> anyhow::Result<()> {
                if !is_var(var) {
                    bail!("{pos}: variable name of '{kind}' must be a valid Lua identifier, got '{var}'");
                }

                Ok(())
            };

            if kind == "track" {
                validate()?;
                validate_track(prop)?;
                continue;
            }

            let rest = prop.rest.map(str::trim);
            let args = || rest.map_or(Vec::new(), |r| r.split(',').map(str::trim).collect::<Vec<_>>());

            match kind.as_str() {
                "check" => {
                    validate()?;

                    let args = args();
                    if args.is_empty() {
                        bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                    }

                    if args.len() > 1 {
                        tracing::warn!(
                            "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                            args.len()
                        );
                    }

                    if !matches!(args[0], "true" | "false" | "0" | "1") {
                        bail!(
                            "{pos}: argument 1 of '{kind}' must be 'true', 'false', '0', or '1', got '{}'",
                            args[0]
                        );
                    }
                }
                "checksection" => {
                    validate()?;

                    let args = args();
                    if args.is_empty() {
                        bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                    }

                    if args.len() > 2 {
                        tracing::warn!(
                            "{pos}: '{kind}' has too many arguments; expected at most 2, got {}",
                            args.len()
                        );
                    }

                    if !matches!(args[0], "true" | "false") {
                        bail!(
                            "{pos}: argument 1 of 'checksection' must be 'true' or 'false', got '{}'",
                            args[0]
                        );
                    }

                    if let Some(&arg) = args.get(1)
                        && !matches!(arg, "true" | "false")
                    {
                        bail!("{pos}: argument 2 of 'checksection' must be 'true' or 'false', got '{arg}'");
                    }
                }
                "select" => {
                    validate()?;

                    let mut default = 0;
                    if let Some((_, val)) = prop.key.trim().split_once('=') {
                        let val = val.trim();
                        default = val.parse::<i64>().map_err(|_| {
                            anyhow::anyhow!("{pos}: default value of '{kind}' must be an integer, got '{val}'")
                        })?;
                    }

                    let rest = rest.unwrap_or("");
                    if rest.is_empty() {
                        bail!("{pos}: '{kind}' requires at least 1 item, got 0");
                    }

                    validate_select(rest.split(',').collect(), &default.to_string())
                        .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
                }
                "color" => {
                    validate()?;

                    let args = args();
                    if args.is_empty() {
                        bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                    }

                    if args.len() > 1 {
                        tracing::warn!(
                            "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                            args.len()
                        );
                    }

                    if args[0] != "nil" && !parse_int(args[0]).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                        bail!(
                            "{pos}: argument 1 of '{kind}' must be 'nil' or an integer between \
                             0x000000 and 0xffffff, got '{}'",
                            args[0]
                        );
                    }
                }
                "value" => {
                    validate()?;

                    if rest.is_none_or(str::is_empty) {
                        bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                    }
                }
                "file" | "folder" => {
                    validate()?;

                    if let Some(rest) = rest
                        && !rest.is_empty()
                    {
                        tracing::warn!("{pos}: '{kind}' expects 0 arguments, got {}", rest.split(',').count());
                    }
                }
                "font" | "figure" | "string" | "text" => {
                    validate()?;

                    let args = args();
                    if args.len() != 1 {
                        tracing::warn!("{pos}: '{kind}' expects 1 argument, got {}", args.len());
                    }
                }
                "data" => {
                    let key = prop.key.trim();
                    if !key.parse::<i64>().is_ok_and(|val| (0..=16_000).contains(&val)) {
                        bail!("{pos}: argument of '{kind}' must be an integer between 0 and 16000, got '{key}'");
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    #[allow(clippy::match_same_arms)]
    fn validate_tra_props(props: &[Prop<'_>]) -> anyhow::Result<()> {
        for prop in props {
            if prop.var.is_some() {
                continue;
            }

            match prop.kind.to_ascii_lowercase().as_str() {
                "param" if prop.rest.is_none() => {
                    if prop.key.trim().parse::<f64>().is_err() {
                        bail!(
                            "{}: argument of 'param' must be a number, got '{}'",
                            prop.pos,
                            prop.key.trim()
                        );
                    }
                }
                "speed" => {
                    let pos = prop.pos;
                    if !matches!(prop.key.trim(), "0" | "1") {
                        bail!(
                            "{pos}: argument 1 of 'speed' must be '0' or '1', got '{}'",
                            prop.key.trim()
                        );
                    }

                    let Some(rest) = prop.rest else {
                        continue;
                    };

                    let args = rest.split(',').map(str::trim).collect::<Vec<_>>();
                    if args.len() > 1 {
                        tracing::warn!(
                            "{pos}: 'speed' has too many arguments; expected at most 2, got {}",
                            args.len() + 1
                        );
                    }

                    if !matches!(args[0], "0" | "1") {
                        bail!("{pos}: argument 2 of 'speed' must be '0' or '1', got '{}'", args[0]);
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_tra2_props(props: &[Prop<'_>]) -> anyhow::Result<()> {
        for prop in props {
            if prop.var.is_some() || !prop.kind.eq_ignore_ascii_case("param") {
                continue;
            }

            let Some(rest) = prop.rest.map(str::trim) else {
                continue;
            };

            let default = rest.split(',').map(str::trim).collect::<Vec<_>>();
            let pos = prop.pos;

            if default.len() > 1 {
                tracing::warn!(
                    "{pos}: 'param' has too many default values; expected at most 1, got {}",
                    default.len()
                );
            }

            let default = default[0];
            let key = prop.key.trim().split('/').collect::<Vec<_>>();
            match key.as_slice() {
                [_] => {
                    if default.parse::<f64>().is_err() {
                        bail!("{pos}: default value of 'param' must be a number, got '{default}'");
                    }
                }
                [_, "check"] => {
                    if !matches!(default, "0" | "1") {
                        bail!("{pos}: default value of 'check' must be '0' or '1', got '{default}'");
                    }
                }
                [_, "select", items @ ..] if !items.is_empty() => {
                    validate_select(items.to_vec(), default).map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_shaders(src: &Source) -> anyhow::Result<()> {
        let lines = std::iter::once(0)
            .chain(src.text.match_indices('\n').map(|(st, _)| st + 1))
            .collect::<Vec<_>>();

        let locate = |at| {
            let line = lines.partition_point(|&st| st <= at).saturating_sub(1);
            let mut pos = src.positions[line].clone();
            pos.col = src.text[lines[line]..at].chars().count() + 1;
            pos
        };

        let mut shaders = HashSet::new();
        let mut tokens = Vec::new();

        for token in lua::Lexer::new(&src.text) {
            match &token.kind {
                TokenKind::Comment {
                    content,
                    is_block: true,
                } => {
                    let Some((kind, name, body)) = content
                        .split_once('@')
                        .filter(|(kind, _)| matches!(*kind, "pixelshader" | "computeshader"))
                        .and_then(|(kind, body)| body.split_once(':').map(|(name, body)| (kind, name, body)))
                    else {
                        continue;
                    };

                    let pos = locate(token.span.st);
                    if name.chars().any(char::is_control) {
                        bail!("{pos}: name of '{kind}' cannot contain control characters");
                    }

                    if name.contains('@') {
                        bail!("{pos}: name of '{kind}' cannot contain '@', got '{name}'");
                    }

                    validate_shader_name(name, kind, &pos)?;

                    if !shaders.insert((kind, name.as_bytes())) {
                        bail!("{pos}: duplicate {kind} definition '{name}'");
                    }
                    if !has_shader_entry_point(body, name, kind) {
                        bail!("{pos}: entry point '{name}' is not defined in '{kind}' block");
                    }
                }
                TokenKind::Whitespace
                | TokenKind::Newline
                | TokenKind::Comment { .. }
                | TokenKind::UnclosedComment(_) => {}
                _ => tokens.push(token),
            }
        }

        for (i, token) in tokens.iter().enumerate() {
            let TokenKind::Ident(kind @ ("pixelshader" | "computeshader")) = &token.kind else {
                continue;
            };

            Self::validate_shader_call(&tokens, i, kind, &shaders, &locate(token.span.st))?;
        }

        Ok(())
    }

    fn validate_shader_call(
        tokens: &[lua::Token<'_>],
        i: usize,
        kind: &str,
        shaders: &HashSet<(&str, &[u8])>,
        pos: &preprocess::Position,
    ) -> anyhow::Result<()> {
        let token = |offset| {
            i.checked_add_signed(offset)
                .and_then(|i| tokens.get(i))
                .map(|token| &token.kind)
        };

        if matches!(token(-1), Some(TokenKind::Colon | TokenKind::Keyword("function")))
            || (matches!(token(-1), Some(TokenKind::Dot))
                && (!matches!(token(-2), Some(TokenKind::Ident("obj")))
                    || matches!(
                        token(-3),
                        Some(TokenKind::Dot | TokenKind::Colon | TokenKind::Keyword("function"))
                    )))
        {
            return Ok(());
        }

        let name = match (token(1), token(2), token(3)) {
            (Some(TokenKind::String(name)), _, _)
            | (Some(TokenKind::LParen), Some(TokenKind::String(name)), Some(TokenKind::Comma | TokenKind::RParen)) => {
                name
            }
            (Some(TokenKind::LParen | TokenKind::LBrace), _, _) => {
                bail!("{pos}: first argument of '{kind}' must be a string literal");
            }
            _ => return Ok(()),
        };

        if !name.contains(&b'@') && !shaders.contains(&(kind, name.as_ref())) {
            bail!("{pos}: undefined {kind} '{}'", String::from_utf8_lossy(name));
        }

        Ok(())
    }

    fn collect_props(&self, props: &[Prop<'_>]) -> anyhow::Result<String> {
        let mut seen = HashSet::new();
        let mut tips = IndexMap::new();
        let mut display = BTreeMap::new();

        for prop in props {
            let kind = prop.kind.to_ascii_lowercase();
            let pos = prop.pos;

            let mut check = |key| -> anyhow::Result<()> {
                validate_property_name(key, pos)?;

                if !seen.insert(key) {
                    bail!("{pos}: duplicate property name '{key}'");
                }
                Ok(())
            };

            let mut key = prop.key.trim();
            match (prop.var, kind.as_str()) {
                (None, "group" | "separator") => {
                    if kind == "group" {
                        check(key)?;
                    }

                    display.insert(key.to_owned(), String::new());
                    continue;
                }
                (Some(var), "data") => {
                    check(var)?;
                    continue;
                }
                (Some(_), "trackgroup") => {
                    check(key)?;
                    continue;
                }
                (
                    Some(_),
                    "track" | "check" | "checksection" | "color" | "file" | "folder" | "font" | "figure" | "select"
                    | "string" | "text" | "value",
                ) => {}
                _ => continue,
            }

            if kind == "select" {
                if let Some((name, val)) = key.split_once('=')
                    && key.matches('=').count() == 1
                    && val.trim().parse::<i64>().is_ok()
                {
                    key = name.trim_end();
                }

                let mut names = HashSet::new();
                for item in prop.rest.unwrap_or("").split(',') {
                    if let Some((name, _)) = item.split_once('=') {
                        let name = name.trim();
                        if !names.insert(name) {
                            bail!("{pos}: duplicate item '{name}' in 'select'");
                        }
                        display.insert(name.to_owned(), String::new());
                    }
                }
            }

            check(key)?;
            tips.insert(key.to_owned(), String::new());
            display.insert(key.rsplit("::").next().unwrap_or(key).to_owned(), String::new());
        }

        Ok(self.format_l10n(&display, Some(&tips)))
    }

    fn collect_tra2_props(&self, props: &[Prop<'_>]) -> anyhow::Result<String> {
        let mut seen = HashSet::new();
        let mut display = BTreeMap::new();

        for prop in props {
            if prop.var.is_some() || !prop.kind.eq_ignore_ascii_case("param") || prop.rest.is_none() {
                continue;
            }

            let pos = prop.pos;
            let key = prop.key.trim().split('/').collect::<Vec<_>>();
            let items = match key.as_slice() {
                [_] | [_, "check"] => &[][..],
                [_, "select", items @ ..] if !items.is_empty() => items,
                _ => continue,
            };

            let mut names = HashSet::new();
            for item in items {
                if let Some((name, _)) = item.split_once('=') {
                    let name = name.trim();
                    if !names.insert(name) {
                        bail!("{pos}: duplicate item '{name}' in 'select'");
                    }

                    display.insert(name.to_owned(), String::new());
                }
            }

            let key = key[0].trim();
            validate_property_name(key, pos)?;

            if !seen.insert(key) {
                bail!("{pos}: duplicate property name '{key}'");
            }

            display.insert(key.to_owned(), String::new());
        }

        Ok(self.format_l10n(&display, None))
    }

    fn format_l10n(&self, display: &BTreeMap<String, String>, tips: Option<&IndexMap<String, String>>) -> String {
        let mut output = String::new();

        let _ = writeln!(output, "[{}]\n{}=", self.target.key(), self.target.key());
        for (key, val) in display {
            if key == self.target.key() {
                continue;
            }

            let _ = writeln!(output, "{key}={val}");
        }

        output.push('\n');

        let Some(tips) = tips else {
            return output;
        };

        let _ = writeln!(output, "[Tips.{}]\neffect.name=", self.target.key());
        for (key, val) in tips {
            let _ = writeln!(output, "{key}={val}");
        }

        output.push('\n');

        output
    }

    fn load_include(&mut self, file: &Path) -> anyhow::Result<Source> {
        if file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("hlsl")) {
            let text = super::shader::build(
                &crate::fs::read_file(file, self.target.encoding())?,
                file,
                self.target,
                self.include_dirs,
                self.vars.as_ref(),
            )?;

            let file: Arc<Path> = file.into();
            let positions = text
                .split('\n')
                .enumerate()
                .map(|(i, _)| preprocess::Position {
                    file: Arc::clone(&file),
                    line: i + 1,
                    col: 1,
                })
                .collect();
            return Ok(Source { text, positions });
        }

        let file = std::fs::canonicalize(file)
            .with_context(|| format!("failed to canonicalize include file '{}'", file.display()))?;

        if self.include_stack.contains(&file) {
            bail!("circular include detected: '{}'", file.display());
        }

        self.include_stack.push(file.clone());

        let result = (|| self.process(&crate::fs::read_file(&file, self.target.encoding())?, &file))();

        let _ = self.include_stack.pop();

        result
    }

    fn expand<'text>(
        text: &'text str,
        vars: &impl crate::vars::Vars,
        locate: impl Fn(usize) -> preprocess::Position,
    ) -> anyhow::Result<Cow<'text, str>> {
        if !text.contains('$') {
            return Ok(Cow::Borrowed(text));
        }

        let mut lexer = lua::Lexer::new(text).peekable();
        let mut output = None;
        let mut curr = 0;

        while let Some(token) = lexer.next() {
            match token.kind {
                TokenKind::Other('$') => {
                    let at = token.span.st;
                    if let Some((key, ed)) =
                        preprocess::placeholder(text, at).map_err(|err| anyhow::anyhow!("{}: {err}", locate(at)))?
                    {
                        let val = vars
                            .get(key)
                            .ok_or_else(|| anyhow::anyhow!("{}: variable '{key}' not found", locate(at)))?;
                        let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                        output.push_str(&text[curr..at]);
                        output.push_str(val);
                        curr = ed;
                        while lexer.peek().is_some_and(|next| next.span.st < ed) {
                            let _ = lexer.next();
                        }
                    }
                }
                TokenKind::String(_) | TokenKind::Comment { .. } | TokenKind::UnclosedComment(_) => {
                    let raw = &text[token.span.st..token.span.ed];
                    let mut scan = 0;
                    let mut st = 0;
                    let mut is_replaced = false;
                    while let Some(rel) = raw[scan..].find('$') {
                        let at = scan + rel;
                        if let Some((key, ed)) = preprocess::placeholder(raw, at)
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(token.span.st + at)))?
                        {
                            let val = vars.get(key).ok_or_else(|| {
                                anyhow::anyhow!("{}: variable '{key}' not found", locate(token.span.st + at))
                            })?;
                            let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                            if !is_replaced {
                                output.push_str(&text[curr..token.span.st]);
                                is_replaced = true;
                            }
                            output.push_str(&raw[st..at]);
                            output.push_str(val);
                            st = ed;
                            scan = ed;
                        } else {
                            scan = at + 1;
                        }
                    }
                    if is_replaced {
                        output.as_mut().unwrap().push_str(&raw[st..]);
                        curr = token.span.ed;
                    }
                }
                _ => {}
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

#[allow(clippy::too_many_lines)]
fn assign_props(kind: &str, val: &str, rest: String) -> anyhow::Result<String> {
    match kind {
        "track" => {
            let Some(parsed) = val.parse::<f64>().ok().filter(|v| v.is_finite()) else {
                bail!("assignment value '{val}' of '{kind}' must be a number");
            };

            let mut parts: Vec<Cow<str>> = rest.split(',').map(Cow::Borrowed).collect();
            if parts.len() >= 4 {
                let target = parts[3].trim();
                if target == "_" {
                    parts[3] = Cow::Owned(parts[3].replacen('_', val, 1));
                    return Ok(parts.join(","));
                } else if target
                    .parse::<f64>()
                    .map_or(true, |v| (v - parsed).abs() > f64::EPSILON)
                {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "check" | "checksection" => {
            if kind == "check" && !matches!(val, "0" | "1" | "true" | "false") {
                bail!("assignment value '{val}' of '{kind}' must be 'true', 'false', '0', or '1'");
            }

            if kind == "checksection" && !matches!(val, "true" | "false") {
                bail!("assignment value '{val}' of '{kind}' must be 'true' or 'false'");
            }

            let mut parts: Vec<Cow<str>> = rest.split(',').map(Cow::Borrowed).collect();
            if parts.len() >= 2 {
                let target = parts[1].trim();
                if target == "_" {
                    parts[1] = Cow::Owned(parts[1].replacen('_', val, 1));
                    return Ok(parts.join(","));
                } else if target != val {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "select" => {
            let Ok(parsed) = val.parse::<i64>() else {
                bail!("assignment value '{val}' of '{kind}' must be an integer");
            };

            let mut parts: Vec<Cow<str>> = rest.split(',').map(Cow::Borrowed).collect();
            if let Some((name, part)) = parts.first().and_then(|part| part.split_once('=')) {
                let target = part.trim();
                if target == "_" {
                    parts[0] = Cow::Owned(format!("{name}={}", part.replacen('_', val, 1)));
                    return Ok(parts.join(","));
                } else if target.parse::<i64>() != Ok(parsed) {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "color" => {
            let parsed = parse_int(val);
            if val != "nil" && parsed.is_none() {
                bail!("assignment value '{val}' of '{kind}' must be 'nil' or an integer");
            }

            let mut parts: Vec<Cow<str>> = rest.split(',').map(Cow::Borrowed).collect();
            if parts.len() >= 2 {
                let target = parts[1].trim();
                if target == "_" {
                    parts[1] = Cow::Owned(parts[1].replacen('_', val, 1));
                    return Ok(parts.join(","));
                } else if (val == "nil" && target != "nil") || (val != "nil" && parse_int(target) != parsed) {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "file" | "folder" => {
            if parse_str(val).is_none() {
                bail!("assignment value '{val}' of '{kind}' must be a string");
            }
        }
        "value" | "font" | "figure" | "string" | "text" => {
            if let Some(parsed) = parse_str(val) {
                if parsed.chars().any(|c| c.is_control() && c != '\n') {
                    bail!("assignment value '{val}' of '{kind}' cannot contain control characters");
                }

                if let Some((name, default)) = rest.split_once(',') {
                    let target = default.trim();
                    if target == "_" {
                        let parsed = if kind == "value" {
                            format!(
                                "\"{}\"",
                                parsed.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
                            )
                        } else {
                            parsed.replace('\n', "\\n")
                        };
                        return Ok(format!("{name},{}", default.replacen('_', &parsed, 1)));
                    } else if kind == "value" {
                        if parse_str(target).as_deref() != Some(parsed.as_str()) {
                            bail!("default value '{target}' does not match assignment value '{val}'");
                        }
                    } else if target != parsed.replace('\n', "\\n") {
                        bail!("default value '{target}' does not match assignment value '{val}'");
                    }
                }
            } else if kind != "value" {
                bail!("assignment value '{val}' of '{kind}' must be a string");
            } else if let Some((name, default)) = rest.split_once(',') {
                let target = default.trim();
                if target == "_" {
                    return Ok(format!("{name},{}", default.replacen('_', val, 1)));
                }

                let is_trivia = |token: &lua::Token| {
                    matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
                    )
                };

                if !lua::Lexer::new(target)
                    .filter(|token| !is_trivia(token))
                    .map(|token| token.kind)
                    .eq(lua::Lexer::new(val)
                        .filter(|token| !is_trivia(token))
                        .map(|token| token.kind))
                {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        _ => {}
    }
    Ok(rest)
}

fn validate_track(prop: &Prop<'_>) -> anyhow::Result<()> {
    static VALID_STEPS: [f64; 10] = [
        1.0,
        0.1,
        0.01,
        0.001,
        0.000_1,
        0.000_01,
        0.000_001,
        0.000_000_1,
        0.000_000_01,
        0.000_000_001,
    ];

    let pos = prop.pos;
    let kind = prop.kind;
    let args = prop.rest.map_or(Vec::new(), |rest| {
        rest.trim().split(',').map(str::trim).collect::<Vec<_>>()
    });
    if args.len() < 4 {
        bail!("{pos}: '{kind}' requires at least 4 arguments, got {}", args.len());
    }

    if args.len() > 6 {
        tracing::warn!(
            "{pos}: '{kind}' has too many arguments; expected at most 6, got {}",
            args.len()
        );
    }

    let parse = |i, arg: &str| -> anyhow::Result<f64> {
        let Some(value) = arg.parse::<f64>().ok().filter(|v| v.is_finite()) else {
            bail!("{pos}: argument {} of '{kind}' must be a number, got '{arg}'", i + 1);
        };
        Ok(value)
    };

    let min = parse(0, args[0])?;
    let max = parse(1, args[1])?;
    let default = parse(2, args[2])?;
    let step = parse(3, args[3])?;

    if !(min <= default && default <= max) {
        bail!("{pos}: default value of '{kind}' must satisfy min ({min}) <= default ({default}) <= max ({max})");
    }

    if !VALID_STEPS.iter().any(|&v| (step - v).abs() < f64::EPSILON) {
        bail!("{pos}: step of '{kind}' must be 1, 0.1, ..., 0.000000001, got '{step}'");
    }

    if let Some(&arg) = args.get(5) {
        parse(5, arg)?;
    }

    Ok(())
}

fn validate_property_name(name: &str, pos: &preprocess::Position) -> anyhow::Result<()> {
    if name.starts_with("effect.") {
        bail!("{pos}: property name must not start with 'effect.', got '{name}'");
    }

    if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        bail!("{pos}: property name must not start with a digit, got '{name}'");
    }

    Ok(())
}

fn validate_select(items: Vec<&str>, default: &str) -> anyhow::Result<()> {
    let mut values = HashSet::new();
    let Ok(default) = default.parse::<i64>() else {
        bail!("default value of 'select' must be an integer, got '{default}'");
    };

    let mut has_default = false;
    for item in items {
        if item.matches('=').count() != 1 {
            bail!("item of 'select' must contain exactly one '=', got '{item}'");
        }

        let (name, value) = item.split_once('=').unwrap();
        let name = name.trim();
        if name.starts_with("effect.") {
            bail!("item name of 'select' must not start with 'effect.', got '{name}'");
        }

        let value = value.trim();
        let Ok(value) = value.parse::<i64>() else {
            bail!("item value of 'select' must be an integer, got '{value}'");
        };

        if !values.insert(value) {
            bail!("item value of 'select' must be unique, got '{value}'");
        }

        has_default |= value == default;
    }

    if !has_default {
        bail!("default value of 'select' must match an item value, got '{default}'");
    }

    Ok(())
}

fn validate_shader_name(name: &str, kind: &str, pos: &preprocess::Position) -> anyhow::Result<()> {
    let mut bytes = name.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        bail!("{pos}: name of '{kind}' must match [A-Za-z_][A-Za-z0-9_]*, got '{name}'");
    }

    if hlsl::is_reserved(name) {
        bail!("{pos}: name of '{kind}' cannot be an HLSL keyword or reserved word, got '{name}'");
    }

    Ok(())
}

fn has_shader_entry_point(src: &str, name: &str, shader_kind: &str) -> bool {
    use hlsl::TokenKind;

    let tokens = hlsl::Lexer::new(src)
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Whitespace
                    | TokenKind::Newline
                    | TokenKind::Comment { .. }
                    | TokenKind::Directive
                    | TokenKind::Continuation
            )
        })
        .collect::<Vec<_>>();
    let kind = |i: usize| tokens.get(i).map(|token| token.kind);
    let has_numthreads = |mut i: usize| {
        while i > 0 && kind(i - 1) == Some(TokenKind::Ident) {
            i -= 1;
        }

        while i > 0 && kind(i - 1) == Some(TokenKind::Other(']')) {
            let mut depth = 0_usize;
            let mut j = i - 1;
            loop {
                match kind(j) {
                    Some(TokenKind::Other(']')) => depth += 1,
                    Some(TokenKind::Other('[')) => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                if j == 0 {
                    return false;
                }
                j -= 1;
            }

            if kind(j + 1) == Some(TokenKind::Ident) && tokens[j + 1].text == "numthreads" {
                return true;
            }

            i = j;
        }
        false
    };

    let has_valid_signature = |name_idx: usize| {
        let is_void = kind(name_idx - 1) == Some(TokenKind::Ident) && tokens[name_idx - 1].text == "void";
        if shader_kind == "computeshader" {
            is_void && has_numthreads(name_idx - 1)
        } else {
            !is_void
        }
    };

    let mut curr = 0;
    let mut is_initializer = false;

    while let Some(token) = tokens.get(curr) {
        if !is_initializer
            && token.kind == TokenKind::Ident
            && token.text == name
            && curr > 0
            && matches!(kind(curr - 1), Some(TokenKind::Ident | TokenKind::Other('>')))
            && has_valid_signature(curr)
            && kind(curr + 1) == Some(TokenKind::Other('('))
            && let Some(mut ed) = hlsl::skip_group(&tokens, curr + 1, '(', ')')
        {
            if kind(ed) == Some(TokenKind::Other(':')) {
                ed += 1;
                if kind(ed) == Some(TokenKind::Ident) {
                    ed += 1;
                }
            }
            if kind(ed) == Some(TokenKind::Other('{')) && hlsl::skip_group(&tokens, ed, '{', '}').is_some() {
                return true;
            }
        }
        match token.kind {
            TokenKind::Other(open @ ('{' | '(' | '[')) => {
                curr = hlsl::skip_group(
                    &tokens,
                    curr,
                    open,
                    match open {
                        '{' => '}',
                        '(' => ')',
                        _ => ']',
                    },
                )
                .unwrap_or(tokens.len());
                if open == '{' {
                    is_initializer = false;
                }
                continue;
            }
            TokenKind::Other('=') => is_initializer = true,
            TokenKind::Other(';') => is_initializer = false,
            _ => {}
        }
        curr += 1;
    }
    false
}

fn parse_int(src: &str) -> Option<i64> {
    src.strip_prefix("0x")
        .or_else(|| src.strip_prefix("0X"))
        .and_then(|hex| i64::from_str_radix(hex, 16).ok())
        .or_else(|| src.parse::<i64>().ok())
}

fn parse_str(src: &str) -> Option<String> {
    let mut tokens =
        lua::Lexer::new(src).filter(|token| !matches!(token.kind, TokenKind::Whitespace | TokenKind::Newline));
    match (tokens.next(), tokens.next()) {
        (
            Some(lua::Token {
                kind: TokenKind::String(bytes),
                ..
            }),
            None,
        ) => String::from_utf8(bytes.into_owned()).ok(),
        _ => None,
    }
}

fn is_var(name: &str) -> bool {
    let mut tokens = lua::Lexer::new(name);
    matches!(tokens.next().map(|token| token.kind), Some(TokenKind::Ident(_))) && tokens.next().is_none()
}
