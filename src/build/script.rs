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
    dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    bundled: bool,
    suffix: &str,
) -> anyhow::Result<Output> {
    Builder {
        target,
        dirs,
        vars: Cow::Borrowed(vars),
        bundled,
        suffix,
        stack: std::fs::canonicalize(target.path()).into_iter().collect(),
    }
    .build(src)
}

#[derive(Debug, Default)]
pub struct Output {
    pub script: String,
    pub l10n: String,
}

impl Output {
    pub fn push_str(&mut self, src: &Self) {
        self.script.push_str(&src.script);
        self.l10n.push_str(&src.l10n);
    }

    pub fn replace(&mut self, from: &str, to: &str) {
        self.script = self.script.replace(from, to);
        self.l10n = self.l10n.replace(from, to);
    }
}

struct Builder<'a> {
    target: &'a BuildTarget,
    dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    bundled: bool,
    suffix: &'a str,
    stack: Vec<PathBuf>,
}

impl Builder<'_> {
    fn build(mut self, src: &str) -> anyhow::Result<Output> {
        let src = self.process(src, Path::new(self.target.path()), 0, 0)?;
        let mut src = self.resolve_header(src)?;

        if self.suffix.ends_with('2') && !self.suffix.eq_ignore_ascii_case(".tra2") {
            src = Self::normalize_props(src)?;
        }

        let props = self.extract_props(&src)?;
        let l10n = if self.suffix.eq_ignore_ascii_case(".tra2") {
            validate_tra_props(&props, true)?;
            self.collect_props(&props)?
        } else if self.suffix.ends_with('2') {
            let mut vars = HashSet::new();
            validate_compat_props(&props, &mut vars)?;
            validate_modern_props(&props, &mut vars)?;
            validate_shader(&src)?;
            self.collect_props(&props)?
        } else {
            if self.suffix.eq_ignore_ascii_case(".tra") {
                validate_tra_props(&props, false)?;
            } else {
                let mut vars = HashSet::new();
                self.validate_legacy_props(&props)?;
                validate_compat_props(&props, &mut vars)?;
            }
            String::new()
        };

        Ok(Output { l10n, script: src.text })
    }

    #[allow(clippy::too_many_lines)]
    fn process(&mut self, src: &str, file: &Path, line: usize, col: usize) -> anyhow::Result<Source> {
        let dir = file.parent().unwrap_or(Path::new("."));
        let file: Arc<Path> = file.into();
        let lines = std::iter::once(0)
            .chain(src.match_indices('\n').map(|(st, _)| st + 1))
            .collect::<Vec<_>>();
        let mut dst = String::with_capacity(src.len());
        let mut positions = Vec::new();
        let mut includes = Vec::new();

        let locate = |at| {
            let i = lines.partition_point(|&st| st <= at).saturating_sub(1);
            preprocess::Position {
                file: Arc::clone(&file),
                line: line + i + 1,
                col: src[lines[i]..at].chars().count() + 1 + if i == 0 { col } else { 0 },
            }
        };

        let mut emitted = false;
        let mut lexer = lua::Lexer::new(src).peekable();
        let mut blocked_ed = 0;
        let mut conditions = preprocess::Conditions::default();
        let mut i = 0;

        while lines.get(i).is_some_and(|&st| st < src.len()) {
            let st = lines[i];
            let ed = lines.get(i + 1).map_or(src.len(), |st| st - 1);
            let line = &src[st..ed];
            let mut col = line.len() - line.trim_start().len();

            let mut blocked = blocked_ed;
            let mut first = None;
            while let Some(token) = lexer.next_if(|token| token.span.st < ed) {
                if token.span.st >= st
                    && matches!(&token.kind, TokenKind::Comment { is_block: false, .. })
                    && !line.trim_start().starts_with('@')
                {
                    col = token.span.st - st;
                }
                if token.span.st >= st && token.span.ed > ed {
                    blocked = blocked.max(token.span.ed);
                }

                if first.is_none()
                    && token.span.st >= st
                    && !matches!(&token.kind, TokenKind::Whitespace | TokenKind::Newline)
                {
                    first = Some(token);
                }
            }

            let block = first.as_ref().is_some_and(|token| {
                matches!(
                    &token.kind,
                    TokenKind::Comment { is_block: true, .. } | TokenKind::UnclosedComment(_)
                )
            });

            let comment = if blocked_ed > st {
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

            blocked_ed = blocked;

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
                    .map_or(i + 1, |token| lines.partition_point(|&st| st < token.span.ed))
            };

            if matches!(&action, preprocess::Action::Drop | preprocess::Action::PragmaOnce) {
                i = if block { next_line(i) } else { i + 1 };
                continue;
            }

            if let Some(token) = first.as_ref()
                && let TokenKind::Comment {
                    content: body,
                    is_block: true,
                } = &token.kind
                && (body.starts_with("pixelshader@") || body.starts_with("computeshader@"))
            {
                let body_st = body.as_ptr() as usize - src.as_ptr() as usize;
                let body_ed = body_st + body.len();
                let origin = locate(body_st);
                let mut nested = self.process(body, file.as_ref(), origin.line - 1, origin.col - 1)?;
                let ed_line = lines.partition_point(|&st| st <= token.span.ed).saturating_sub(1);

                if body.ends_with('\n') && !nested.text.ends_with('\n') {
                    nested.text.push('\n');
                    nested.positions.push(preprocess::Origin {
                        pos: locate(body_ed),
                        cols: Vec::new(),
                    });
                }

                let mut cols = vec![preprocess::Column {
                    st: 0,
                    col: locate(body_ed).col,
                    replaced: false,
                }];
                let suffix = Self::expand(
                    &src[body_ed..lines.get(ed_line + 1).map_or(src.len(), |st| st - 1)],
                    self.vars.as_ref(),
                    |at| locate(body_ed + at),
                    |(st, at, replaced)| {
                        cols.push(preprocess::Column {
                            st,
                            col: locate(body_ed + at).col,
                            replaced,
                        });
                    },
                )?;
                if emitted {
                    dst.push('\n');
                }

                if let Some(origin) = nested.positions.first_mut() {
                    for column in &mut origin.cols {
                        column.st += body_st - st;
                    }
                    origin.cols.insert(
                        0,
                        preprocess::Column {
                            st: 0,
                            col: locate(st).col,
                            replaced: false,
                        },
                    );
                }
                dst.push_str(&src[st..body_st]);
                dst.push_str(&nested.text);
                dst.push_str(suffix.as_ref());
                let mut origins = preprocess::map_lines(&suffix, &locate(body_ed), &cols).into_iter();
                if let Some(origin) = nested.positions.last_mut() {
                    let offset = nested.text.rsplit('\n').next().unwrap_or("").len()
                        + if nested.text.contains('\n') { 0 } else { body_st - st };
                    origin
                        .cols
                        .extend(origins.next().unwrap().cols.into_iter().map(|col| preprocess::Column {
                            st: col.st + offset,
                            ..col
                        }));
                }
                positions.extend(nested.positions);
                positions.extend(origins);

                emitted = true;
                i = ed_line + 1;
                continue;
            }

            if let preprocess::Action::Define(key, val) = &action {
                if RESERVED_VARIABLES.contains(key) {
                    bail!("{pos}: cannot define reserved variable '{key}'");
                }

                let vars = self.vars.to_mut();
                vars.insert(
                    (*key).to_owned(),
                    Self::expand(
                        val,
                        vars,
                        |at| locate(val.as_ptr() as usize - src.as_ptr() as usize + at),
                        |_| {},
                    )?
                    .into_owned(),
                );
                i = if block { next_line(i) } else { i + 1 };
                continue;
            }

            if let preprocess::Action::Undef(key) = &action {
                if self.vars.contains_key(*key) {
                    self.vars.to_mut().shift_remove(*key);
                }

                i = if block { next_line(i) } else { i + 1 };
                continue;
            }

            if let preprocess::Action::Include(include, is_quoted) = &action {
                let include = Self::expand(
                    include.as_ref(),
                    self.vars.as_ref(),
                    |at| match include {
                        Cow::Borrowed(text) => locate(text.as_ptr() as usize - src.as_ptr() as usize + at),
                        Cow::Owned(_) => pos.clone(),
                    },
                    |_| {},
                )?;

                let file = is_quoted
                    .then(|| dir.join(include.as_ref()))
                    .filter(|file| file.is_file())
                    .or_else(|| {
                        self.dirs
                            .iter()
                            .map(|dir| dir.join(include.as_ref()))
                            .find(|file| file.is_file())
                    })
                    .ok_or_else(|| {
                        anyhow::anyhow!("{pos}: include file '{include}' not found from '{}'", dir.display())
                    })?;

                let Source {
                    text: nested,
                    positions: mut locations,
                } = self.load_include(&file)?;

                if locations.is_empty() {
                    locations.push(preprocess::Origin {
                        pos: pos.clone(),
                        cols: Vec::new(),
                    });
                }

                if emitted {
                    dst.push('\n');
                }

                let st = dst.len();
                let indent = first.as_ref().map_or("", |token| &line[..token.span.st - lines[i]]);
                dst.push_str(&textwrap::indent(&nested, indent));
                if !indent.is_empty() {
                    for origin in &mut locations {
                        if let Some(column) = origin.cols.first().copied() {
                            for column in &mut origin.cols {
                                column.st += indent.len();
                            }
                            origin.cols.insert(
                                0,
                                preprocess::Column {
                                    st: 0,
                                    replaced: true,
                                    ..column
                                },
                            );
                        }
                    }
                }

                if file
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
                {
                    includes.push((
                        st,
                        dst.len(),
                        positions.len(),
                        file,
                        include.into_owned(),
                        nested,
                        indent.len(),
                    ));
                }

                positions.extend(locations);

                emitted = true;
                i += 1;
                continue;
            }

            if emitted {
                dst.push('\n');
            }

            let mut cols = vec![preprocess::Column {
                st: 0,
                col: locate(st).col,
                replaced: false,
            }];
            let line = Self::expand(
                line,
                self.vars.as_ref(),
                |at| locate(st + at),
                |(at, offset, replaced)| {
                    cols.push(preprocess::Column {
                        st: at,
                        col: locate(st + offset).col,
                        replaced,
                    });
                },
            )?;
            positions.extend(preprocess::map_lines(&line, &pos, &cols));
            dst.push_str(line.as_ref());
            emitted = true;
            i += 1;
        }

        conditions.finish()?;

        if includes.is_empty() {
            return Ok(Source { text: dst, positions });
        }

        let text = dst;
        let locations = positions;
        let mut dst = String::with_capacity(text.len());
        let mut positions = Vec::with_capacity(locations.len());
        let mut curr = 0;
        let mut line = 0;

        for (i, (st, ed, st_line, file, include, nested, padding)) in includes.iter().enumerate() {
            let Some(found) = lua::find_require(&text[..includes.get(i + 1).map_or(text.len(), |next| next.0)], *ed)
            else {
                continue;
            };

            let ed_line = st_line + nested.matches('\n').count();
            let req_line = ed_line + text[*ed..found.st].matches('\n').count();
            let req_st = text[..found.st].rfind('\n').map_or(0, |st| st + 1);
            let pos = locations[req_line].locate(&text[req_st..], found.st - req_st);

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
            let prefix = prefix.strip_prefix('\n').unwrap_or(prefix);

            dst.push_str(&text[curr..*st]);

            let _ = write!(
                dst,
                "{prefix}(function()\n{}\n{indent}end)()",
                textwrap::indent(nested, &format!("{indent}    "))
            );

            positions.extend_from_slice(&locations[line..*st_line]);
            positions
                .extend_from_slice(&locations[ed_line + usize::from(text[*ed..found.st].starts_with('\n'))..=req_line]);
            positions.extend(locations[*st_line..=ed_line].iter().cloned().map(|mut origin| {
                if let Some(column) = origin.cols.first().copied() {
                    for column in &mut origin.cols {
                        column.st = column.st.saturating_sub(*padding) + indent.len() + 4;
                    }
                    origin.cols.insert(
                        0,
                        preprocess::Column {
                            st: 0,
                            replaced: true,
                            ..column
                        },
                    );
                }
                origin
            }));
            positions.push(preprocess::Origin { pos, cols: Vec::new() });

            curr = found.ed;
            line = req_line + text[found.st..found.ed].matches('\n').count() + 1;
        }

        dst.push_str(&text[curr..]);
        positions.extend_from_slice(&locations[line..]);

        Ok(Source { text: dst, positions })
    }

    fn load_include(&mut self, file: &Path) -> anyhow::Result<Source> {
        if file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("hlsl")) {
            let super::shader::Output { mut text, positions } = super::shader::build(
                &crate::fs::read_file(file, self.target.encoding())?,
                file,
                self.target,
                self.dirs,
                self.vars.as_ref(),
            )?;

            if text.ends_with('\n') {
                text.pop();
            }

            return Ok(Source { text, positions });
        }

        let file = std::fs::canonicalize(file)
            .with_context(|| format!("failed to canonicalize include file '{}'", file.display()))?;

        if self.stack.contains(&file) {
            bail!("circular include detected: '{}'", file.display());
        }

        self.stack.push(file.clone());

        let res = crate::fs::read_file(&file, self.target.encoding()).and_then(|src| self.process(&src, &file, 0, 0));

        let _ = self.stack.pop();

        res
    }

    fn expand<'text>(
        text: &'text str,
        vars: &impl crate::vars::Vars,
        locate: impl Fn(usize) -> preprocess::Position,
        mut record: impl FnMut((usize, usize, bool)),
    ) -> anyhow::Result<Cow<'text, str>> {
        if !text.contains('$') {
            return Ok(Cow::Borrowed(text));
        }

        let mut lexer = lua::Lexer::new(text).peekable();
        let mut dst = None;
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
                        let dst = dst.get_or_insert_with(|| String::with_capacity(text.len()));
                        dst.push_str(&text[curr..at]);
                        record((dst.len(), at, true));
                        dst.push_str(val);
                        record((dst.len(), ed, false));
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
                    let mut replaced = false;
                    while let Some(rel) = raw[scan..].find('$') {
                        let at = scan + rel;
                        if let Some((key, ed)) = preprocess::placeholder(raw, at)
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(token.span.st + at)))?
                        {
                            let val = vars.get(key).ok_or_else(|| {
                                anyhow::anyhow!("{}: variable '{key}' not found", locate(token.span.st + at))
                            })?;
                            let dst = dst.get_or_insert_with(|| String::with_capacity(text.len()));
                            if !replaced {
                                dst.push_str(&text[curr..token.span.st]);
                                replaced = true;
                            }
                            dst.push_str(&raw[st..at]);
                            record((dst.len(), token.span.st + at, true));
                            dst.push_str(val);
                            record((dst.len(), token.span.st + ed, false));
                            st = ed;
                            scan = ed;
                        } else {
                            scan = at + 1;
                        }
                    }
                    if replaced {
                        dst.as_mut().unwrap().push_str(&raw[st..]);
                        curr = token.span.ed;
                    }
                }
                _ => {}
            }
        }

        if let Some(mut dst) = dst {
            dst.push_str(&text[curr..]);
            Ok(Cow::Owned(dst))
        } else {
            Ok(Cow::Borrowed(text))
        }
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
            if !self.bundled {
                bail!("{}: single target cannot have '@' header", positions[0].pos);
            }
            (Some(first.trim_end().to_owned()), tail)
        } else if let Some(caps) = PATTERN.captures(first) {
            if self.bundled {
                (Some(format!("@{}", caps["header"].trim())), tail)
            } else {
                tracing::warn!(
                    "{}: single target has '--@' on the first line; leaving it as comment",
                    positions[0].pos
                );
                (Some(first.to_owned()), tail)
            }
        } else if self.bundled {
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

        src.text = PATTERN
            .replace_all(text, |caps: &regex::Captures| {
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

                let origin = &src.positions[text[..caps.name("kind").unwrap().start()].matches('\n').count()];
                let pos = &origin.pos;
                let st = text[..m.start()].matches('\n').count();
                let ed = text[..m.end()].matches('\n').count();
                if st != ed {
                    merges.push((st, ed, origin.clone()));
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
            })
            .into_owned();

        if let Some(err) = err {
            return Err(err);
        }

        for (st, ed, pos) in merges.into_iter().rev() {
            drop(src.positions.splice(st..=ed, std::iter::once(pos)));
        }

        Ok(src)
    }

    fn extract_props<'s>(&self, src: &'s Source) -> anyhow::Result<Vec<Prop<'s>>> {
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
            let Some(caps) = PATTERN.captures(&text[token.span.st..token.span.ed]) else {
                continue;
            };

            if caps.name("var").is_some() && (!self.suffix.ends_with('2') || self.suffix.eq_ignore_ascii_case(".tra2"))
            {
                bail!(
                    "{}: '{}' cannot use '@' in '{}'",
                    src.positions[i].pos,
                    &caps["kind"],
                    self.suffix
                );
            }

            if !text[..curr].rsplit('\n').next().unwrap_or("").trim().is_empty() {
                continue;
            }

            if caps.get(0).unwrap().as_str().chars().any(char::is_control) {
                bail!("{}: annotation cannot contain control characters", src.positions[i].pos);
            }

            props.push(Prop {
                pos: &src.positions[i].pos,
                kind: caps.name("kind").unwrap().as_str(),
                var: caps.name("var").map(|m| m.as_str().trim()),
                key: caps.name("key").unwrap().as_str(),
                rest: caps.name("rest").map(|m| m.as_str()),
            });
        }

        Ok(props)
    }

    fn validate_legacy_props(&self, props: &[Prop<'_>]) -> anyhow::Result<()> {
        let numbered = |kind: &str, prefix: &str| {
            kind.strip_prefix(prefix)
                .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
        };

        for prop in props {
            let kind = prop.kind.to_ascii_lowercase();
            let pos = prop.pos;

            if numbered(&kind, "track") {
                if !matches!(kind.as_str(), "track0" | "track1")
                    && (self.suffix.eq_ignore_ascii_case(".scn") || !matches!(kind.as_str(), "track2" | "track3"))
                {
                    bail!("{pos}: '{kind}' is not supported in '{}'", self.suffix);
                }

                if let Some(rest) = prop.rest {
                    let mut args = rest.trim().split(',').map(str::trim);
                    let count = args.clone().count();
                    if count > 4 {
                        bail!("{pos}: '{kind}' has too many arguments; expected at most 4, got {count}");
                    }

                    if let Some(step) = args.nth(3)
                        && !step.parse::<f64>().is_ok_and(|step| {
                            [1.0, 0.1, 0.01]
                                .iter()
                                .any(|&value| (step - value).abs() < f64::EPSILON)
                        })
                    {
                        bail!("{pos}: step of '{kind}' must be 1, 0.1, or 0.01, got '{step}'");
                    }
                }
            } else if numbered(&kind, "check") && kind != "check0" {
                bail!("{pos}: '{kind}' is not supported in '{}'", self.suffix);
            } else if kind == "color" {
                let key = prop.key.trim();
                if !key
                    .strip_prefix("0x")
                    .or_else(|| key.strip_prefix("0X"))
                    .and_then(|hex| i64::from_str_radix(hex, 16).ok())
                    .or_else(|| key.parse::<i64>().ok())
                    .is_some_and(|value| (0..=0xff_ffff).contains(&value))
                {
                    bail!(
                        "{pos}: default value of 'color' must be an integer between 0x000000 and 0xffffff, got '{key}'"
                    );
                }
            } else if kind == "param" {
                let (bytes, _, has_unmappable) = encoding_rs::SHIFT_JIS.encode(prop.key);
                if has_unmappable {
                    bail!("{pos}: key of 'param' contains characters that cannot be encoded in Shift_JIS");
                }

                if bytes.len() > 255 {
                    bail!(
                        "{pos}: key of 'param' must be at most 255 bytes in Shift_JIS, got {}",
                        bytes.len()
                    );
                }
            } else if kind == "dialog" {
                let items = prop.rest.map_or_else(
                    || Cow::Borrowed(prop.key),
                    |rest| Cow::Owned(format!("{},{rest}", prop.key)),
                );

                let count = items.split(';').count();

                if count > 16 {
                    bail!("{pos}: 'dialog' has too many items; expected at most 16, got {count}");
                }

                let mut counts = [0; 3];
                for item in items.split(';') {
                    if let Some((ctrl, index)) = match item
                        .split_once(',')
                        .map_or(item, |(name, _)| name)
                        .trim()
                        .rsplit_once('/')
                        .map(|(_, suffix)| suffix)
                    {
                        Some("chk") => Some(("chk", 0)),
                        Some("col") => Some(("col", 1)),
                        Some("fig") => Some(("fig", 2)),
                        _ => None,
                    } {
                        counts[index] += 1;
                        if counts[index] > 4 {
                            bail!(
                                "{pos}: 'dialog' has too many '/{ctrl}' items; expected at most 4, got {}",
                                counts[index]
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::match_same_arms, clippy::too_many_lines)]
    fn collect_props(&self, props: &[Prop<'_>]) -> anyhow::Result<String> {
        let validate = |name: &str, pos: &preprocess::Position| -> anyhow::Result<()> {
            if name.starts_with("effect.") {
                bail!("{pos}: property name must not start with 'effect.', got '{name}'");
            }

            if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                bail!("{pos}: property name must not start with a digit, got '{name}'");
            }

            Ok(())
        };

        let mut disp = BTreeMap::new();

        if self.suffix.eq_ignore_ascii_case(".tra2") {
            for prop in props {
                if prop.var.is_some() || !prop.kind.eq_ignore_ascii_case("param") || prop.rest.is_none() {
                    continue;
                }

                let key = prop.key.trim().split('/').collect::<Vec<_>>();
                let items = match key.as_slice() {
                    [_] | [_, "check"] => &[][..],
                    [_, "select", items @ ..] if !items.is_empty() => items,
                    _ => continue,
                };

                for item in items {
                    if let Some((name, _)) = item.split_once('=') {
                        let name = name.trim();
                        disp.insert(name.rsplit("::").next().unwrap_or(name).to_owned(), String::new());
                    }
                }

                let key = key[0].trim();
                validate(key, prop.pos)?;

                disp.insert(key.rsplit("::").next().unwrap_or(key).to_owned(), String::new());
            }

            return Ok(self.format_l10n(&disp, None));
        }

        let mut seen = HashSet::new();
        let mut tips = IndexMap::new();
        for prop in props {
            let kind = prop.kind.to_ascii_lowercase();
            let pos = prop.pos;

            let mut check = |key| -> anyhow::Result<()> {
                validate(key, pos)?;
                if key.contains("dialog::") {
                    bail!("{pos}: property name cannot contain 'dialog::', got '{key}'");
                }

                if !seen.insert(key) {
                    bail!("{pos}: duplicate property name '{key}'");
                }
                Ok(())
            };

            let mut key = prop.key.trim();
            match (prop.var, kind.as_str()) {
                (None, kind)
                    if kind
                        .strip_prefix("track")
                        .or_else(|| kind.strip_prefix("check"))
                        .is_some_and(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit())) => {}
                (None, "color") => {
                    check("色")?;
                    disp.insert("色".to_owned(), String::new());
                    tips.insert("色".to_owned(), String::new());
                    continue;
                }
                (None, "file") => {
                    check("ファイル")?;
                    disp.insert("ファイル".to_owned(), String::new());
                    tips.insert("ファイル".to_owned(), String::new());
                    continue;
                }
                (None, "param") => {
                    for item in prop.key.split(';') {
                        let Some((var, _)) = item.split_once('=') else {
                            bail!("{pos}: item of 'param' must contain '=', got '{item}'");
                        };

                        let var = var.trim();
                        check(var)?;
                        disp.insert(var.to_owned(), String::new());
                        tips.insert(var.to_owned(), String::new());
                    }
                    continue;
                }
                (None, "dialog") => {
                    let items = prop.rest.map_or_else(
                        || Cow::Borrowed(prop.key),
                        |rest| Cow::Owned(format!("{},{rest}", prop.key)),
                    );

                    let mut names = Vec::new();
                    let mut counts = BTreeMap::new();
                    for item in items.split(';') {
                        let Some((name, _)) = item.split_once(',') else {
                            bail!("{pos}: item of 'dialog' must contain ',', got '{item}'");
                        };

                        let name = name.trim();
                        let name = ["/chk", "/col", "/fig"]
                            .iter()
                            .find_map(|&suffix| name.strip_suffix(suffix))
                            .unwrap_or(name);

                        validate(name, pos)?;
                        if name.contains("dialog::") {
                            bail!("{pos}: property name cannot contain 'dialog::', got '{name}'");
                        }
                        names.push(name);
                        *counts.entry(name).or_insert(0_usize) += 1;
                        disp.insert(name.rsplit("::").next().unwrap_or(name).to_owned(), String::new());
                    }

                    for name in names {
                        let count = counts.get_mut(name).expect("dialog item count must exist");
                        *count -= 1;
                        tips.insert(format!("{}{name}", "dialog::".repeat(*count)), String::new());
                    }
                    continue;
                }
                (None, "group" | "separator") => {
                    if kind == "group" {
                        check(key)?;
                    }

                    disp.insert(key.rsplit("::").next().unwrap_or(key).to_owned(), String::new());
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
                        if name.contains("dialog::") {
                            bail!("{pos}: property name cannot contain 'dialog::', got '{name}'");
                        }
                        if !names.insert(name) {
                            bail!("{pos}: duplicate item '{name}' in 'select'");
                        }
                        disp.insert(name.rsplit("::").next().unwrap_or(name).to_owned(), String::new());
                    }
                }
            }

            check(key)?;
            tips.insert(key.to_owned(), String::new());
            disp.insert(key.rsplit("::").next().unwrap_or(key).to_owned(), String::new());
        }

        Ok(self.format_l10n(&disp, Some(&tips)))
    }

    fn format_l10n(&self, disp: &BTreeMap<String, String>, tips: Option<&IndexMap<String, String>>) -> String {
        let mut dst = String::new();

        let _ = writeln!(dst, "[{}]\n{}=", self.target.key(), self.target.key());
        for (key, val) in disp {
            if key == self.target.key() {
                continue;
            }

            let _ = writeln!(dst, "{key}={val}");
        }

        dst.push('\n');

        let Some(tips) = tips else {
            return dst;
        };

        let _ = writeln!(dst, "[Tips.{}]\neffect.name=", self.target.key());
        for (key, val) in tips {
            let _ = writeln!(dst, "{key}={val}");
        }

        dst.push('\n');

        dst
    }
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
    positions: Vec<preprocess::Origin>,
}

struct Packing {
    depth: usize,
    is_row_major: Option<bool>,
}

impl Packing {
    fn apply<'a>(&mut self, token: &hlsl::Token<'a>) -> Option<&'a str> {
        let directive = hlsl::parse_directive(*token)?;
        match directive.name {
            "if" | "ifdef" | "ifndef" => self.depth += 1,
            "endif" => self.depth = self.depth.saturating_sub(1),
            "include" => self.is_row_major = None,
            "pragma" => {
                match hlsl::Lexer::new(directive.rest)
                    .filter(|token| {
                        !matches!(
                            token.kind,
                            hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment { .. }
                        )
                    })
                    .map(|token| token.text)
                    .collect::<Vec<_>>()
                    .as_slice()
                {
                    ["pack_matrix", "(", "row_major", ")"] if self.depth == 0 => self.is_row_major = Some(true),
                    ["pack_matrix", "(", "column_major", ")"] if self.depth == 0 => self.is_row_major = Some(false),
                    ["pack_matrix", ..] => self.is_row_major = None,
                    _ => {}
                }
            }
            _ => {}
        }
        Some(directive.name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout {
    size: usize,
    payload: usize,
    alignment: usize,
}

#[allow(clippy::too_many_lines)]
fn assign_props(kind: &str, val: &str, rest: String) -> anyhow::Result<String> {
    let parse = |src: &str| {
        src.strip_prefix("0x")
            .or_else(|| src.strip_prefix("0X"))
            .and_then(|hex| i64::from_str_radix(hex, 16).ok())
            .or_else(|| src.parse::<i64>().ok())
    };
    let string = |src: &str| {
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
    };

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
            let parsed = parse(val);
            if val != "nil" && parsed.is_none() {
                bail!("assignment value '{val}' of '{kind}' must be 'nil' or an integer");
            }

            let mut parts: Vec<Cow<str>> = rest.split(',').map(Cow::Borrowed).collect();
            if parts.len() >= 2 {
                let target = parts[1].trim();
                if target == "_" {
                    parts[1] = Cow::Owned(parts[1].replacen('_', val, 1));
                    return Ok(parts.join(","));
                } else if (val == "nil" && target != "nil") || (val != "nil" && parse(target) != parsed) {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "file" | "folder" => {
            if string(val).is_none() {
                bail!("assignment value '{val}' of '{kind}' must be a string");
            }
        }
        "value" | "font" | "figure" | "string" | "text" => {
            if let Some(parsed) = string(val) {
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
                        if string(target).as_deref() != Some(parsed.as_str()) {
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

                let trivia = |token: &lua::Token| {
                    matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
                    )
                };

                if !lua::Lexer::new(target)
                    .filter(|token| !trivia(token))
                    .map(|token| token.kind)
                    .eq(lua::Lexer::new(val)
                        .filter(|token| !trivia(token))
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

#[allow(clippy::too_many_lines)]
fn validate_compat_props(props: &[Prop<'_>], vars: &mut HashSet<String>) -> anyhow::Result<()> {
    let numbered = |kind: &str, prefix: &str| {
        kind.strip_prefix(prefix)
            .is_some_and(|index| !index.is_empty() && index.bytes().all(|b| b.is_ascii_digit()))
    };

    let parse = |src: &str| {
        src.strip_prefix("0x")
            .or_else(|| src.strip_prefix("0X"))
            .and_then(|hex| i64::from_str_radix(hex, 16).ok())
            .or_else(|| src.parse::<i64>().ok())
    };
    let valid = |name: &str| {
        let mut tokens = lua::Lexer::new(name);
        matches!(tokens.next().map(|token| token.kind), Some(TokenKind::Ident(_))) && tokens.next().is_none()
    };

    let mut exclusive = None;
    let mut kinds = HashSet::new();
    for prop in props {
        if prop.var.is_some() {
            continue;
        }

        let kind = prop.kind.to_ascii_lowercase();
        let pos = prop.pos;
        if matches!(kind.as_str(), "color" | "file" | "param" | "dialog") {
            if let Some(prev) = exclusive {
                bail!(
                    "{pos}: '{kind}' cannot coexist with '{prev}'; \
                     only one color, file, param, or dialog is allowed"
                );
            }
            exclusive = Some(prop.kind);
        }

        if (numbered(&kind, "track") || numbered(&kind, "check")) && !kinds.insert(kind.clone()) {
            bail!("{pos}: duplicate property '{kind}'");
        }

        if matches!(kind.as_str(), "color" | "file") && !vars.insert(kind.clone()) {
            bail!("{pos}: duplicate variable name '{kind}'");
        }

        match kind.as_str() {
            kind if numbered(kind, "track") => validate_track(prop)?,
            kind if numbered(kind, "check") => {
                if !matches!(prop.rest.map(str::trim), Some("0" | "1")) {
                    bail!(
                        "{}: default value of '{kind}' must be '0' or '1', got '{}'",
                        prop.pos,
                        prop.rest.unwrap_or("").trim()
                    );
                }
            }
            "color" => {
                if prop.rest.is_some_and(|rest| !rest.trim().is_empty()) {
                    bail!(
                        "{pos}: argument of 'color' must be empty, got '{}'",
                        prop.rest.unwrap_or("").trim()
                    );
                }

                let value = prop.key.trim();
                if value != "nil" && !parse(value).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                    bail!(
                        "{pos}: default value of 'color' must be 'nil' or an integer \
                         between 0x000000 and 0xffffff, got '{value}'"
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
                    if !valid(var) {
                        bail!(
                            "{}: variable name of 'param' must be a valid Lua identifier, got '{var}'",
                            prop.pos
                        );
                    }
                    if !vars.insert(var.to_owned()) {
                        bail!("{pos}: duplicate variable name '{var}'");
                    }
                }
            }
            "dialog" => {
                let items = prop.rest.map_or_else(
                    || Cow::Borrowed(prop.key),
                    |rest| Cow::Owned(format!("{},{rest}", prop.key)),
                );

                for item in items.split(';') {
                    let Some((name, assignment)) = item.split_once(',') else {
                        bail!("{pos}: item of 'dialog' must contain ',', got '{item}'");
                    };

                    let Some((var, value)) = assignment.split_once('=') else {
                        bail!("{}: item of 'dialog' must contain '=', got '{item}'", prop.pos);
                    };

                    let var = var.trim();
                    if !valid(var) {
                        bail!(
                            "{}: variable name of 'dialog' must be a valid Lua identifier, got '{var}'",
                            prop.pos
                        );
                    }

                    if !vars.insert(var.to_owned()) {
                        bail!("{pos}: duplicate variable name '{var}'");
                    }

                    match name.trim().rsplit_once('/').map(|(_, suffix)| suffix) {
                        Some("chk") => {
                            let value = value.trim();
                            if !matches!(value, "0" | "1") {
                                bail!("{pos}: value of '/chk' in 'dialog' must be '0' or '1', got '{value}'");
                            }
                        }
                        Some("col") => {
                            let value = value.trim();
                            if value != "nil" && !parse(value).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                                bail!(
                                    "{pos}: argument of 'color' must be 'nil' or an integer \
                                     between 0x000000 and 0xffffff, got '{value}'"
                                );
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_modern_props(props: &[Prop<'_>], vars: &mut HashSet<String>) -> anyhow::Result<()> {
    for prop in props {
        let kind = prop.kind.to_ascii_lowercase();
        let pos = prop.pos;

        let Some(var) = prop.var else {
            match kind.as_str() {
                kind if kind
                    .strip_prefix("track")
                    .or_else(|| kind.strip_prefix("check"))
                    .is_some_and(|i| !i.is_empty() && i.bytes().all(|b| b.is_ascii_digit()))
                    || matches!(kind, "color" | "file" | "param" | "dialog") =>
                {
                    tracing::warn!("{pos}: '{kind}' uses legacy syntax");
                }
                "group" => {
                    if let Some(rest) = prop.rest.map(str::trim)
                        && !rest.is_empty()
                    {
                        let args = rest.split(',').map(str::trim).collect::<Vec<_>>();
                        if !matches!(args[0], "true" | "false") {
                            bail!(
                                "{pos}: folding of '{kind}' must be 'true' or 'false', got '{}'",
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
                        "lua" => tracing::warn!("{pos}: runtime of '{kind}' is 'Lua'"),
                        _ => {
                            bail!(
                                "{pos}: runtime of '{kind}' must be 'LuaJIT' or 'Lua', got '{}'",
                                prop.key.trim()
                            );
                        }
                    }
                }
                _ => {}
            }
            continue;
        };

        let validate = |var: &str| -> anyhow::Result<()> {
            let mut tokens = lua::Lexer::new(var);
            if !matches!(tokens.next().map(|token| token.kind), Some(TokenKind::Ident(_))) || tokens.next().is_some() {
                bail!("{pos}: variable name of '{kind}' must be a valid Lua identifier, got '{var}'");
            }
            Ok(())
        };

        match kind.as_str() {
            "track" | "check" | "checksection" | "color" | "file" | "folder" | "string" | "text" | "font"
            | "figure" | "select" | "value" => {
                validate(var)?;
                if !vars.insert(var.to_owned()) {
                    bail!("{pos}: duplicate variable name '{var}'");
                }
            }
            "hide" => validate(var)?,
            "trackgroup" => {
                for var in var.split(',').map(str::trim) {
                    validate(var)?;
                }
            }
            _ => {}
        }

        if kind == "track" {
            validate_track(prop)?;
            continue;
        }

        let rest = prop.rest.map(str::trim);
        let args = || rest.map_or(Vec::new(), |r| r.split(',').map(str::trim).collect::<Vec<_>>());

        match kind.as_str() {
            "check" => {
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
                        "{pos}: default value of '{kind}' must be 'true', 'false', '0', or '1', got '{}'",
                        args[0]
                    );
                }
            }
            "checksection" => {
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
                        "{pos}: default value of 'checksection' must be 'true' or 'false', got '{}'",
                        args[0]
                    );
                }

                if let Some(&arg) = args.get(1)
                    && !matches!(arg, "true" | "false")
                {
                    bail!("{pos}: folding of 'checksection' must be 'true' or 'false', got '{arg}'");
                }
            }
            "select" => {
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

                validate_select(rest.split(','), &default.to_string())
                    .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
            }
            "color" => {
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

                if args[0] != "nil"
                    && !args[0]
                        .strip_prefix("0x")
                        .or_else(|| args[0].strip_prefix("0X"))
                        .and_then(|hex| i64::from_str_radix(hex, 16).ok())
                        .or_else(|| args[0].parse::<i64>().ok())
                        .is_some_and(|val| (0..=0xff_ffff).contains(&val))
                {
                    bail!(
                        "{pos}: default value of '{kind}' must be 'nil' or an integer between \
                         0x000000 and 0xffffff, got '{}'",
                        args[0]
                    );
                }
            }
            "value" => {
                if rest.is_none_or(str::is_empty) {
                    bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                }
            }
            "file" | "folder" => {
                if let Some(rest) = rest
                    && !rest.is_empty()
                {
                    tracing::warn!("{pos}: '{kind}' expects 0 arguments, got {}", rest.split(',').count());
                }
            }
            "font" | "figure" | "string" | "text" => {
                let args = args();
                if args.len() != 1 {
                    tracing::warn!("{pos}: '{kind}' expects 1 argument, got {}", args.len());
                }
            }
            "data" => {
                let key = prop.key.trim();
                if !key.parse::<i64>().is_ok_and(|val| (0..=16_000).contains(&val)) {
                    bail!("{pos}: size of '{kind}' must be an integer between 0 and 16000, got '{key}'");
                }
            }
            _ => {}
        }
    }

    Ok(())
}

#[allow(clippy::match_same_arms)]
fn validate_tra_props(props: &[Prop<'_>], modern: bool) -> anyhow::Result<()> {
    for prop in props {
        if prop.var.is_some() {
            continue;
        }

        match prop.kind.to_ascii_lowercase().as_str() {
            "param" if prop.rest.is_none() => {
                if prop.key.trim().parse::<f64>().is_err() {
                    bail!(
                        "{}: default value of 'param' must be a number, got '{}'",
                        prop.pos,
                        prop.key.trim()
                    );
                }
            }
            "speed" => {
                let pos = prop.pos;
                if !matches!(prop.key.trim(), "0" | "1") {
                    bail!(
                        "{pos}: acceleration of 'speed' must be '0' or '1', got '{}'",
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
                    bail!("{pos}: deceleration of 'speed' must be '0' or '1', got '{}'", args[0]);
                }
            }
            _ => {}
        }
    }

    if !modern {
        return Ok(());
    }

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
                "{pos}: 'param' has too many arguments; expected at most 1, got {}",
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
                validate_select(items.iter().copied(), default).map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
            }
            _ => {}
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn validate_shader(src: &Source) -> anyhow::Result<()> {
    let lines = std::iter::once(0)
        .chain(src.text.match_indices('\n').map(|(st, _)| st + 1))
        .collect::<Vec<_>>();

    let locate = |at| {
        let line = lines.partition_point(|&st| st <= at).saturating_sub(1);
        src.positions[line].locate(&src.text[lines[line]..], at - lines[line])
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

                let mut bytes = name.bytes();
                if !bytes
                    .next()
                    .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
                    || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
                {
                    bail!("{pos}: name of '{kind}' must match [A-Za-z_][A-Za-z0-9_]*, got '{name}'");
                }

                if hlsl::parse_builtin(name).is_some() || hlsl::KEYWORDS.contains(&name) {
                    bail!("{pos}: name of '{kind}' cannot be an HLSL keyword or reserved word, got '{name}'");
                }

                if !shaders.insert((kind, name.as_bytes())) {
                    bail!("{pos}: duplicate {kind} definition '{name}'");
                }

                for (span, msg) in validate_cbuffers(body) {
                    let pos = locate(body.as_ptr() as usize - src.text.as_ptr() as usize + span.st);
                    tracing::warn!("{pos}: {msg}");
                }

                if hlsl::find_entry_point(body, name, kind).is_none() {
                    bail!("{pos}: entry point '{name}' is not defined in '{kind}' block");
                }
            }
            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. } | TokenKind::UnclosedComment(_) => {}
            _ => tokens.push(token),
        }
    }

    for (i, token) in tokens.iter().enumerate() {
        let TokenKind::Ident(kind @ ("pixelshader" | "computeshader")) = &token.kind else {
            continue;
        };

        let pos = locate(token.span.st);
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
            continue;
        }

        let name = match (token(1), token(2), token(3)) {
            (Some(TokenKind::String(name)), _, _)
            | (Some(TokenKind::LParen), Some(TokenKind::String(name)), Some(TokenKind::Comma | TokenKind::RParen)) => {
                name
            }
            (Some(TokenKind::LParen | TokenKind::LBrace), _, _) => {
                bail!("{pos}: first argument of '{kind}' must be a string literal");
            }
            _ => continue,
        };

        if !name.contains(&b'@') && !shaders.contains(&(*kind, name.as_ref())) {
            bail!("{pos}: undefined {kind} '{}'", String::from_utf8_lossy(name));
        }
    }

    Ok(())
}

fn validate_track(prop: &Prop<'_>) -> anyhow::Result<()> {
    let pos = prop.pos;
    let kind = prop.kind;
    let args = prop.rest.map_or(Vec::new(), |rest| {
        rest.trim().split(',').map(str::trim).collect::<Vec<_>>()
    });

    if args.len() < 3 {
        bail!("{pos}: '{kind}' requires at least 3 arguments, got {}", args.len());
    }

    if args.len() > 6 {
        tracing::warn!(
            "{pos}: '{kind}' has too many arguments; expected at most 6, got {}",
            args.len()
        );
    }

    let parse = |name: &str, arg: &str| -> anyhow::Result<f64> {
        let Some(value) = arg.parse::<f64>().ok().filter(|v| v.is_finite()) else {
            bail!("{pos}: {name} of '{kind}' must be a number, got '{arg}'");
        };
        Ok(value)
    };

    let min = parse("min", args[0])?;
    let max = parse("max", args[1])?;
    let default = parse("default value", args[2])?;
    let step = args.get(3).map(|arg| parse("step", arg)).transpose()?.unwrap_or(0.1);

    if !(min <= default && default <= max) {
        bail!("{pos}: default value of '{kind}' must satisfy min ({min}) <= default ({default}) <= max ({max})");
    }

    if ![
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
    ]
    .iter()
    .any(|&v| (step - v).abs() < f64::EPSILON)
    {
        bail!("{pos}: step of '{kind}' must be 1, 0.1, ..., 0.000000001, got '{step}'");
    }

    if let Some(&arg) = args.get(5) {
        parse("sensitivity", arg)?;
    }

    Ok(())
}

fn validate_select<'a>(items: impl IntoIterator<Item = &'a str>, default: &str) -> anyhow::Result<()> {
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

fn validate_cbuffers(src: &str) -> Vec<(hlsl::Span, String)> {
    let tokens = hlsl::Lexer::new(src)
        .filter(|token| {
            !matches!(
                token.kind,
                hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment { .. }
            )
        })
        .collect::<Vec<_>>();
    let mut issues = Vec::new();
    let mut curr = 0;
    let mut packing = Packing {
        depth: 0,
        is_row_major: Some(false),
    };
    while curr < tokens.len() {
        if packing.apply(&tokens[curr]).is_some() || tokens[curr].text != "cbuffer" {
            curr += 1;
            continue;
        }

        let span = tokens[curr].span;
        let is_conditional = packing.depth != 0;
        curr += 1;
        let name = tokens.get(curr).map_or("<unnamed>", |token| token.text);
        while curr < tokens.len() && !matches!(tokens[curr].text, "{" | ";") {
            let _ = packing.apply(&tokens[curr]);
            curr += 1;
        }

        if tokens.get(curr).is_none_or(|token| token.text != "{") {
            continue;
        }

        let Some(ed) = hlsl::skip_group(&tokens, curr, '{', '}') else {
            issues.push((span, format!("cannot analyze unclosed cbuffer '{name}'")));
            break;
        };
        let body = &tokens[curr + 1..ed - 1];
        curr = ed;

        if is_conditional || packing.depth != 0 {
            issues.push((
                span,
                format!("cannot determine cbuffer '{name}' layout before HLSL preprocessing"),
            ));
            for token in body {
                let _ = packing.apply(token);
            }
            continue;
        }

        validate_buffer(body, name, &mut packing, &mut issues);
    }
    issues
}

fn validate_buffer(
    tokens: &[hlsl::Token<'_>],
    name: &str,
    packing: &mut Packing,
    issues: &mut Vec<(hlsl::Span, String)>,
) {
    let mut offset = Some(0_usize);
    let mut st = 0;
    for (i, token) in tokens.iter().enumerate() {
        if let Some(directive) = packing.apply(token) {
            if st != i || matches!(directive, "if" | "ifdef" | "ifndef" | "include") {
                if offset.is_some() {
                    issues.push((
                        token.span,
                        format!("cannot determine subsequent offsets in cbuffer '{name}' before HLSL preprocessing"),
                    ));
                }
                offset = None;
            }
            st = i + 1;
        } else if token.text == ";" {
            if st != i && packing.depth == 0 {
                validate_declaration(&tokens[st..i], name, &mut offset, packing.is_row_major, issues);
            }
            st = i + 1;
        }
    }

    if st < tokens.len() && packing.depth == 0 {
        validate_declaration(&tokens[st..], name, &mut offset, packing.is_row_major, issues);
    }
}

fn validate_declaration(
    tokens: &[hlsl::Token<'_>],
    name: &str,
    offset: &mut Option<usize>,
    is_row_major: Option<bool>,
    issues: &mut Vec<(hlsl::Span, String)>,
) {
    let Some(declaration) = hlsl::parse_declaration(tokens) else {
        return;
    };
    let align = |val: usize, size: usize| val.checked_add(size - 1).map(|val| val / size * size);
    let is_known = offset.is_some();
    let is_row_major = declaration.is_row_major.or(is_row_major);
    if !declaration.ty.is_some_and(|ty| {
        hlsl::SCALARS
            .iter()
            .any(|&(scalar, _, is_float)| is_float && scalar == ty.scalar)
    }) {
        issues.push((
            declaration.token.span,
            format!(
                "cbuffer '{name}' uses non-float or unsupported type '{}'",
                declaration.token.text
            ),
        ));
    }

    let layout = declaration.ty.and_then(|ty| match is_row_major {
        Some(is_row_major) => resolve_layout(ty, is_row_major),
        None => resolve_layout(ty, false).filter(|&layout| Some(layout) == resolve_layout(ty, true)),
    });

    for member in &declaration.members {
        let mut layout = layout;
        for len in &member.dimensions {
            layout = layout.zip(*len).and_then(|(layout, len)| {
                Some(Layout {
                    size: align(layout.size, 16)?
                        .checked_mul(len.checked_sub(1)?)?
                        .checked_add(layout.size)?,
                    payload: layout.payload.checked_mul(len)?,
                    alignment: 16,
                })
            });
        }
        let placement = (*offset).zip(layout).and_then(|(st, layout)| {
            let at = align(st, layout.alignment)?;
            let at = if layout.alignment < 16 && at % 16 + layout.size > 16 {
                align(at, 16)?
            } else {
                at
            };
            Some((at.checked_add(layout.size)?, (at - st) + (layout.size - layout.payload)))
        });
        *offset = placement.map(|(ed, _)| ed);
        if let Some((_, bytes)) = placement.filter(|&(_, bytes)| bytes != 0) {
            issues.push((
                member.span,
                format!(
                    "cbuffer '{name}' member '{}' introduces {bytes} bytes of padding ({} {})",
                    member.name,
                    bytes / 4,
                    if bytes / 4 == 1 { "float" } else { "floats" }
                ),
            ));
        }
    }

    if declaration.is_manual {
        *offset = None;
    } else if declaration.has_error || layout.is_none() || (is_known && offset.is_none()) {
        issues.push((
            declaration.token.span,
            format!("cannot determine layout of declaration in cbuffer '{name}'; subsequent offsets are unknown"),
        ));
        *offset = None;
    }
}

fn resolve_layout(ty: hlsl::Type<'_>, is_row_major: bool) -> Option<Layout> {
    let &(_, size, _) = hlsl::SCALARS.iter().find(|&&(scalar, ..)| scalar == ty.scalar)?;
    let (len, width) = if !ty.is_matrix {
        (1, ty.cols)
    } else if is_row_major {
        (ty.rows, ty.cols)
    } else {
        (ty.cols, ty.rows)
    };
    let width = width * size;
    Some(Layout {
        size: (len - 1) * width.div_ceil(16) * 16 + width,
        payload: len * width,
        alignment: if len > 1 { 16 } else { size },
    })
}
