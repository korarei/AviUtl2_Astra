use super::lua::{self, TokenKind};
use super::preprocess;
use super::source::{self, Position};
use super::validate;
use crate::config::{BuildTarget, RESERVED_VARIABLES};
use anyhow::{Context as _, bail};
use indexmap::{IndexMap, IndexSet};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

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

pub(super) struct Property<'a> {
    pub(super) pos: Position,
    pub(super) kind: &'a str,
    pub(super) var: Option<&'a str>,
    pub(super) key: &'a str,
    pub(super) rest: Option<&'a str>,
}

impl Property<'_> {
    pub(super) fn is_track(&self) -> bool {
        self.kind.starts_with("track") && self.kind != "trackgroup"
    }

    pub(super) fn is_check(&self) -> bool {
        self.kind.starts_with("check") && self.kind != "checksection"
    }
}

pub(super) struct Shader<'a> {
    pub(super) pos: Position,
    pub(super) kind: &'a str,
    pub(super) name: &'a str,
    pub(super) body: &'a str,
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    is_bundled: bool,
    suffix: &'a str,
    include_stack: Vec<PathBuf>,
    map: source::Map,
    scopes: Vec<Scope>,
    repls: Vec<String>,
}

impl Builder<'_> {
    fn build(mut self, src: &str) -> anyhow::Result<Output> {
        self.process(src, Path::new(self.target.path()))?;

        self.replace_vals()?;

        self.inline_requires()?;

        let map = self.normalize_header()?;

        let map = if self.suffix.ends_with('2') && !self.suffix.eq_ignore_ascii_case(".tra2") {
            Self::normalize_props(&map)?
        } else {
            map
        };

        let src = map.text();
        let defs = self.extract_defs(&map, &src)?;
        let props = &defs.props;
        let l10n = if self.suffix.eq_ignore_ascii_case(".tra2") {
            validate::script::validate_tra_props(props, true)?;
            self.resolve_l10n(props)?
        } else if self.suffix.ends_with('2') {
            let mut vars = HashSet::new();
            validate::script::validate_compat_props(props, &mut vars)?;
            validate::script::validate_modern_props(props, &mut vars)?;
            validate::script::validate_shader(&map, &src, &defs.shaders, &defs.tokens)?;
            self.resolve_l10n(props)?
        } else if self.suffix.eq_ignore_ascii_case(".tra") {
            validate::script::validate_tra_props(props, false)?;
            String::new()
        } else {
            let mut vars = HashSet::new();
            validate::script::validate_legacy_props(props, self.suffix)?;
            validate::script::validate_compat_props(props, &mut vars)?;
            String::new()
        };

        Ok(Output {
            l10n,
            script: src.into_owned(),
        })
    }

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
        let lines = map.lines();
        let scope_st = self.map.len();
        let mut includes = Vec::new();

        let locate = |at| map.locate(at);

        let header_ed = locate_header(src);
        if header_ed != 0 {
            let ed = header_ed - usize::from(src[..header_ed].ends_with('\n'));
            let expanded = preprocess::expand(&src[..ed], self.vars.as_ref())
                .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
            expanded.append(&mut self.map, &map, 0..ed);
        }

        let mut lexer = lua::Lexer::new(src);
        lexer.curr = header_ed;
        let mut lexer = lexer.peekable();
        let mut has_emitted = header_ed != 0;
        let omit_tokens = std::cell::OnceCell::new();
        let mut omit_parser = None;
        let mut omit_ed = 0;
        let mut blocked_ed = 0;
        let mut repl = None;
        let mut parser = preprocess::Parser::new(preprocess::Lexer::new(src));

        let mut i = lines.partition_point(|&st| st < header_ed);
        while lines.get(i).is_some_and(|&st| st < src.len()) {
            let ed = lines.get(i + 1).map_or(src.len(), |st| st - 1);
            if omit_ed != 0 && ed <= omit_ed {
                i += 1;
                continue;
            }

            let st = lines[i].max(omit_ed);
            let line = &src[st..ed];
            let mut at = line.len() - line.trim_start().len();

            let mut blocked = blocked_ed;
            let mut first = None;
            let mut shader = None;
            let mut repls = Vec::new();
            let mut has_code = false;

            while let Some(token) = lexer.next_if(|token| token.as_ref().map_or(true, |token| token.span.st < ed)) {
                let token = token.map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;
                has_code |= !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                );

                if token.span.st >= st
                    && let TokenKind::Comment(comment) = &token.kind
                    && comment.bracket.is_some()
                    && comment.content.split_whitespace().next() == Some("#replace")
                    && let Some(preprocess::Directive::Replace(_)) =
                        preprocess::Parser::parse_directive(comment.content.trim_start(), true)
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(token.span.st)))?
                {
                    repls.push((token.span, comment.content.trim_start()));
                }

                if let TokenKind::Comment(comment) = &token.kind
                    && let Some(bracket) = comment.bracket
                    && bracket.level == 0
                    && let Some((kind, rest)) = comment.content.split_once('@')
                    && matches!(kind.trim_end(), "pixelshader" | "computeshader")
                    && let Some((_, body)) = rest.split_once(':')
                    && token.span.st >= st
                {
                    if !src[st..token.span.st].trim().is_empty() {
                        bail!("{}: shader block must start on its own line", locate(token.span.st));
                    }

                    shader = Some((
                        token.span,
                        body.as_ptr() as usize - src.as_ptr() as usize..bracket.span.ed,
                    ));
                }

                if token.span.st >= st
                    && matches!(&token.kind, TokenKind::Comment(comment) if comment.bracket.is_none())
                {
                    at = token.span.st - st;
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

            let comment = first.as_ref().and_then(|token| match &token.kind {
                TokenKind::Comment(comment) => Some(comment),
                _ => None,
            });
            let is_block = comment.is_some_and(|comment| comment.bracket.is_some());
            let directive = if blocked_ed > st {
                None
            } else {
                comment
                    .filter(|comment| comment.content.starts_with('#'))
                    .map(|comment| comment.content)
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

            let pos = locate(directive.map_or(st + at, |content| content.as_ptr() as usize - src.as_ptr() as usize));
            let action = parser.process(
                directive.map(|content| (content, is_block)),
                self.vars.as_ref(),
                &pos,
                self.include_dirs,
            )?;
            let action = match action {
                preprocess::Action::Replace(val) => {
                    self.repls.push(val);
                    repls.retain(|(span, _)| first.as_ref().is_none_or(|token| token.span != *span));
                    if !is_block {
                        repl = Some(pos.clone());
                    }
                    preprocess::Action::Keep
                }
                action => action,
            };

            if matches!(action, preprocess::Action::Keep) {
                for (span, content) in repls {
                    if let preprocess::Action::Replace(val) = parser.process(
                        Some((content, true)),
                        self.vars.as_ref(),
                        &locate(span.st),
                        self.include_dirs,
                    )? {
                        self.repls.push(val);
                    }
                }
            }

            let next_line = if is_block && !matches!(&action, preprocess::Action::Keep) {
                first
                    .as_ref()
                    .map_or(i + 1, |token| lines.partition_point(|&st| st < token.span.ed))
            } else {
                i + 1
            };

            match &action {
                preprocess::Action::Keep => {
                    if let Some((span, body)) = shader {
                        let body_st = body.start;
                        let body_ed = body.end;
                        let prefix = preprocess::expand(&src[st..body_st], self.vars.as_ref())
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(st + err.offset)))?;
                        let body = Self::normalize_shader(&map.slice(body))?;
                        let nested = super::shader::Builder::build(
                            &body,
                            self.target,
                            self.include_dirs,
                            &mut self.vars,
                            &mut self.include_stack,
                        )?;
                        let nested = nested.slice(0..nested.len() - usize::from(nested.ends_with('\n')));

                        if has_emitted {
                            self.map.append(&map, lines[i] - 1..lines[i]);
                        }

                        prefix.append(&mut self.map, &map, st..body_st);
                        self.map.append(&nested, 0..nested.len());

                        let line_ed = lines.partition_point(|&st| st <= span.ed).saturating_sub(1);
                        if body.ends_with('\n') && !nested.ends_with('\n') {
                            self.map.append(&map, body_ed - 1..body_ed);
                        }

                        let suffix_ed = lines.get(line_ed + 1).map_or(src.len(), |st| st - 1);
                        let suffix = preprocess::expand(&src[body_ed..suffix_ed], self.vars.as_ref())
                            .map_err(|err| anyhow::anyhow!("{}: {err}", locate(body_ed + err.offset)))?;
                        suffix.append(&mut self.map, &map, body_ed..suffix_ed);

                        has_emitted = true;
                        i = line_ed + 1;
                        continue;
                    }

                    if has_emitted {
                        self.map.append(&map, lines[i] - 1..lines[i]);
                    }

                    let expanded = preprocess::expand(line, self.vars.as_ref())
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(st + err.offset)))?;
                    expanded.append(&mut self.map, &map, st..ed);
                    has_emitted = true;
                }
                preprocess::Action::Omit => {
                    let tokens = omit_tokens
                        .get_or_init(|| {
                            let mut lexer = lua::Lexer::new(src);
                            lexer.curr = header_ed;
                            lexer.collect::<Result<Vec<_>, _>>()
                        })
                        .as_ref()
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    let stmt = omit_parser
                        .get_or_insert_with(|| lua::Parser::new(tokens))
                        .parse_stmt(ed + usize::from(ed < src.len()))
                        .map_err(|err| anyhow::anyhow!("{}: {err}", locate(err.offset)))?;
                    for token in tokens[tokens.partition_point(|token| token.span.st <= ed)..]
                        .iter()
                        .take_while(|token| token.span.st < stmt.st)
                    {
                        if let lua::TokenKind::Comment(comment) = &token.kind
                            && comment.content.trim_start().starts_with('#')
                        {
                            bail!("{pos}: directive between #omit and its statement");
                        }
                    }
                    omit_ed = stmt.ed;
                }
                preprocess::Action::Drop | preprocess::Action::PragmaOnce => {}
                preprocess::Action::Replace(_) => bail!("{pos}: invalid #replace placement"),
                preprocess::Action::Define(key, val) => {
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
                preprocess::Action::Undef(key) => {
                    self.vars.to_mut().shift_remove(*key);
                }
                preprocess::Action::Include(file) => {
                    if has_emitted {
                        self.map.append(&map, lines[i] - 1..lines[i]);
                    }

                    let include_st = self.map.len();
                    let nested_st = self.scopes.len();
                    self.include(file, &pos)?;

                    if self.map.len() == include_st {
                        self.map.append(&map, lines[i]..lines[i]);
                    }

                    let indent = first.as_ref().map_or("", |token| &line[..token.span.st - st]);
                    if !indent.is_empty() {
                        self.indent(include_st, nested_st, indent);
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
                            file: file.clone(),
                            padding: indent.len(),
                            require: None,
                        });
                    }

                    has_emitted = true;
                }
            }

            i = next_line;
        }

        if let Some(pos) = repl {
            bail!("{pos}: missing statement after #replace");
        }

        parser.finish()?;

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

    #[allow(clippy::too_many_lines)]
    fn replace_vals(&mut self) -> anyhow::Result<()> {
        if self.repls.is_empty() {
            return Ok(());
        }

        let src = self.map.text();
        let mut lexer = lua::Lexer::new(&src);
        lexer.curr = locate_header(&src);
        let tokens = lexer
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
        let mut parser = lua::Parser::new(&tokens);
        let mut vals = std::mem::take(&mut self.repls).into_iter();
        let mut result = source::Map::default();
        let mut edits = Vec::new();
        let mut curr = 0;

        for token in &tokens {
            let TokenKind::Comment(comment) = &token.kind else {
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
                let st = tokens.partition_point(|next| next.span.st < token.span.ed);
                let st = st
                    + tokens[st..]
                        .iter()
                        .take_while(|next| {
                            matches!(
                                next.kind,
                                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                            )
                        })
                        .count();
                let stmt = parser
                    .parse_stmt(tokens.get(st).map_or(src.len(), |next| next.span.st))
                    .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
                let ed = tokens.partition_point(|next| next.span.st < stmt.ed);
                let output = lua::Parser::new(&tokens[st..ed])
                    .parse()
                    .map_err(|err| anyhow::anyhow!("{}: {err}", self.map.locate(err.offset)))?;
                let assign = output
                    .assigns
                    .iter()
                    .find(|assign| assign.span.st == stmt.st)
                    .ok_or_else(|| {
                        anyhow::anyhow!("{pos}: #replace must precede a declaration or assignment with one value")
                    })?;

                if assign.targets.len() != 1 || assign.vals.len() != 1 {
                    bail!("{pos}: #replace requires one target and one value");
                }

                assign.vals[0]
            };

            let val_tokens = lua::Lexer::new(&val)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;
            let val_span = lua::Parser::new(&val_tokens)
                .parse_expr(0)
                .map_err(|err| anyhow::anyhow!("{pos}: invalid #replace value: {err}"))?;

            if val_tokens.iter().any(|token| {
                token.span.st >= val_span.ed
                    && !matches!(
                        token.kind,
                        TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                    )
            }) {
                bail!("{pos}: #replace value must contain one expression");
            }

            if val_tokens.iter().any(|token| {
                matches!(&token.kind, TokenKind::Comment(comment) if comment.bracket.is_none())
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
            edits.push((marker.clone(), result.len()..result.len()));
            result.append(&self.map, marker.end..span.st);

            if token.span.ed == span.st
                && let Some(prev) = src[..token.span.st].chars().next_back()
                && let Some(first) = val.chars().next()
                && ((prev.is_alphanumeric() || prev == '_') && (first.is_alphanumeric() || first == '_')
                    || matches!((prev, first), ('+', '+') | ('-', '-') | ('/' | '*', '/')))
            {
                result.generate(" ", &self.map, token.span.st..token.span.ed);
            }

            let val_st = result.len();
            result.generate(val, &self.map, span.st..span.ed);
            edits.push((span.st..span.ed, val_st..result.len()));
            curr = span.ed;
        }

        if vals.next().is_some() {
            bail!("unmatched #replace values after preprocessing");
        }

        result.append(&self.map, curr..self.map.len());
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
        self.map = result;
        Ok(())
    }

    fn inline_modules(&self, scope: &Scope, src: &str) -> Option<source::Map> {
        let mut result = source::Map::default();
        let mut curr = scope.span.st;

        for Include {
            span, padding, require, ..
        } in &scope.includes
        {
            let Some(require) = require else {
                continue;
            };

            let line_st = src[..require.st].rfind('\n').map_or(0, |at| at + 1);
            let indent = &src[line_st..];
            let indent = &indent[..indent.bytes().take_while(|&byte| matches!(byte, b' ' | b'\t')).count()];
            let prefix_st = span.ed + usize::from(src[span.ed..require.st].starts_with('\n'));

            result.append(&self.map, curr..span.st);
            result.append(&self.map, prefix_st..require.st);
            result.generate("(function()\n", &self.map, require.st..require.ed);

            let body = self
                .map
                .slice(span.st..span.ed)
                .indent(*padding, &format!("{indent}    "));

            result.append(&body, 0..body.len());
            result.generate(format!("\n{indent}end)()"), &self.map, require.st..require.ed);

            curr = require.ed;
        }

        if curr == scope.span.st {
            return None;
        }

        result.append(&self.map, curr..scope.span.ed);
        Some(result)
    }

    fn indent(&mut self, src_st: usize, scopes_st: usize, indent: &str) {
        let map = self.map.slice(src_st..self.map.len());
        let starts = map.lines();
        let lines = starts
            .iter()
            .enumerate()
            .filter(|&(i, &st)| st < map.len() && !map.is_blank(st..starts.get(i + 1).copied().unwrap_or(map.len())))
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

        let indented = map.indent(0, indent);
        self.map = self.map.slice(0..src_st);
        self.map.append(&indented, 0..indented.len());
    }

    #[allow(clippy::too_many_lines)]
    fn resolve_requires(&self, scopes: &mut [Scope], src: &str) -> anyhow::Result<()> {
        let mut includes = scopes
            .iter()
            .enumerate()
            .flat_map(|(scope_idx, scope)| {
                scope.includes.iter().enumerate().map(move |(index, include)| {
                    (
                        scope_idx,
                        index,
                        include.span,
                        scope.includes.get(index + 1).map_or(scope.span.ed, |next| next.span.st),
                    )
                })
            })
            .collect::<Vec<_>>();
        includes.sort_by_key(|&(scope, _, span, _)| (span.st, std::cmp::Reverse(span.ed), std::cmp::Reverse(scope)));
        let mut lexer = lua::Lexer::new(src);
        lexer.curr = locate_header(src);
        let raw = lexer
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {}", self.map.locate(err.offset), err.message))?;
        let mut starts = includes
            .iter()
            .map(|&(_, _, span, _)| {
                [
                    TokenKind::LParen,
                    TokenKind::Keyword("function"),
                    TokenKind::LParen,
                    TokenKind::RParen,
                ]
                .map(|kind| lua::Token {
                    kind,
                    span: lua::Span {
                        st: span.st,
                        ed: span.st,
                    },
                })
            })
            .collect::<Vec<_>>();
        let mut ends = includes
            .iter()
            .map(|&(_, _, span, _)| {
                [
                    TokenKind::Keyword("end"),
                    TokenKind::RParen,
                    TokenKind::LParen,
                    TokenKind::RParen,
                ]
                .map(|kind| lua::Token {
                    kind,
                    span: lua::Span {
                        st: span.ed,
                        ed: span.ed,
                    },
                })
            })
            .collect::<Vec<_>>();
        let mut requires: Vec<Option<lua::Span>> = vec![None; includes.len()];

        for curr in 0..includes.len() {
            let (scope, include_index, span, search_ed) = includes[curr];
            let mut has_content = true;
            let mut stmt_st = None;

            for token in raw[raw.partition_point(|token| token.span.st < span.ed)..]
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
                    _ => {
                        stmt_st = Some(token.span.st);
                        break;
                    }
                }
            }

            let Some(stmt_st) = stmt_st else { continue };
            let mut tokens = Vec::new();
            let mut index = curr;
            let mut at = 0;

            while let Some(token) = raw.get(at) {
                if let Some(&(_, _, span, _)) = includes.get(index)
                    && span.st <= token.span.st
                {
                    at = raw.partition_point(|token| token.span.st < span.ed);
                    index += 1;
                    while includes.get(index).is_some_and(|&(_, _, child, _)| child.st < span.ed) {
                        index += 1;
                    }
                    continue;
                }

                tokens.push(token);
                at += 1;
            }

            for index in (0..curr).rev() {
                let Some(require) = requires[index] else { continue };
                let span = includes[index].2;
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
                tokens.splice(call_st..call_ed, starts[index].iter().chain(body).chain(&ends[index]));
            }

            let include = &scopes[scope].includes[include_index];
            let require = lua::Parser::new(tokens.iter().copied())
                .parse()
                .map_err(|err| anyhow::anyhow!("{}: {}", self.map.locate(err.offset), err.message))?
                .calls
                .into_iter()
                .filter(|call| {
                    call.span.st >= span.ed
                        && call.span.ed <= search_ed
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
                    Some((call.span, name))
                })
                .min_by_key(|(span, _)| span.st);
            let Some((span, name)) = require else { continue };
            let pos = self.map.locate(span.st);
            let name = std::str::from_utf8(&name.val).map_err(|_| {
                anyhow::anyhow!(
                    "{pos}: require module name is not valid UTF-8 after include '{}'",
                    include.file.display()
                )
            })?;
            let stem = name.rsplit(['.', '/', '\\']).next().unwrap_or(name);
            let file_stem = include.file.file_stem().and_then(|name| name.to_str());
            if file_stem != Some(stem)
                && !(file_stem == Some("init")
                    && include
                        .file
                        .parent()
                        .and_then(|dir| dir.file_name())
                        .and_then(|name| name.to_str())
                        == Some(stem))
            {
                bail!(
                    "{pos}: require module '{name}' does not match included file '{}'",
                    include.file.display()
                );
            }
            requires[curr] = Some(span);
            for token in &mut starts[curr] {
                token.span = lua::Span {
                    st: span.st,
                    ed: span.st,
                };
            }

            for token in &mut ends[curr] {
                token.span = lua::Span {
                    st: span.ed,
                    ed: span.ed,
                };
            }
        }

        for ((scope, index, _, _), require) in includes.into_iter().zip(requires) {
            scopes[scope].includes[index].require = require;
        }

        Ok(())
    }

    fn inline_requires(&mut self) -> anyhow::Result<()> {
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
            let mut map = source::Map::default();
            let mut edits = Vec::new();
            let mut curr = 0;

            for scope in scopes.by_ref().take(count) {
                let Some(inlined) = self.inline_modules(&scope, &src) else {
                    continue;
                };

                map.append(&self.map, curr..scope.span.st);
                map.append(&inlined, 0..inlined.len());
                edits.push((scope.span.ed, map.len()));
                curr = scope.span.ed;
            }

            if edits.is_empty() {
                continue;
            }

            map.append(&self.map, curr..self.map.len());
            self.map = map;

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

    fn include(&mut self, file: &Path, pos: &Position) -> anyhow::Result<()> {
        let key = std::fs::canonicalize(file)
            .with_context(|| format!("{pos}: failed to canonicalize include file '{}'", file.display()))?;

        if self.include_stack.contains(&key) {
            bail!("{pos}: circular include detected: '{}'", file.display());
        }

        if self.include_stack.len() >= 64 {
            bail!("{pos}: include nesting exceeds 64 files");
        }

        self.include_stack.push(key);

        let result = crate::fs::read_file(file, self.target.encoding())
            .with_context(|| format!("{pos}: failed to read include file '{}'", file.display()))
            .and_then(|src| self.process(&src, file));

        let _ = self.include_stack.pop();

        result
    }

    fn normalize_shader(map: &source::Map) -> anyhow::Result<source::Map> {
        let text = map.text();
        let text = text.as_ref();
        let mut result = source::Map::default();
        let mut curr = 0;
        let mut lexer_st = 0;
        let mut lexer = super::hlsl::Lexer::new(text);

        while let Some(token) = lexer.next() {
            let token =
                token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(lexer_st + err.offset), err.message))?;
            let st = lexer_st + token.span.st;
            if token.kind != super::hlsl::TokenKind::Other('-')
                || !text[st..].starts_with("--")
                || !text[..st].rsplit('\n').next().unwrap().trim().is_empty()
            {
                continue;
            }

            if text[st..].starts_with("--#") {
                result.append(map, curr..st);
                result.generate("//", map, st..st + 2);
                curr = st + 2;
                lexer_st = st + text[st..].find('\n').map_or(text.len() - st, |at| at + 1);
                lexer = super::hlsl::Lexer::new(&text[lexer_st..]);
            } else if text[st..].starts_with("--[") {
                let token = lua::Lexer::new(&text[st..])
                    .next()
                    .unwrap()
                    .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(st + err.offset), err.message))?;
                let TokenKind::Comment(comment) = token.kind else {
                    continue;
                };
                let Some(bracket) = comment.bracket.filter(|_| comment.content.starts_with('#')) else {
                    continue;
                };

                if let Some(at) = comment.content.find("*/") {
                    bail!(
                        "{}: Lua directive contains an HLSL block comment terminator",
                        map.locate(st + bracket.span.st + at)
                    );
                }

                result.append(map, curr..st);
                result.generate(
                    format!("/*{}", " ".repeat(bracket.span.st - 2)),
                    map,
                    st..st + bracket.span.st,
                );
                result.append(map, st + bracket.span.st..st + bracket.span.ed);
                result.generate(
                    format!("*/{}", " ".repeat(token.span.ed - bracket.span.ed - 2)),
                    map,
                    st + bracket.span.ed..st + token.span.ed,
                );
                curr = st + token.span.ed;
                lexer_st = curr;
                lexer = super::hlsl::Lexer::new(&text[lexer_st..]);
            }
        }

        result.append(map, curr..map.len());
        Ok(result)
    }

    fn normalize_header(&mut self) -> anyhow::Result<source::Map> {
        let map = std::mem::take(&mut self.map);
        let text = map.text();
        let st = text.len() - text.trim_start().len();
        let text = text.trim();
        if text.is_empty() {
            return Ok(map.slice(0..0));
        }

        let map = map.slice(st..st + text.len());
        let first_ed = text.find('\n').unwrap_or(text.len());
        let first = &text[..first_ed];
        let mut result = source::Map::default();
        let mut body_st = 0;

        if first.starts_with('@') {
            if !self.is_bundled {
                bail!("{}: single target cannot have '@' header", map.locate(0));
            }
            result.append(&map, 0..first.trim_end().len());
            body_st = first_ed + usize::from(first_ed < text.len());
        } else if let Some(header) = first
            .strip_prefix("--")
            .map(|tail| tail.trim_start_matches('-').trim_start_matches([' ', '\t']))
            .and_then(|tail| tail.strip_prefix('@'))
        {
            if self.is_bundled {
                result.generate(format!("@{}", header.trim()), &map, 0..first_ed);
            } else {
                tracing::warn!(
                    "{}: single target has '--@' on the first line; leaving it as comment",
                    map.locate(0)
                );
                result.append(&map, 0..first_ed);
            }
            body_st = first_ed + usize::from(first_ed < text.len());
        } else if self.is_bundled {
            let name = self.target.name();
            if name.is_empty() {
                bail!("failed to determine target name for target '{}'", self.target.path());
            }
            result.generate(format!("@{name}"), &map, 0..0);
        }

        if body_st < text.len() {
            if !result.is_empty() {
                result.generate("\n", &map, body_st..body_st);
            }
            result.append(&map, body_st..map.len());
        }

        if !result.is_empty() {
            result.generate("\n", &map, map.len()..map.len());
        }

        Ok(result)
    }

    #[allow(clippy::too_many_lines)]
    fn normalize_props(map: &source::Map) -> anyhow::Result<source::Map> {
        let src = map.text();
        let mut tokens = Vec::new();
        let mut lexer = lua::Lexer::new(&src);
        lexer.curr = locate_header(&src);

        for token in lexer {
            let token = token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
            if !matches!(token.kind, TokenKind::Whitespace | TokenKind::Newline) {
                tokens.push(token);
            }
        }

        let assigns = lua::Parser::new(&tokens)
            .parse()
            .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?
            .assigns;
        let mut result = source::Map::default();
        let mut curr = 0;

        for token in &tokens {
            let TokenKind::Comment(comment) = &token.kind else {
                continue;
            };

            if comment.bracket.is_some() {
                continue;
            }

            let span = token.span;
            let comment = comment.content;

            let Some((header, rest)) = comment.split_once(':') else {
                continue;
            };
            let Some((kind, var_part)) = header.split_once('@') else {
                continue;
            };
            let kind = kind.trim_end_matches([' ', '\t']);
            if kind.is_empty() || !kind.bytes().all(|b| b.is_ascii_lowercase()) {
                continue;
            }

            let var = var_part.trim_matches([' ', '\t']);
            if !lua::is_ident(var).unwrap_or(false) || src[..span.st].rsplit('\n').next().unwrap().trim().is_empty() {
                continue;
            }

            let pos = map.locate(span.st);
            if matches!(kind, "data" | "hide") {
                bail!("{pos}: '{kind}' definition cannot be attached to variable declaration: '{comment}'");
            }

            if !matches!(
                kind,
                "track"
                    | "check"
                    | "checksection"
                    | "select"
                    | "color"
                    | "file"
                    | "folder"
                    | "value"
                    | "font"
                    | "figure"
                    | "string"
                    | "text"
            ) {
                bail!("{pos}: unsupported property '{kind}' on variable declaration");
            }

            let Some(assign) = assigns
                .partition_point(|assign| assign.span.ed <= span.st)
                .checked_sub(1)
                .map(|i| &assigns[i])
            else {
                bail!("{pos}: property definition must follow a single variable declaration or assignment");
            };
            let line_st = src[..assign.span.st].rfind('\n').map_or(0, |at| at + 1);
            let assign_st = if src[line_st..assign.span.st].contains(';') {
                line_st
            } else {
                assign.span.st
            };

            if assign_st < curr
                || !src[line_st..assign.span.st]
                    .bytes()
                    .all(|b| matches!(b, b' ' | b'\t' | b';'))
                || !src[assign.span.ed..span.st]
                    .bytes()
                    .all(|b| matches!(b, b' ' | b'\t' | b';'))
                || assign.targets.len() != 1
                || assign.vals.len() > 1
            {
                bail!("{pos}: property definition must follow a single variable declaration or assignment");
            }

            let [token] = &tokens[tokens.partition_point(|token| token.span.st < assign.targets[0].st)
                ..tokens.partition_point(|token| token.span.st < assign.targets[0].ed)]
            else {
                bail!("{pos}: property definition cannot be used on table field assignment: '{comment}'");
            };
            let Some(name) = token.kind.name() else {
                bail!("{pos}: property definition must follow a single variable declaration or assignment");
            };

            if var != "_" && name != var {
                bail!("{pos}: variable name '{var}' does not match assignment target '{name}' in '{comment}'");
            }

            let var = if var == "_" {
                Cow::Owned(var_part.replacen('_', name, 1))
            } else {
                Cow::Borrowed(var_part)
            };
            let trimmed = rest.trim_start_matches([' ', '\t']);
            let ws = &rest[..rest.len() - trimmed.len()];
            let rest = if let Some(val) = assign.vals.first() {
                resolve_default(kind, &src[val.st..val.ed], trimmed)
                    .map_err(|err| anyhow::anyhow!("{pos}: {err} in '{comment}'"))?
            } else {
                Cow::Borrowed(trimmed)
            };

            result.append(map, curr..assign_st);
            result.generate(format!("--{kind}@{var}:{ws}{rest}"), map, span.st..span.ed);
            curr = span.ed;
        }

        result.append(map, curr..map.len());
        Ok(result)
    }

    fn extract_defs<'s>(&self, map: &source::Map, src: &'s str) -> anyhow::Result<Definition<'s>> {
        let mut props = Vec::new();
        let mut shaders = Vec::new();
        let mut tokens = Vec::new();

        let mut lexer = lua::Lexer::new(src);
        lexer.curr = locate_header(src);

        for token in lexer {
            let token = token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
            let TokenKind::Comment(comment) = token.kind else {
                if !matches!(token.kind, TokenKind::Whitespace | TokenKind::Newline) {
                    tokens.push(token);
                }
                continue;
            };

            if let Some(bracket) = comment.bracket {
                if bracket.level == 0
                    && let Some((kind, rest)) = comment.content.split_once('@')
                    && matches!(kind.trim_end(), "pixelshader" | "computeshader")
                    && let Some((name, body)) = rest.split_once(':')
                {
                    if !src[..token.span.st].rsplit('\n').next().unwrap().trim().is_empty() {
                        bail!("{}: shader block must start on its own line", map.locate(token.span.st));
                    }

                    shaders.push(Shader {
                        pos: map.locate(token.span.st),
                        kind: kind.trim_end(),
                        name: name.trim_matches(' '),
                        body: body.trim(),
                    });
                }

                continue;
            }

            let curr = token.span.st;
            if !src[..curr].rsplit('\n').next().unwrap().trim().is_empty() {
                continue;
            }

            let Some((header, body)) = comment.content.split_once(':') else {
                continue;
            };

            let (kind, var) = header
                .split_once('@')
                .map_or((header.trim(), None), |(k, v)| (k.trim(), Some(v.trim())));

            if !kind.starts_with(|c: char| c.is_ascii_lowercase()) {
                continue;
            }

            if var.is_some() && (!self.suffix.ends_with('2') || self.suffix.eq_ignore_ascii_case(".tra2")) {
                bail!("{}: '{}' cannot use '@' in '{}'", map.locate(curr), kind, self.suffix);
            }

            if comment.content.chars().any(char::is_control) {
                bail!("{}: definition cannot contain control characters", map.locate(curr));
            }

            let (key, rest) = body
                .split_once(',')
                .map_or((body.trim(), None), |(k, r)| (k.trim(), Some(r.trim())));

            props.push(Property {
                pos: map.locate(curr),
                kind,
                var,
                key,
                rest,
            });
        }

        Ok(Definition { props, shaders, tokens })
    }

    #[allow(clippy::match_same_arms, clippy::too_many_lines)]
    fn resolve_l10n(&self, props: &[Property<'_>]) -> anyhow::Result<String> {
        fn to_display(name: &str) -> &str {
            name.rsplit("::").next().unwrap_or(name)
        }

        let validate = |name: &str, pos: &Position| -> anyhow::Result<()> {
            if name.starts_with("effect.") {
                bail!("{pos}: property name must not start with 'effect.', got '{name}'");
            }

            if name.as_bytes().first().is_some_and(u8::is_ascii_digit) {
                bail!("{pos}: property name must not start with a digit, got '{name}'");
            }

            Ok(())
        };

        let mut displays = BTreeSet::new();

        if self.suffix.eq_ignore_ascii_case(".tra2") {
            for prop in props {
                if prop.var.is_some() || !prop.kind.eq_ignore_ascii_case("param") || prop.rest.is_none() {
                    continue;
                }

                let key = prop.key.split('/').collect::<Vec<_>>();
                let items = match key.as_slice() {
                    [_] | [_, "check"] => &[][..],
                    [_, "select", items @ ..] if !items.is_empty() => items,
                    _ => continue,
                };

                for item in items {
                    if let Some((name, _)) = item.rsplit_once('=') {
                        let name = name.trim();
                        displays.insert(to_display(name).to_owned());
                    }
                }

                let key = key[0].trim();
                validate(key, &prop.pos)?;

                displays.insert(to_display(key).to_owned());
            }

            return Ok(self.format_l10n(&displays, None));
        }

        let mut seen = HashSet::new();
        let mut tips = IndexSet::new();

        for prop in props {
            let kind = prop.kind.to_ascii_lowercase();
            let pos = &prop.pos;

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

            let mut key = prop.key;
            match (prop.var, kind.as_str()) {
                (None, _) if prop.is_track() || prop.is_check() => {}
                (None, "color") => {
                    check("色")?;
                    displays.insert("色".to_owned());
                    tips.insert("色".to_owned());
                    continue;
                }
                (None, "file") => {
                    check("ファイル")?;
                    displays.insert("ファイル".to_owned());
                    tips.insert("ファイル".to_owned());
                    continue;
                }
                (None, "param") => {
                    for item in prop.key.split(';') {
                        let Some((var, _)) = item.split_once('=') else {
                            bail!("{pos}: item of 'param' must contain '=', got '{item}'");
                        };

                        let var = var.trim();
                        check(var)?;
                        displays.insert(var.to_owned());
                        tips.insert(var.to_owned());
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
                        displays.insert(to_display(name).to_owned());
                    }

                    for name in names {
                        let count = counts.get_mut(name).expect("dialog item count must exist");
                        *count -= 1;
                        tips.insert(format!("{}{name}", "dialog::".repeat(*count)));
                    }

                    continue;
                }
                (None, "group" | "separator") => {
                    if kind == "group" {
                        check(key)?;
                    }

                    displays.insert(to_display(key).to_owned());
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
                if let Some((name, val)) = key.rsplit_once('=')
                    && val.trim().parse::<i64>().is_ok()
                {
                    key = name.trim_end();
                }

                let mut names = HashSet::new();
                for item in prop.rest.unwrap_or("").split(',') {
                    if let Some((name, _)) = item.rsplit_once('=') {
                        let name = name.trim();
                        if name.contains("dialog::") {
                            bail!("{pos}: property name cannot contain 'dialog::', got '{name}'");
                        }

                        if !names.insert(name) {
                            bail!("{pos}: duplicate item '{name}' in 'select'");
                        }

                        displays.insert(to_display(name).to_owned());
                    }
                }
            }

            check(key)?;
            tips.insert(key.to_owned());
            displays.insert(to_display(key).to_owned());
        }

        Ok(self.format_l10n(&displays, Some(&tips)))
    }

    fn format_l10n(&self, displays: &BTreeSet<String>, tips: Option<&IndexSet<String>>) -> String {
        let mut result = format!("[{}]\n{}=\n", self.target.key(), self.target.key());
        for key in displays {
            if key == self.target.key() {
                continue;
            }

            result.push_str(key);
            result.push_str("=\n");
        }

        result.push('\n');

        let Some(tips) = tips else {
            return result;
        };

        result.push_str("[Tips.");
        result.push_str(self.target.key());
        result.push_str("]\neffect.name=\n");
        for key in tips {
            result.push_str(key);
            result.push_str("=\n");
        }

        result.push('\n');

        result
    }
}

struct Definition<'a> {
    props: Vec<Property<'a>>,
    shaders: Vec<Shader<'a>>,
    tokens: Vec<lua::Token<'a>>,
}

struct Scope {
    span: lua::Span,
    includes: Vec<Include>,
}

struct Include {
    span: lua::Span,
    file: PathBuf,
    padding: usize,
    require: Option<lua::Span>,
}

pub fn build(
    src: &str,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    is_bundled: bool,
    suffix: &str,
) -> anyhow::Result<Output> {
    Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        is_bundled,
        suffix,
        include_stack: std::fs::canonicalize(target.path()).into_iter().collect(),
        map: source::Map::default(),
        scopes: Vec::new(),
        repls: Vec::new(),
    }
    .build(src)
}

fn locate_header(src: &str) -> usize {
    let rest = src.trim_start();
    if rest.starts_with('@') {
        src.len() - rest.len() + rest.find('\n').map_or(rest.len(), |at| at + 1)
    } else {
        0
    }
}

#[allow(clippy::too_many_lines)]
fn resolve_default<'a>(kind: &str, val: &str, rest: &'a str) -> anyhow::Result<Cow<'a, str>> {
    let parse_string = |src: &str| -> anyhow::Result<Option<String>> {
        let mut tokens = lua::Lexer::new(src).filter(|token| {
            token.as_ref().map_or(true, |token| {
                !matches!(token.kind, TokenKind::Whitespace | TokenKind::Newline)
            })
        });
        Ok(match (tokens.next().transpose()?, tokens.next().transpose()?) {
            (
                Some(lua::Token {
                    kind: TokenKind::String(bytes),
                    ..
                }),
                None,
            ) => String::from_utf8(bytes.val.into_owned()).ok(),
            _ => None,
        })
    };

    match kind {
        "track" => {
            let Some(parsed) = val.parse::<f64>().ok().filter(|v| v.is_finite()) else {
                bail!("assignment value '{val}' of '{kind}' must be a number");
            };

            let mut parts = rest.split(',').collect::<Vec<_>>();
            if let Some(part) = parts.get_mut(3) {
                let target = part.trim();
                if target == "_" {
                    let part = part.replacen('_', val, 1);
                    parts[3] = &part;
                    return Ok(Cow::Owned(parts.join(",")));
                }

                if target
                    .parse::<f64>()
                    .ok()
                    .is_none_or(|v| (v - parsed).abs() > f64::EPSILON)
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

            let mut parts = rest.split(',').collect::<Vec<_>>();
            if let Some(part) = parts.get_mut(1) {
                let target = part.trim();
                if target == "_" {
                    let part = part.replacen('_', val, 1);
                    parts[1] = &part;
                    return Ok(Cow::Owned(parts.join(",")));
                }

                if target != val {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "select" => {
            let Ok(parsed) = val.parse::<i64>() else {
                bail!("assignment value '{val}' of '{kind}' must be an integer");
            };

            let mut parts = rest.split(',').collect::<Vec<_>>();
            if let Some((name, part)) = parts.first().and_then(|part| part.rsplit_once('=')) {
                let target = part.trim();
                if target == "_" {
                    let val = format!("{name}={}", part.replacen('_', val, 1));
                    parts[0] = &val;
                    return Ok(Cow::Owned(parts.join(",")));
                }

                if target.parse::<i64>() != Ok(parsed) {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "color" => {
            let parsed = validate::script::parse_int(val);
            if val != "nil" && parsed.is_none() {
                bail!("assignment value '{val}' of '{kind}' must be 'nil' or an integer");
            }

            let mut parts = rest.split(',').collect::<Vec<_>>();
            if let Some(part) = parts.get_mut(1) {
                let target = part.trim();
                if target == "_" {
                    let part = part.replacen('_', val, 1);
                    parts[1] = &part;
                    return Ok(Cow::Owned(parts.join(",")));
                }

                if if val == "nil" {
                    target != "nil"
                } else {
                    validate::script::parse_int(target) != parsed
                } {
                    bail!("default value '{target}' does not match assignment value '{val}'");
                }
            }
        }
        "file" | "folder" => {
            if parse_string(val)?.is_none() {
                bail!("assignment value '{val}' of '{kind}' must be a string");
            }
        }
        "value" | "font" | "figure" | "string" | "text" => {
            if let Some(parsed) = parse_string(val)? {
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
                        return Ok(Cow::Owned(format!("{name},{}", default.replacen('_', &parsed, 1))));
                    }

                    if if kind == "value" {
                        parse_string(target)?.as_deref() != Some(parsed.as_str())
                    } else {
                        target != parsed.replace('\n', "\\n")
                    } {
                        bail!("default value '{target}' does not match assignment value '{val}'");
                    }
                }
            } else if kind != "value" {
                bail!("assignment value '{val}' of '{kind}' must be a string");
            } else if let Some((name, default)) = rest.split_once(',') {
                let target = default.trim();
                if target == "_" {
                    return Ok(Cow::Owned(format!("{name},{}", default.replacen('_', val, 1))));
                }

                let is_significant = |token: &Result<lua::Token<'_>, lua::Error>| {
                    token.as_ref().map_or(true, |token| {
                        !matches!(
                            token.kind,
                            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                        )
                    })
                };
                let mut lhs = lua::Lexer::new(target).filter(is_significant);
                let mut rhs = lua::Lexer::new(val).filter(is_significant);
                loop {
                    match (lhs.next().transpose()?, rhs.next().transpose()?) {
                        (None, None) => break,
                        (Some(lhs), Some(rhs))
                            if match (&lhs.kind, &rhs.kind) {
                                (TokenKind::String(lhs), TokenKind::String(rhs)) => lhs.val == rhs.val,
                                (lhs, rhs) => lhs == rhs,
                            } => {}
                        _ => bail!("default value '{target}' does not match assignment value '{val}'"),
                    }
                }
            }
        }
        _ => {}
    }

    Ok(Cow::Borrowed(rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserve_property_origins() {
        let map = source::Map::new(
            "local value = \"日本語\" --string@value:名前, _\nreturn 1",
            Position {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );
        let map = Builder::normalize_props(&map).unwrap();
        let src = map.text();
        assert_eq!(src, "--string@value:名前, 日本語\nreturn 1");
        for at in [0, src.find("日本語").unwrap()] {
            assert_eq!(map.locate(at).to_string(), "test.lua:1:21");
        }
        assert_eq!(map.locate(src.find("return").unwrap()).to_string(), "test.lua:2:1");
    }

    #[test]
    fn extract_defs() {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({
            "path": "test.anm2",
        }))
        .unwrap();
        let builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Owned(IndexMap::new()),
            is_bundled: false,
            suffix: ".anm2",
            include_stack: Vec::new(),
            map: source::Map::default(),
            scopes: Vec::new(),
            repls: Vec::new(),
        };

        let map = source::Map::new(
            "--track 0: A, 0, 100, 0, 1\n\
             --check0: B, 0\n\
             --track @var: C, 0, 100, 0, 1\n\
             --track custom: D, 0, 100, 0, 1\n\
             --[[pixelshader @main:\nfloat4 main() : SV_Target { return 0; }\n]]\n",
            Position {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );

        let src = map.text();
        let defs = builder.extract_defs(&map, &src).unwrap();
        assert_eq!(defs.props.len(), 4);
        assert_eq!(defs.props[0].kind, "track 0");
        assert_eq!(defs.props[0].key, "A");
        assert_eq!(defs.props[1].kind, "check0");
        assert_eq!(defs.props[1].key, "B");
        assert_eq!(defs.props[2].kind, "track");
        assert_eq!(defs.props[2].var, Some("var"));
        assert_eq!(defs.props[2].key, "C");
        assert_eq!(defs.props[3].kind, "track custom");
        assert_eq!(defs.props[3].key, "D");

        assert_eq!(defs.shaders.len(), 1);
        assert_eq!(defs.shaders[0].kind, "pixelshader");
        assert_eq!(defs.shaders[0].name, "main");
        assert_eq!(defs.shaders[0].body, "float4 main() : SV_Target { return 0; }");
    }

    #[test]
    fn collect_l10n() {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({
            "path": "test.anm2",
        }))
        .unwrap();
        let builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Owned(IndexMap::new()),
            is_bundled: false,
            suffix: ".anm2",
            include_stack: Vec::new(),
            map: source::Map::default(),
            scopes: Vec::new(),
            repls: Vec::new(),
        };

        let map = source::Map::new(
            "--track0: X, 0, 100, 0, 1\n\
             --check0: Y, 0\n\
             --track aaa: V, 0, 100, 0, 1\n\
             --check custom: U, 1\n\
             --track@var: Z, 0, 100, 0, 1\n\
             --check@flag: W, 1\n",
            Position {
                file: Path::new("test.lua").into(),
                line: 1,
                col: 1,
            },
        );

        let src = map.text();
        let defs = builder.extract_defs(&map, &src).unwrap();
        let l10n = builder.resolve_l10n(&defs.props).unwrap();
        assert!(l10n.contains("X=\n"));
        assert!(l10n.contains("Y=\n"));
        assert!(l10n.contains("V=\n"));
        assert!(l10n.contains("U=\n"));
        assert!(l10n.contains("Z=\n"));
        assert!(l10n.contains("W=\n"));
    }

    #[test]
    fn normalize_assignments() {
        let make_map = |src: &str| {
            source::Map::new(
                src,
                Position {
                    file: Path::new("input.lua").into(),
                    line: 1,
                    col: 1,
                },
            )
        };

        for (src, expected) in [
            ("local value = 1; --value@_:Value, _", "--value@value:Value, 1"),
            (
                "local value = { first = 1, second = {2, 3} } --value@_:Value, _",
                "--value@value:Value, { first = 1, second = {2, 3} }",
            ),
            ("local value --track@_:Value, 0, 1, 0", "--track@value:Value, 0, 1, 0"),
            ("local value = '日本語' --text@_:Value, _", "--text@value:Value, 日本語"),
        ] {
            assert_eq!(
                Builder::normalize_props(&make_map(src)).unwrap().text(),
                expected,
                "{src}"
            );
        }

        for src in [
            "local first, second = 0, 1 --value@_:Value, _",
            "obj.value = 0 --value@_:Value, _",
            "obj[key] = 0 --value@_:Value, _",
            "local value = 0 --value@other:Value, _",
            "local value = 0 --value@_:Value, 1",
            "local value = 0 --hide@_:Value",
            "local value = 0 --data@_:Value",
        ] {
            assert!(Builder::normalize_props(&make_map(src)).is_err(), "{src}");
        }

        for src in [
            "local value = 0 ---value@_:Value, _",
            "local value = '--value@_:Value, _'",
            "--[[local value = 0 --value@_:Value, _]]",
        ] {
            assert_eq!(Builder::normalize_props(&make_map(src)).unwrap().text(), src, "{src}");
        }
    }

    #[test]
    fn validate_l10n_names() {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.anm2" })).unwrap();
        let builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Owned(IndexMap::new()),
            is_bundled: false,
            suffix: ".anm2",
            include_stack: Vec::new(),
            map: source::Map::default(),
            scopes: Vec::new(),
            repls: Vec::new(),
        };
        let pos = Position {
            file: Path::new("input.anm2").into(),
            line: 1,
            col: 1,
        };

        for key in ["effect.hidden", "1value", "dialog::hidden"] {
            let props = [Property {
                pos: pos.clone(),
                kind: "value",
                var: Some("value"),
                key,
                rest: Some("0"),
            }];
            assert!(builder.resolve_l10n(&props).is_err(), "{key}");
        }

        let props = [
            Property {
                pos: pos.clone(),
                kind: "value",
                var: Some("first"),
                key: "Value",
                rest: Some("0"),
            },
            Property {
                pos,
                kind: "value",
                var: Some("second"),
                key: "Value",
                rest: Some("1"),
            },
        ];
        assert!(builder.resolve_l10n(&props).is_err());
    }

    #[test]
    fn combine_outputs() {
        let mut dst = Output {
            script: "first".to_owned(),
            l10n: "First=\n".to_owned(),
        };
        dst.push_str(&Output {
            script: "second".to_owned(),
            l10n: "Second=\n".to_owned(),
        });
        assert_eq!(dst.script, "firstsecond");
        assert_eq!(dst.l10n, "First=\nSecond=\n");
        dst.replace("irst", "IRST");
        assert_eq!(dst.script, "fIRSTsecond");
        assert_eq!(dst.l10n, "FIRST=\nSecond=\n");
    }

    #[test]
    fn format_l10n() {
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.anm2" })).unwrap();
        let builder = Builder {
            target: &target,
            include_dirs: &[],
            vars: Cow::Owned(IndexMap::new()),
            is_bundled: false,
            suffix: ".anm2",
            include_stack: Vec::new(),
            map: source::Map::default(),
            scopes: Vec::new(),
            repls: Vec::new(),
        };
        let names = BTreeSet::from(["Z".to_owned(), "A".to_owned(), target.key().to_owned()]);
        let tips = IndexSet::from(["Z".to_owned(), "A".to_owned()]);
        let key = target.key();
        assert_eq!(
            builder.format_l10n(&names, Some(&tips)),
            format!("[{key}]\n{key}=\nA=\nZ=\n\n[Tips.{key}]\neffect.name=\nZ=\nA=\n\n")
        );
        assert_eq!(
            builder.format_l10n(&names, None),
            format!("[{key}]\n{key}=\nA=\nZ=\n\n")
        );
    }
}
