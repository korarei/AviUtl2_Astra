use super::hlsl::{self, Span, Token, TokenKind};
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
    file: &Path,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
) -> anyhow::Result<String> {
    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        include_stack: std::fs::canonicalize(file).into_iter().collect(),
        once_files: HashSet::new(),
        src: Source::default(),
    };
    builder.process(src, file)?;

    let Source { text, positions } = builder.src;
    for (span, msg) in validate_cbuffers(&text) {
        let (st, pos, is_replaced) = &positions[positions.partition_point(|(st, _, _)| *st <= span.st) - 1];
        let mut pos = pos.clone();
        if !*is_replaced {
            pos.col += text[*st..span.st].chars().count();
        }
        tracing::warn!("{pos}: {msg}");
    }
    Ok(text)
}

struct Builder<'a> {
    target: &'a BuildTarget,
    include_dirs: &'a [PathBuf],
    vars: Cow<'a, IndexMap<String, String>>,
    include_stack: Vec<PathBuf>,
    once_files: HashSet<PathBuf>,
    src: Source,
}

#[derive(Default)]
struct Source {
    text: String,
    positions: Vec<(usize, Position, bool)>,
}

struct Packing {
    depth: usize,
    is_row_major: Option<bool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Layout {
    size: usize,
    payload: usize,
    alignment: usize,
}

impl Builder<'_> {
    fn process(&mut self, content: &str, file: &Path) -> anyhow::Result<()> {
        let curr_dir = file.parent().unwrap_or(Path::new("."));
        let file: Arc<Path> = file.into();
        let mut conditions = Conditions::default();
        let mut is_comment = false;

        for (i, line) in content.split_terminator('\n').enumerate() {
            let is_blocked = is_comment;
            let mut lexer = hlsl::Lexer::new(line);
            lexer.state.is_comment = is_comment;
            lexer.by_ref().for_each(drop);
            is_comment = lexer.state.is_comment;
            let comment = line
                .trim_start()
                .strip_prefix("//")
                .filter(|comment| !is_blocked && comment.starts_with('#'))
                .map(|comment| (comment, false, false));

            let locate = |at| Position {
                file: Arc::clone(&file),
                line: i + 1,
                col: line[..at].chars().count() + 1,
            };
            let locate_in = |text: &str, at| locate(text.as_ptr() as usize - line.as_ptr() as usize + at);
            let pos = locate(line.len() - line.trim_start().len() + if comment.is_some() { 2 } else { 0 });

            match preprocess::resolve(comment, &mut conditions, self.vars.as_ref(), &pos)? {
                Action::Keep => {
                    let st = self.src.text.len();
                    self.src.positions.push((st, locate(0), false));
                    let line = Self::expand(line, self.vars.as_ref(), locate, |(at, src, is_replaced)| {
                        self.src.positions.push((st + at, locate(src), is_replaced));
                    })?;
                    self.src.text.push_str(&line);
                    self.src.text.push('\n');
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
                        Self::expand(val, vars, |at| locate_in(val, at), |_| {})?.into_owned(),
                    );
                }
                Action::Undef(key) => {
                    self.vars.to_mut().shift_remove(key);
                }
                Action::Include(include, is_quoted) => {
                    let include = Self::expand(
                        &include,
                        self.vars.as_ref(),
                        |at| match &include {
                            Cow::Borrowed(text) => locate_in(text, at),
                            Cow::Owned(_) => pos.clone(),
                        },
                        |_| {},
                    )?;
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

                    self.load_include(&file, &pos)?;
                }
            }
        }

        conditions.finish()
    }

    fn load_include(&mut self, file: &Path, pos: &Position) -> anyhow::Result<()> {
        if file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lua")) {
            bail!("{pos}: cannot include Lua file '{}' from HLSL", file.display());
        }

        let file = std::fs::canonicalize(file)
            .with_context(|| format!("failed to canonicalize include file '{}'", file.display()))?;
        if self.once_files.contains(&file) {
            return Ok(());
        }

        if self.include_stack.contains(&file) {
            bail!("circular include detected: '{}'", file.display());
        }

        self.include_stack.push(file.clone());
        let res = crate::fs::read_file(&file, self.target.encoding()).and_then(|src| self.process(&src, &file));
        let _ = self.include_stack.pop();
        res
    }

    fn expand<'text>(
        text: &'text str,
        vars: &impl crate::vars::Vars,
        locate: impl Fn(usize) -> Position,
        mut record: impl FnMut((usize, usize, bool)),
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
                let output = output.get_or_insert_with(|| String::with_capacity(text.len()));
                output.push_str(&text[curr..st]);
                record((output.len(), st, true));
                output.push_str(
                    vars.get(key)
                        .ok_or_else(|| anyhow::anyhow!("{}: variable '{key}' not found", locate(st)))?,
                );
                record((output.len(), ed, false));
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

fn validate_cbuffers(src: &str) -> Vec<(Span, String)> {
    let tokens = hlsl::Lexer::new(src)
        .filter(|token| {
            !matches!(
                token.kind,
                TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
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

fn validate_buffer(tokens: &[Token<'_>], name: &str, packing: &mut Packing, issues: &mut Vec<(Span, String)>) {
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
    tokens: &[Token<'_>],
    name: &str,
    offset: &mut Option<usize>,
    is_row_major: Option<bool>,
    issues: &mut Vec<(Span, String)>,
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

impl Packing {
    fn apply<'a>(&mut self, token: &Token<'a>) -> Option<&'a str> {
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
                            TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment { .. }
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
