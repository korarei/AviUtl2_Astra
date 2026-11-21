use crate::build::lang::astra::{self, Action, Preprocessor};
use crate::build::lang::lua::{self, TokenKind};
use crate::build::source::{self, Pos};
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub(in crate::build) struct CommentContext<'a, 's> {
    pub prefix: &'a str,
    pub tokens: &'a [lua::Token<'s>],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpandKind {
    Raw,
    Skip,
    LineComment,
    BlockComment,
    ShortString,
    LongString,
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    should_expand_comment: &'a dyn Fn(&lua::Comment<'_>, &CommentContext<'_, '_>) -> anyhow::Result<bool>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    map: source::Map,
    modules: HashMap<PathBuf, Module>,
    scopes: Vec<Scope>,
    replacements: Vec<Arc<str>>,
}

struct BlockState {
    ed: usize,
    kind: ExpandKind,
}

struct PendingLiteral {
    kind: ExpandKind,
    dst_st: usize,
    src_ed: usize,
    edit_offset: Option<usize>,
}

struct ScanState<'a> {
    block: BlockState,
    stmt_tokens: Vec<lua::Token<'a>>,
    depth: usize,
    markers: Vec<Marker<'a>>,
    segments: Vec<(std::ops::Range<usize>, ExpandKind)>,
}

struct LineScan<'a> {
    first: Option<lua::Token<'a>>,
    comment_col: usize,
    has_code: bool,
}

struct Marker<'a> {
    span: lua::Span,
    at_sigil: &'a str,
    body: &'a str,
    kind: astra::Directive<'a>,
}

impl Builder<'_> {
    #[allow(clippy::too_many_lines)]
    fn preprocess(&mut self, map: &source::Map) -> anyhow::Result<()> {
        let src = map.text();
        let lines = map.lines();
        let scope_st = self.map.len();
        let mut includes = Vec::new();

        let locate = |at| map.locate(at);

        let mut lexer = lua::Lexer::new(&src).peekable();
        let mut has_emitted = false;
        let omit_tokens = std::cell::OnceCell::new();
        let mut omit_parser = None;
        let mut omit_ed = 0;
        let mut pending = None;
        let mut pending_literal = None;
        let mut preprocessor = Preprocessor::new();
        let mut state = ScanState {
            block: BlockState {
                ed: 0,
                kind: ExpandKind::Raw,
            },
            stmt_tokens: Vec::new(),
            depth: 0,
            markers: Vec::new(),
            segments: Vec::new(),
        };

        let mut i = 0;
        while lines.get(i).is_some_and(|&st| st < src.len()) {
            let ed = lines.get(i + 1).map_or(src.len(), |st| st - 1);
            if omit_ed != 0 && ed <= omit_ed {
                i += 1;
                continue;
            }

            let st = lines[i].max(omit_ed);

            if state.block.ed <= st
                && let Some((spans, next)) = preprocessor
                    .parse_verbatim(&src, st, |line| {
                        astra::split_sigil(line.trim().strip_prefix("--")?).map(|(_, body)| body)
                    })
                    .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?
            {
                if let Some(pos) = &pending {
                    bail!("{pos}: directive between #replace and its statement");
                }

                for span in spans {
                    if has_emitted {
                        self.map.copy(map, span.start - 1..span.start);
                    }

                    let ed = span.end - usize::from(src[..span.end].ends_with('\n'));
                    self.map.copy(map, span.start..ed);
                    has_emitted = true;
                }

                lexer = lua::Lexer::new_at(&src, next)
                    .map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?
                    .peekable();
                state.stmt_tokens.clear();
                state.depth = 0;
                i = lines.partition_point(|&st| st < next);
                continue;
            }

            let was_blocked = state.block.ed > st;
            if state.depth == 0 && !was_blocked {
                state.stmt_tokens.clear();
            }

            state.markers.clear();
            state.segments.clear();
            let scan = scan_line(&src, st..ed, &mut lexer, &mut state, self.should_expand_comment, map)?;

            let comment = scan.first.as_ref().and_then(|token| match &token.kind {
                TokenKind::Comment(comment) => Some(comment),
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

            let sigil_st = directive.map_or(st + scan.comment_col, |(at_sigil, _)| {
                at_sigil.as_ptr() as usize - src.as_ptr() as usize
            });
            let body_st = directive.map_or(sigil_st, |(_, body)| body.as_ptr() as usize - src.as_ptr() as usize);
            let pos = locate(sigil_st);
            let kind = directive
                .map(|(_, body)| {
                    state
                        .markers
                        .iter()
                        .find(|marker| scan.first.as_ref().is_some_and(|token| token.span == marker.span))
                        .map(|marker| Some(marker.kind))
                        .map_or_else(|| astra::parse_directive(body, is_block), Ok)
                })
                .transpose()
                .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?
                .flatten();
            let action = preprocessor
                .process(kind, self.vars.as_ref(), directive.map_or(st, |_| sigil_st), body_st)
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            let action = match action {
                Action::Replace(val) => {
                    self.replacements.push(val.into());
                    state
                        .markers
                        .retain(|marker| scan.first.as_ref().is_none_or(|token| token.span != marker.span));
                    if !is_block {
                        pending = Some(pos.clone());
                    }
                    Action::Keep
                }
                action => action,
            };

            if matches!(action, Action::Keep) {
                for marker in &state.markers {
                    if let Action::Replace(val) = preprocessor
                        .process(
                            Some(marker.kind),
                            self.vars.as_ref(),
                            marker.at_sigil.as_ptr() as usize - src.as_ptr() as usize,
                            marker.body.as_ptr() as usize - src.as_ptr() as usize,
                        )
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?
                    {
                        self.replacements.push(val.into());
                    }
                }
            }

            let next_line = if is_block && !matches!(action, Action::Keep) {
                scan.first
                    .as_ref()
                    .map_or(i + 1, |token| lines.partition_point(|&st| st < token.span.ed))
            } else {
                i + 1
            };

            if !matches!(action, Action::Keep) {
                state.stmt_tokens.clear();
                state.depth = 0;
            }

            match action {
                Action::Keep => {
                    if has_emitted {
                        self.map.copy(map, lines[i] - 1..lines[i]);
                    }
                    self.emit_line(map, &src, ed, state.block.ed, &state.segments, &mut pending_literal)?;
                    has_emitted = true;
                }
                Action::Omit => {
                    let tokens = omit_tokens
                        .get_or_init(|| lua::Lexer::new(&src).collect::<Result<Vec<_>, _>>())
                        .as_ref()
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    omit_ed = resolve_omit_ed(&src, tokens, &mut omit_parser, ed, &pos, map)?;
                }
                Action::Include { path, is_quoted } => {
                    let file = source::resolve_include(&path, is_quoted, &pos, self.include_dirs)?;
                    let indent = scan.first.as_ref().map_or("", |token| &src[st..token.span.st]);
                    if has_emitted {
                        self.map.copy(map, lines[i] - 1..lines[i]);
                    }
                    self.include_line(map, lines[i], &file, &pos, indent, &mut includes)?;
                    has_emitted = true;
                }
                action => self.apply_action(map, &src, &pos, &action)?,
            }

            i = next_line;
        }

        if let Some(pos) = pending {
            bail!("{pos}: missing statement after #replace");
        }

        preprocessor
            .finish()
            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;

        if !includes.is_empty() {
            self.scopes.push(Scope {
                span: lua::Span {
                    st: scope_st,
                    ed: self.map.len(),
                },
                includes,
            });
        }

        Ok(())
    }

    fn emit_line(
        &mut self,
        map: &source::Map,
        src: &str,
        ed: usize,
        block_ed: usize,
        segments: &[(std::ops::Range<usize>, ExpandKind)],
        pending_literal: &mut Option<PendingLiteral>,
    ) -> anyhow::Result<()> {
        for (index, (span, kind)) in segments.iter().enumerate() {
            match kind {
                ExpandKind::Skip => self.map.copy(map, span.clone()),
                kind => {
                    let escape = match kind {
                        ExpandKind::Raw | ExpandKind::LongString | ExpandKind::BlockComment => astra::Escape::None,
                        ExpandKind::ShortString => astra::Escape::Lua,
                        ExpandKind::LineComment => astra::Escape::Comment,
                        ExpandKind::Skip => unreachable!(),
                    };
                    let expansion = astra::expand(&src[span.clone()], self.vars.as_ref(), escape)
                        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(span.start + err.offset)))?;
                    let edit_offset = expansion.first_expansion_offset().map(|at| span.start + at);
                    let is_tracked = matches!(
                        kind,
                        ExpandKind::ShortString | ExpandKind::LongString | ExpandKind::BlockComment
                    );

                    if index == 0
                        && let Some(mut pending) = pending_literal.take()
                    {
                        if pending.edit_offset.is_none() {
                            pending.edit_offset = edit_offset;
                        }
                        self.map.apply(map, span.clone(), expansion.to_edits(span.start));

                        if pending.src_ed <= ed {
                            if let Some(offset) = pending.edit_offset {
                                let literal = self.map.slice(pending.dst_st..self.map.len());
                                validate_expanded_literal(pending.kind, &literal.text(), &map.locate(offset))?;
                            }
                        } else {
                            *pending_literal = Some(pending);
                        }
                    } else if is_tracked && index + 1 == segments.len() && block_ed > ed {
                        let dst_st = self.map.len();
                        self.map.apply(map, span.clone(), expansion.to_edits(span.start));
                        *pending_literal = Some(PendingLiteral {
                            kind: *kind,
                            dst_st,
                            src_ed: block_ed,
                            edit_offset,
                        });
                    } else {
                        if is_tracked && let Some(offset) = edit_offset {
                            validate_expanded_literal(*kind, &expansion.text, &map.locate(offset))?;
                        }
                        self.map.apply(map, span.clone(), expansion.to_edits(span.start));
                    }
                }
            }
        }

        Ok(())
    }

    fn include_line(
        &mut self,
        map: &source::Map,
        line_st: usize,
        file: &Path,
        pos: &Pos,
        indent: &str,
        includes: &mut Vec<Include>,
    ) -> anyhow::Result<()> {
        let include_st = self.map.len();
        let nested_st = self.scopes.len();
        let is_reused = self.include_file(file, pos)?;

        if self.map.len() == include_st {
            self.map.copy(map, line_st..line_st);
        }

        if !indent.is_empty() {
            self.indent_includes(include_st, nested_st, indent);
        }

        if file
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
        {
            includes.push(Include {
                span: lua::Span {
                    st: include_st,
                    ed: self.map.len(),
                },
                file: file.to_path_buf(),
                padding: indent.len(),
                require: None,
                is_suppressed: is_reused,
            });
        }

        Ok(())
    }

    fn apply_action(&mut self, map: &source::Map, src: &str, pos: &Pos, action: &Action<'_>) -> anyhow::Result<()> {
        match action {
            Action::Keep | Action::Omit | Action::Include { .. } => unreachable!(),
            Action::Drop => {}
            Action::PragmaOnce => {
                let file = map.locate(0).file;
                self.once_files
                    .insert(std::fs::canonicalize(file.as_ref()).unwrap_or_else(|_| file.to_path_buf()));
            }
            Action::Replace(_) => bail!("{pos}: invalid #replace placement"),
            Action::Define { key, val } => {
                if RESERVED_VARIABLES.contains(key) {
                    bail!("{pos}: cannot define reserved variable '{key}'");
                }

                let vars = self.vars.to_mut();
                vars.insert(
                    (*key).to_owned(),
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
                self.vars.to_mut().shift_remove(*key);
            }
        }

        Ok(())
    }

    fn replace_vals(&mut self) -> anyhow::Result<()> {
        if self.replacements.is_empty() {
            return Ok(());
        }

        let src = self.map.text();
        let tokens = lua::Lexer::new(&src)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
        let mut parser = lua::Parser::new(&src, &tokens);
        let mut vals = std::mem::take(&mut self.replacements).into_iter();
        let mut dst = source::Map::default();
        let mut edits = Vec::new();
        let mut curr = 0;

        for token in &tokens {
            let TokenKind::Comment(comment) = &token.kind else {
                continue;
            };

            if astra::split_sigil(comment.content)
                .map(|(_, body)| astra::parse_replace(body))
                .transpose()
                .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(token.span.st)))?
                .flatten()
                .is_none()
            {
                continue;
            }

            let pos = self.map.locate(token.span.st);
            if token.span.st < curr {
                bail!("{pos}: overlapping #replace expressions");
            }

            let val = vals
                .next()
                .ok_or_else(|| anyhow::anyhow!("{pos}: #replace was not resolved during preprocessing"))?;
            let span = find_replace_span(
                &src,
                &tokens,
                &mut parser,
                &pos,
                token,
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

            dst.copy(&self.map, curr..marker.start);
            edits.push((marker.clone(), dst.len()..dst.len()));
            dst.copy(&self.map, marker.end..span.st);

            if token.span.ed == span.st
                && let Some(prev) = src[..token.span.st].chars().next_back()
                && let Some(first) = val.chars().next()
                && ((prev.is_alphanumeric() || prev == '_') && (first.is_alphanumeric() || first == '_')
                    || matches!((prev, first), ('+', '+') | ('-', '-') | ('/' | '*', '/')))
            {
                dst.emit(" ", &self.map, token.span.st..token.span.ed);
            }

            let val_st = dst.len();
            dst.emit(val, &self.map, span.st..span.ed);
            edits.push((span.st..span.ed, val_st..dst.len()));
            curr = span.ed;
        }

        if vals.next().is_some() {
            bail!("unmatched #replace values after preprocessing");
        }

        dst.copy(&self.map, curr..self.map.len());
        drop(parser);
        drop(tokens);
        drop(src);

        let is_removed = |span: lua::Span| {
            let index = edits.partition_point(|(old, _)| old.start <= span.st);
            index.checked_sub(1).is_some_and(|index| edits[index].0.end >= span.ed)
        };

        let shift = |at: usize| {
            let index = edits.partition_point(|(old, _)| old.start <= at);
            index.checked_sub(1).map_or(at, |index| {
                let (old, new) = &edits[index];
                if at < old.end {
                    new.start + (at - old.start).min(new.len())
                } else {
                    new.end + (at - old.end)
                }
            })
        };

        for scope in &mut self.scopes {
            scope.includes.retain(|include| !is_removed(include.span));
            scope.span.st = shift(scope.span.st);
            scope.span.ed = shift(scope.span.ed);
            for include in &mut scope.includes {
                include.span.st = shift(include.span.st);
                include.span.ed = shift(include.span.ed);
            }
        }

        self.scopes.retain(|scope| !scope.includes.is_empty());
        self.map = dst;
        Ok(())
    }

    fn inline_requires(&mut self) -> anyhow::Result<()> {
        if self.scopes.is_empty() {
            for token in lua::Lexer::new(&self.map.text()) {
                token.map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
            }

            return Ok(());
        }

        let mut scopes = std::mem::take(&mut self.scopes);
        self.resolve_requires(&mut scopes, &self.map.text())?;
        let mut scopes = scopes.into_iter();

        while !scopes.as_slice().is_empty() {
            let src = self.map.text();
            let mut prev = None;
            let count = scopes
                .as_slice()
                .iter()
                .take_while(|scope| {
                    if prev.is_some_and(|ed| scope.span.st < ed) {
                        return false;
                    }
                    prev = Some(scope.span.ed);
                    true
                })
                .count();
            let mut dst = source::Map::default();
            let mut edits = Vec::new();
            let mut curr = 0;

            for scope in scopes.by_ref().take(count) {
                let Some(inlined) = self.inline_modules(&scope, &src) else {
                    continue;
                };

                dst.copy(&self.map, curr..scope.span.st);
                dst.copy(&inlined, ..);
                edits.push((scope.span.ed, dst.len()));
                curr = scope.span.ed;
            }

            if edits.is_empty() {
                continue;
            }

            dst.copy(&self.map, curr..self.map.len());
            self.map = dst;

            let shift = |at| {
                let i = edits.partition_point(|&(ed, _)| ed <= at);
                i.checked_sub(1).map_or(at, |i| {
                    let (ed, offset) = edits[i];
                    offset + (at - ed)
                })
            };

            for scope in scopes.as_mut_slice() {
                scope.span.st = shift(scope.span.st);
                scope.span.ed = shift(scope.span.ed);
                for include in &mut scope.includes {
                    include.span.st = shift(include.span.st);
                    include.span.ed = shift(include.span.ed);
                    if let Some(require) = &mut include.require {
                        require.st = shift(require.st);
                        require.ed = shift(require.ed);
                    }
                }
            }
        }

        Ok(())
    }

    fn include_file(&mut self, file: &Path, pos: &Pos) -> anyhow::Result<bool> {
        let key = std::fs::canonicalize(file)
            .with_context(|| format!("{pos}: failed to canonicalize include file '{}'", file.display()))?;

        if self.once_files.contains(&key) {
            if let Some(module) = self.modules.get(&key) {
                let st = self.map.len();
                self.map.copy(&module.map, ..);
                for scope in &module.scopes {
                    let mut scope = scope.clone();
                    scope.span.st += st;
                    scope.span.ed += st;
                    for include in &mut scope.includes {
                        include.span.st += st;
                        include.span.ed += st;
                    }
                    self.scopes.push(scope);
                }
                self.replacements.extend(module.replacements.iter().cloned());
            }

            return Ok(true);
        }

        if self.include_stack.contains(&key) {
            bail!("{pos}: circular include detected: '{}'", file.display());
        }

        if self.include_stack.len() >= 64 {
            bail!("{pos}: include nesting exceeds 64 files");
        }

        let st = self.map.len();
        let scopes_st = self.scopes.len();
        let replacements_st = self.replacements.len();
        self.include_stack.push(key.clone());

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

        if self.once_files.contains(&key)
            && file
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("lua"))
        {
            let mut scopes = self.scopes[scopes_st..].to_vec();
            for scope in &mut scopes {
                scope.span.st -= st;
                scope.span.ed -= st;
                for include in &mut scope.includes {
                    include.span.st -= st;
                    include.span.ed -= st;
                }
            }
            self.modules.insert(
                key,
                Module {
                    map: self.map.slice(st..self.map.len()),
                    scopes,
                    replacements: self.replacements[replacements_st..].to_vec(),
                },
            );
        }

        Ok(false)
    }

    fn resolve_requires(&self, scopes: &mut [Scope], src: &str) -> anyhow::Result<()> {
        let mut entries = scopes
            .iter()
            .enumerate()
            .flat_map(|(scope_pos, scope)| {
                scope
                    .includes
                    .iter()
                    .enumerate()
                    .map(move |(include_pos, include)| IncludeEntry {
                        scope_pos,
                        include_pos,
                        span: include.span,
                        search_ed: scope
                            .includes
                            .get(include_pos + 1)
                            .map_or(scope.span.ed, |next| next.span.st),
                    })
            })
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| {
            (
                entry.span.st,
                std::cmp::Reverse(entry.span.ed),
                std::cmp::Reverse(entry.scope_pos),
            )
        });

        let raw = lua::Lexer::new(src)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {}", self.map.locate(err.offset), err.message))?;
        let mut requires: Vec<Option<(lua::Span, usize)>> = vec![None; entries.len()];
        let mut wrappers = Vec::new();

        for (curr, entry) in entries.iter().enumerate() {
            let Some(stmt_st) = find_next_stmt_st(&raw, entry.span.ed, entry.search_ed) else {
                continue;
            };

            let tokens = build_require_tokens(&raw, &entries, &requires, &wrappers, curr);
            let Some((span, name)) = find_require_call(src, &tokens, &raw, entry, stmt_st, &self.map)? else {
                continue;
            };

            let include = &scopes[entry.scope_pos].includes[entry.include_pos];
            validate_require_target(&self.map.locate(span.st), &include.file, name)?;
            requires[curr] = Some((span, wrappers.len()));
            wrappers.push((
                wrap_tokens(
                    [
                        TokenKind::LParen,
                        TokenKind::Keyword("function"),
                        TokenKind::LParen,
                        TokenKind::RParen,
                    ],
                    span.st,
                ),
                wrap_tokens(
                    [
                        TokenKind::Keyword("end"),
                        TokenKind::RParen,
                        TokenKind::LParen,
                        TokenKind::RParen,
                    ],
                    span.ed,
                ),
            ));
        }

        for (entry, require) in entries.into_iter().zip(requires) {
            scopes[entry.scope_pos].includes[entry.include_pos].require = require.map(|(span, _)| span);
        }

        Ok(())
    }

    fn inline_modules(&self, scope: &Scope, src: &str) -> Option<source::Map> {
        let mut dst = source::Map::default();
        let mut curr = scope.span.st;

        for Include {
            span,
            padding,
            require,
            is_suppressed,
            ..
        } in &scope.includes
        {
            let Some(require) = require else {
                if *is_suppressed {
                    dst.copy(&self.map, curr..span.st);
                    curr = span.ed;
                }

                continue;
            };

            let line_st = src[..require.st].rfind('\n').map_or(0, |at| at + 1);
            let indent = &src[line_st..];
            let indent = &indent[..indent.len() - indent.trim_start().len()];
            let prefix_st = span.ed + usize::from(src[span.ed..require.st].starts_with('\n'));

            dst.copy(&self.map, curr..span.st);
            dst.copy(&self.map, prefix_st..require.st);
            dst.emit("(function()\n", &self.map, require.st..require.ed);

            let body = self
                .map
                .slice(span.st..span.ed)
                .indent(*padding, &format!("{indent}    "));

            dst.copy(&body, ..);
            dst.emit(format!("\n{indent}end)()"), &self.map, require.st..require.ed);

            curr = require.ed;
        }

        if curr == scope.span.st {
            return None;
        }

        dst.copy(&self.map, curr..scope.span.ed);
        Some(dst)
    }

    fn indent_includes(&mut self, src_st: usize, scopes_st: usize, indent: &str) {
        let map = self.map.slice(src_st..self.map.len());
        let lines = map.lines();
        let lines = lines
            .iter()
            .enumerate()
            .filter(|&(i, &st)| st < map.len() && !map.is_blank(st..lines.get(i + 1).copied().unwrap_or(map.len())))
            .map(|(_, st)| src_st + st)
            .collect::<Vec<_>>();

        let shift = |at| at + lines.partition_point(|&line| line < at) * indent.len();
        for scope in &mut self.scopes[scopes_st..] {
            scope.span.st = shift(scope.span.st);
            scope.span.ed = shift(scope.span.ed);
            for include in &mut scope.includes {
                include.span.st = shift(include.span.st);
                include.span.ed = shift(include.span.ed);
                include.padding += indent.len();
            }
        }

        let map = map.indent(0, indent);
        self.map = self.map.slice(0..src_st);
        self.map.copy(&map, ..);
    }
}

struct Module {
    map: source::Map,
    scopes: Vec<Scope>,
    replacements: Vec<Arc<str>>,
}

#[derive(Clone)]
struct Scope {
    span: lua::Span,
    includes: Vec<Include>,
}

#[derive(Clone)]
struct Include {
    span: lua::Span,
    file: PathBuf,
    padding: usize,
    require: Option<lua::Span>,
    is_suppressed: bool,
}

pub(in crate::build) fn build(
    map: &source::Map,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    should_expand_comment: impl Fn(&lua::Comment<'_>, &CommentContext<'_, '_>) -> anyhow::Result<bool>,
) -> anyhow::Result<source::Map> {
    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        should_expand_comment: &should_expand_comment,
        include_stack: std::fs::canonicalize(map.locate(0).file.as_ref()).into_iter().collect(),
        once_files: HashSet::new(),
        map: source::Map::default(),
        modules: HashMap::new(),
        scopes: Vec::new(),
        replacements: Vec::new(),
    };
    builder.preprocess(map)?;
    builder.replace_vals()?;
    builder.inline_requires()?;
    Ok(builder.map)
}

#[derive(Clone, Copy)]
struct IncludeEntry {
    scope_pos: usize,
    include_pos: usize,
    span: lua::Span,
    search_ed: usize,
}

fn scan_line<'a>(
    src: &'a str,
    line: std::ops::Range<usize>,
    lexer: &mut std::iter::Peekable<lua::Lexer<'a>>,
    state: &mut ScanState<'a>,
    should_expand_comment: &dyn Fn(&lua::Comment<'_>, &CommentContext<'_, '_>) -> anyhow::Result<bool>,
    map: &source::Map,
) -> anyhow::Result<LineScan<'a>> {
    let std::ops::Range { start: st, end: ed } = line;
    let line_text = &src[st..ed];
    let mut comment_col = line_text.len() - line_text.trim_start().len();
    let mut next_block = BlockState {
        ed: state.block.ed,
        kind: state.block.kind,
    };
    let mut first = None;
    let mut has_code = false;
    let mut curr = st;

    if state.block.ed > st {
        let prefix_ed = ed.min(state.block.ed);
        state.segments.push((st..prefix_ed, state.block.kind));
        curr = prefix_ed;
    }

    while let Some(token) = lexer.next_if(|token| token.as_ref().map_or(true, |token| token.span.st < ed)) {
        let token = token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
        has_code |= !token.kind.is_trivia();

        let kind = classify_expansion(src, st, &token, &state.stmt_tokens, should_expand_comment, map)?;

        if !token.kind.is_trivia() && state.block.ed <= token.span.st {
            match token.kind {
                TokenKind::LParen | TokenKind::LBracket | TokenKind::LBrace => state.depth += 1,
                TokenKind::RParen | TokenKind::RBracket | TokenKind::RBrace => {
                    state.depth = state.depth.saturating_sub(1);
                }
                _ => {}
            }

            state.stmt_tokens.push(token.clone());
        }

        let tok_st = token.span.st.max(curr);
        if tok_st > curr {
            push_segment(&mut state.segments, curr..tok_st, ExpandKind::Raw);
        }
        let tok_ed = token.span.ed.min(ed);
        if tok_ed > tok_st {
            push_segment(&mut state.segments, tok_st..tok_ed, kind);
            curr = tok_ed;
        }

        if token.span.st >= st
            && let TokenKind::Comment(comment) = &token.kind
            && comment.bracket.is_some()
            && let Some((at_sigil, body)) = astra::split_sigil(comment.content)
            && let Some(kind) =
                astra::parse_replace(body).map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(token.span.st)))?
        {
            state.markers.push(Marker {
                span: token.span,
                at_sigil,
                body,
                kind,
            });
        }

        if token.span.st >= st && matches!(&token.kind, TokenKind::Comment(comment) if comment.bracket.is_none()) {
            comment_col = token.span.st - st;
        }

        if token.span.st >= st && token.span.ed > next_block.ed {
            next_block = BlockState {
                ed: token.span.ed,
                kind,
            };
        }

        if first.is_none() && token.span.st >= st && !matches!(&token.kind, TokenKind::Whitespace | TokenKind::Newline)
        {
            first = Some(token);
        }
    }

    if curr < ed {
        push_segment(&mut state.segments, curr..ed, ExpandKind::Raw);
    }

    state.block = next_block;
    Ok(LineScan {
        first,
        comment_col,
        has_code,
    })
}

fn classify_expansion(
    src: &str,
    st: usize,
    token: &lua::Token<'_>,
    stmt_tokens: &[lua::Token<'_>],
    should_expand_comment: &dyn Fn(&lua::Comment<'_>, &CommentContext<'_, '_>) -> anyhow::Result<bool>,
    map: &source::Map,
) -> anyhow::Result<ExpandKind> {
    Ok(match &token.kind {
        TokenKind::Comment(comment) => {
            let ctx = CommentContext {
                prefix: &src[st..token.span.st.max(st)],
                tokens: stmt_tokens,
            };

            if should_expand_comment(comment, &ctx)
                .with_context(|| format!("{}: failed to classify comment", map.locate(token.span.st)))?
            {
                if comment.bracket.is_none() {
                    ExpandKind::LineComment
                } else {
                    ExpandKind::BlockComment
                }
            } else {
                ExpandKind::Skip
            }
        }
        TokenKind::String(literal) => {
            if literal.bracket.is_none() {
                ExpandKind::ShortString
            } else {
                ExpandKind::LongString
            }
        }
        _ => ExpandKind::Raw,
    })
}

fn resolve_omit_ed<'a>(
    src: &'a str,
    tokens: &'a [lua::Token<'a>],
    parser: &mut Option<lua::Parser<'a, 'a>>,
    ed: usize,
    pos: &Pos,
    map: &source::Map,
) -> anyhow::Result<usize> {
    let stmt = parser
        .get_or_insert_with(|| lua::Parser::new(src, tokens))
        .parse_stmt_at(ed + usize::from(ed < src.len()))
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;

    for token in tokens[tokens.partition_point(|token| token.span.st <= ed)..]
        .iter()
        .take_while(|token| token.span.st < stmt.st)
    {
        if let lua::TokenKind::Comment(comment) = &token.kind
            && astra::split_sigil(comment.content).is_some()
        {
            bail!("{pos}: directive between #omit and its statement");
        }
    }

    Ok(stmt.ed)
}

fn find_replace_span(
    src: &str,
    tokens: &[lua::Token<'_>],
    parser: &mut lua::Parser<'_, '_>,
    pos: &Pos,
    token: &lua::Token<'_>,
    is_block: bool,
    map: &source::Map,
) -> anyhow::Result<lua::Span> {
    if is_block {
        return parser
            .parse_expr_at(token.span.ed)
            .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)));
    }

    let st = tokens.partition_point(|next| next.span.st < token.span.ed);
    let st = st + tokens[st..].iter().take_while(|next| next.kind.is_trivia()).count();
    let stmt = parser
        .parse_stmt_at(tokens.get(st).map_or(src.len(), |next| next.span.st))
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;
    let ed = tokens.partition_point(|next| next.span.st < stmt.ed);
    let output = lua::Parser::new(src, &tokens[st..ed])
        .parse()
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?;
    let assign = output
        .assigns
        .iter()
        .find(|assign| assign.span.st == stmt.st)
        .ok_or_else(|| anyhow::anyhow!("{pos}: #replace must precede a declaration or assignment with one value"))?;

    if assign.targets.len() != 1 || assign.vals.len() != 1 {
        bail!("{pos}: #replace requires one target and one value");
    }

    Ok(assign.vals[0])
}

fn validate_replace_val(pos: &Pos, val: &str) -> anyhow::Result<()> {
    let tokens = lua::Lexer::new(val)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;
    let span = lua::Parser::new(val, &tokens)
        .parse_expr_at(0)
        .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;

    if tokens
        .iter()
        .any(|token| token.span.st >= span.ed && !token.kind.is_trivia())
    {
        bail!("{pos}: #replace value must contain one expression");
    }

    if tokens.iter().any(|token| {
        matches!(&token.kind, TokenKind::Comment(comment) if comment.bracket.is_none())
            && !val[token.span.ed..].contains('\n')
    }) {
        bail!("{pos}: #replace value cannot end with a line comment");
    }

    Ok(())
}

fn find_next_stmt_st(raw: &[lua::Token<'_>], after: usize, search_ed: usize) -> Option<usize> {
    let mut has_content = true;

    for token in raw[raw.partition_point(|token| token.span.st < after)..]
        .iter()
        .take_while(|token| token.span.st < search_ed)
    {
        match token.kind {
            TokenKind::Whitespace => {}
            TokenKind::Newline => {
                if !has_content {
                    break;
                }
                has_content = false;
            }
            TokenKind::Comment(_) => has_content = true,
            _ => return Some(token.span.st),
        }
    }

    None
}

fn build_require_tokens<'a, 't>(
    raw: &'t [lua::Token<'a>],
    entries: &[IncludeEntry],
    requires: &[Option<(lua::Span, usize)>],
    wrappers: &'t [([lua::Token<'static>; 4], [lua::Token<'static>; 4])],
    curr: usize,
) -> Vec<&'t lua::Token<'a>> {
    let mut tokens = Vec::with_capacity(raw.len());
    let mut entry_pos = curr;
    let mut at = 0;

    while let Some(token) = raw.get(at) {
        if let Some(entry) = entries.get(entry_pos)
            && entry.span.st <= token.span.st
        {
            at = raw.partition_point(|token| token.span.st < entry.span.ed);
            entry_pos += 1;
            while entries
                .get(entry_pos)
                .is_some_and(|child| child.span.st < entry.span.ed)
            {
                entry_pos += 1;
            }
            continue;
        }

        tokens.push(token);
        at += 1;
    }

    for entry_pos in (0..curr).rev() {
        let Some((require, wrap_pos)) = requires[entry_pos] else {
            continue;
        };
        let span = entries[entry_pos].span;
        let body_st = tokens
            .iter()
            .position(|token| token.span.st >= span.st && token.span.st < span.ed);
        let body = if let Some(body_st) = body_st {
            let body_ed = body_st
                + tokens[body_st..]
                    .iter()
                    .take_while(|token| token.span.st >= span.st && token.span.st < span.ed)
                    .count();
            tokens.drain(body_st..body_ed).collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let Some(call_st) = tokens.iter().position(|token| token.span.st == require.st) else {
            continue;
        };
        let call_ed = call_st
            + tokens[call_st..]
                .iter()
                .take_while(|token| token.span.st < require.ed)
                .count();
        let (start, end) = &wrappers[wrap_pos];
        tokens.splice(call_st..call_ed, start.iter().chain(body).chain(end));
    }

    tokens
}

fn wrap_tokens(kinds: [TokenKind<'static>; 4], at: usize) -> [lua::Token<'static>; 4] {
    kinds.map(|kind| lua::Token {
        kind,
        span: lua::Span { st: at, ed: at },
    })
}

fn find_require_call<'r>(
    src: &str,
    tokens: &[&lua::Token<'_>],
    raw: &'r [lua::Token<'r>],
    entry: &IncludeEntry,
    stmt_st: usize,
    map: &source::Map,
) -> anyhow::Result<Option<(lua::Span, &'r [u8])>> {
    Ok(lua::Parser::new(src, tokens)
        .parse()
        .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?
        .calls
        .into_iter()
        .filter(|call| {
            call.span.st >= entry.span.ed
                && call.span.ed <= entry.search_ed
                && call.stmt_st == Some(stmt_st)
                && call.reference.as_deref() == Some(&["require"][..])
                && call.args.len() == 1
        })
        .filter_map(|call| {
            let arg = call.args[0];
            let index = raw.partition_point(|token| token.span.st < arg.st);
            let token = raw.get(index)?;
            if token.span != arg {
                return None;
            }

            let TokenKind::String(name) = &token.kind else {
                return None;
            };
            Some((call.span, name.val.as_ref()))
        })
        .min_by_key(|(span, _)| span.st))
}

fn validate_require_target(pos: &Pos, file: &Path, name: &[u8]) -> anyhow::Result<()> {
    let name = std::str::from_utf8(name).map_err(|_| {
        anyhow::anyhow!(
            "{pos}: require module name is not valid UTF-8 after include '{}'",
            file.display()
        )
    })?;
    let stem = name.rsplit(['.', '/', '\\']).next().unwrap_or(name);
    let file_stem = file.file_stem().and_then(|name| name.to_str());
    if file_stem != Some(stem)
        && !(file_stem == Some("init")
            && file
                .parent()
                .and_then(|dir| dir.file_name())
                .and_then(|name| name.to_str())
                == Some(stem))
    {
        bail!(
            "{pos}: require module '{name}' does not match included file '{}'",
            file.display()
        );
    }

    Ok(())
}

fn validate_expanded_literal(kind: ExpandKind, text: &str, pos: &Pos) -> anyhow::Result<()> {
    let label = if matches!(kind, ExpandKind::BlockComment) {
        "block comment"
    } else {
        "string literal"
    };
    let mut lexer = lua::Lexer::new(text);
    let Some(Ok(token)) = lexer.next() else {
        bail!("{pos}: expanded placeholder breaks {label}");
    };
    let is_valid = match kind {
        ExpandKind::ShortString => matches!(&token.kind, TokenKind::String(literal) if literal.bracket.is_none()),
        ExpandKind::LongString => matches!(&token.kind, TokenKind::String(literal) if literal.bracket.is_some()),
        ExpandKind::BlockComment => matches!(&token.kind, TokenKind::Comment(comment) if comment.bracket.is_some()),
        ExpandKind::Raw | ExpandKind::Skip | ExpandKind::LineComment => unreachable!(),
    };

    if !is_valid || token.span.st != 0 || token.span.ed != text.len() || lexer.next().is_some() {
        bail!("{pos}: expanded placeholder breaks {label}");
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

    if kind == ExpandKind::Raw
        && let Some((prev, ExpandKind::Raw)) = segments.last_mut()
        && prev.end == span.start
    {
        prev.end = span.end;
    } else {
        segments.push((span, kind));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_block_controls() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;

        for body in [
            "if false",
            "if",
            "elif",
            "else",
            "endif",
            "omit",
            "include \"missing.lua\"",
            "pragma once",
            "if false\n${MISSING}\n--#include \"missing.lua\"\n",
        ] {
            let src = format!("--[[#{body}]]\nreturn 1\n");
            let map = source::Map::new(
                src.as_str(),
                Pos {
                    file: Path::new("test.lua").into(),
                    line: 1,
                    col: 1,
                },
            );
            let output = build(&map, &target, &[], &IndexMap::new(), |_, _| Ok(false))?;
            assert_eq!(output.text(), src.trim_suffix('\n'), "{body}");
        }

        Ok(())
    }

    #[test]
    fn preserves_verbatim_and_skips_comments() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;
        let src = "--#define VALUE changed\n-- ${UNTOUCHED}\n--#verbatim\n--[[pixelshader@${NAME}:\n//#undef VALUE\nfloat value = ${MISSING};\n]]\n--#endverbatim\nreturn \"${VALUE}\"\n";
        let map = source::Map::new(
            src,
            Pos {
                file: Path::new("included.lua").into(),
                line: 5,
                col: 1,
            },
        );
        let output = build(&map, &target, &[], &IndexMap::new(), |_, _| Ok(false))?;
        let text = output.text();
        assert!(text.starts_with("-- ${UNTOUCHED}\n--[[pixelshader@${NAME}:\n"));
        assert!(text.contains("float value = ${MISSING};"));
        assert!(text.ends_with("return \"changed\""));
        let pos = output.locate(text.find("${MISSING}").unwrap());
        assert_eq!(pos.to_string(), "included.lua:10:15");
        Ok(())
    }

    #[test]
    fn reports_errors_in_sliced_input() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;
        let map = source::Map::new(
            "@header\n-- ${MISSING}\n",
            Pos {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );
        let error = build(&map.slice(8..map.len()), &target, &[], &IndexMap::new(), |_, _| {
            Ok(true)
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "test.lua:2:4: variable 'MISSING' not found");
        Ok(())
    }

    #[test]
    fn propagates_comment_errors() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;

        for comment in ["--invalid", "--[[invalid]]"] {
            let map = source::Map::new(
                format!("@header\n  {comment}\n"),
                Pos {
                    file: Path::new("included.lua").into(),
                    line: 5,
                    col: 1,
                },
            );
            let error = build(&map.slice(8..), &target, &[], &IndexMap::new(), |comment, _| {
                assert_eq!(comment.content, "invalid");
                Err(std::io::Error::other("invalid comment").into())
            })
            .unwrap_err();
            assert_eq!(error.to_string(), "included.lua:6:3: failed to classify comment");
            assert_eq!(
                error.downcast_ref::<std::io::Error>().unwrap().to_string(),
                "invalid comment"
            );
        }

        Ok(())
    }

    #[test]
    fn expands_selected_comments_and_escapes_short_strings() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;
        let vars = IndexMap::from([("TEXT".to_owned(), "a\tb\r\nc\x1b".to_owned())]);
        let src = concat!(
            "--keep:${TEXT}\n",
            "-- ${MISSING}\n",
            "--[[${MISSING}]]\n",
            "local short = \"${TEXT}\"\n",
            "local long = [[${TEXT}]]\n",
        );
        let map = source::Map::new(
            src,
            Pos {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );
        let output = build(&map, &target, &[], &vars, |comment, _| {
            Ok(comment.bracket.is_none() && comment.content.starts_with("keep:"))
        })?;
        assert_eq!(
            output.text(),
            concat!(
                "--keep:a\\tb\\nc\\x1b\n",
                "-- ${MISSING}\n",
                "--[[${MISSING}]]\n",
                "local short = \"a\\tb\\nc\\027\"\n",
                "local long = [[a\tb\nc\x1b]]",
            )
        );
        Ok(())
    }

    #[test]
    fn rejects_broken_literal_boundaries() -> anyhow::Result<()> {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "test.lua" }))?;
        let make_map = |src| {
            source::Map::new(
                src,
                Pos {
                    file: Path::new("test.lua").into(),
                    line: 1,
                    col: 1,
                },
            )
        };

        let ok_vars = IndexMap::from([
            ("ESCAPED".to_owned(), r#"a\nb\"c"#.to_owned()),
            ("BRACKET".to_owned(), "a]]b".to_owned()),
        ]);
        let ok_src = "local a = \"${ESCAPED}\"\nlocal b = [=[${BRACKET}]=]\n";
        let output = build(&make_map(ok_src), &target, &[], &ok_vars, |_, _| Ok(false))?;
        assert_eq!(output.text(), "local a = \"a\\nb\\\"c\"\nlocal b = [=[a]]b]=]");

        let bad_vars = IndexMap::from([
            ("QUOTE".to_owned(), r#"a"b"#.to_owned()),
            ("INJECT_SHORT".to_owned(), r#"a" .. "b"#.to_owned()),
            ("SLASH".to_owned(), r"a\".to_owned()),
            ("CLOSE".to_owned(), "a]]b".to_owned()),
            ("INJECT_LONG".to_owned(), "a]] .. [[b".to_owned()),
        ]);

        for (src, should_expand) in [
            ("local s = \"${QUOTE}\"", false),
            ("local s = \"${INJECT_SHORT}\"", false),
            ("local s = \"${SLASH}\"", false),
            ("local s = \"first\\\n${QUOTE}\"", false),
            ("local s = [[${CLOSE}]]", false),
            ("local s = [[first\n${INJECT_LONG}\nlast]]", false),
            ("--[[first\n${CLOSE}\nlast]]", true),
        ] {
            let err = build(&make_map(src), &target, &[], &bad_vars, |_, _| Ok(should_expand)).unwrap_err();
            assert!(err.to_string().contains("breaks"), "{src}: {err}");
        }

        Ok(())
    }
}
