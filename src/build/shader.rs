use super::hlsl::{self, Token, TokenKind};
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
    let mut include_stack = Vec::new();
    if let Ok(file) = std::fs::canonicalize(file) {
        include_stack.push(file);
    }

    let mut builder = Builder {
        target,
        include_dirs,
        vars: Cow::Borrowed(vars),
        include_stack,
        once_files: HashSet::new(),
        src: Source::default(),
    };
    builder.process(src, file).inspect(|_| {
        for issue in validate_cbuffers(&builder.src.text) {
            let (st, pos, is_replaced) =
                &builder.src.positions[builder.src.positions.partition_point(|(st, _, _)| *st <= issue.span.st) - 1];
            let mut pos = pos.clone();
            if !*is_replaced {
                pos.col += builder.src.text[*st..issue.span.st].chars().count();
            }
            match issue.kind {
                IssueKind::Unclosed(name) => {
                    tracing::warn!("{pos}: cannot analyze unclosed cbuffer '{name}'");
                }
                IssueKind::Conditional(name) => {
                    tracing::warn!("{pos}: cannot determine cbuffer '{name}' layout before HLSL preprocessing");
                }
                IssueKind::Offsets(name) => {
                    tracing::warn!(
                        "{pos}: cannot determine subsequent offsets in cbuffer '{name}' \
                         before HLSL preprocessing"
                    );
                }
                IssueKind::Unsupported { name, ty } => {
                    tracing::warn!("{pos}: cbuffer '{name}' uses non-float or unsupported type '{ty}'");
                }
                IssueKind::Padding { name, member, bytes } => {
                    tracing::warn!(
                        "{pos}: cbuffer '{name}' member '{member}' introduces {bytes} bytes of padding ({} {})",
                        bytes / 4,
                        if bytes / 4 == 1 { "float" } else { "floats" }
                    );
                }
                IssueKind::Unknown(name) => {
                    tracing::warn!(
                        "{pos}: cannot determine layout of declaration in cbuffer '{name}'; \
                         subsequent offsets are unknown"
                    );
                }
            }
        }
    })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Issue<'a> {
    span: hlsl::Span,
    kind: IssueKind<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IssueKind<'a> {
    Unclosed(&'a str),
    Conditional(&'a str),
    Offsets(&'a str),
    Unsupported {
        name: &'a str,
        ty: &'a str,
    },
    Padding {
        name: &'a str,
        member: &'a str,
        bytes: usize,
    },
    Unknown(&'a str),
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
    fn process(&mut self, content: &str, file: &Path) -> anyhow::Result<String> {
        let curr_dir = file.parent().unwrap_or(Path::new("."));
        let file: Arc<Path> = file.into();
        let mut output = String::with_capacity(content.len());
        let mut conditions = Conditions::default();
        let mut is_comment = false;

        for (i, line) in content.split_inclusive('\n').enumerate() {
            let is_newline = line.ends_with('\n');
            let line = line.strip_suffix('\n').unwrap_or(line);
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
            let pos = locate(line.len() - line.trim_start().len() + if comment.is_some() { 2 } else { 0 });

            match preprocess::resolve(comment, &mut conditions, self.vars.as_ref(), &pos)? {
                Action::Keep => {
                    output.push_str(&self.expand_line(line, locate)?);
                    if is_newline {
                        output.push('\n');
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
                        Self::expand(
                            val,
                            vars,
                            |at| locate(val.as_ptr() as usize - line.as_ptr() as usize + at),
                            |_| {},
                        )?
                        .into_owned(),
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
                            Cow::Borrowed(text) => locate(text.as_ptr() as usize - line.as_ptr() as usize + at),
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

                    let Some(nested) = self.load_include(&file, &pos)? else {
                        continue;
                    };

                    output.push_str(&nested);
                    if is_newline && !nested.is_empty() && !nested.ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        }

        conditions.finish().map(|()| output)
    }

    fn expand_line<'text>(
        &mut self,
        line: &'text str,
        locate: impl Fn(usize) -> Position + Copy,
    ) -> anyhow::Result<Cow<'text, str>> {
        let st = self.src.text.len();
        self.src.positions.push((st, locate(0), false));
        let line = Self::expand(line, self.vars.as_ref(), locate, |(at, src, is_replaced)| {
            self.src.positions.push((st + at, locate(src), is_replaced));
        })?;
        self.src.text.push_str(&line);
        self.src.text.push('\n');
        Ok(line)
    }

    fn load_include(&mut self, file: &Path, pos: &Position) -> anyhow::Result<Option<String>> {
        if file.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("lua")) {
            bail!("{pos}: cannot include Lua file '{}' from HLSL", file.display());
        }

        let file = std::fs::canonicalize(file)
            .with_context(|| format!("failed to canonicalize include file '{}'", file.display()))?;
        if self.once_files.contains(&file) {
            return Ok(None);
        }

        if self.include_stack.contains(&file) {
            bail!("circular include detected: '{}'", file.display());
        }

        self.include_stack.push(file.clone());
        let src = crate::fs::read_file(&file, self.target.encoding()).and_then(|src| self.process(&src, &file));
        let _ = self.include_stack.pop();
        src.map(Some)
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

fn validate_cbuffers(src: &str) -> Vec<Issue<'_>> {
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

        curr += 1;
        let st = curr;
        let mut depth = 1;
        while curr < tokens.len() {
            match tokens[curr].text {
                "{" => depth += 1,
                "}" => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            curr += 1;
        }

        if depth != 0 {
            issues.push(Issue {
                span,
                kind: IssueKind::Unclosed(name),
            });
            break;
        }

        if is_conditional || packing.depth != 0 {
            issues.push(Issue {
                span,
                kind: IssueKind::Conditional(name),
            });
            for token in &tokens[st..curr] {
                let _ = packing.apply(token);
            }
            curr += 1;
            continue;
        }

        validate_buffer(&tokens[st..curr], name, &mut packing, &mut issues);
        curr += 1;
    }
    issues
}

fn validate_buffer<'a>(tokens: &[Token<'a>], name: &'a str, packing: &mut Packing, issues: &mut Vec<Issue<'a>>) {
    let mut offset = Some(0_usize);
    let mut st = 0;
    for (i, token) in tokens.iter().enumerate() {
        if let Some(directive) = packing.apply(token) {
            if st != i || matches!(directive, "if" | "ifdef" | "ifndef" | "include") {
                if offset.is_some() {
                    issues.push(Issue {
                        span: token.span,
                        kind: IssueKind::Offsets(name),
                    });
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

fn validate_declaration<'a>(
    tokens: &[Token<'a>],
    name: &'a str,
    offset: &mut Option<usize>,
    is_row_major: Option<bool>,
    issues: &mut Vec<Issue<'a>>,
) {
    let Some(declaration) = hlsl::parse_declaration(tokens) else {
        return;
    };
    let align = |val: usize, size: usize| val.checked_add(size - 1).map(|val| val / size * size);
    let is_known = offset.is_some();
    let is_row_major = declaration.is_row_major.or(is_row_major);
    let layout = declaration
        .ty
        .and_then(|ty| resolve_layout(ty, is_row_major.unwrap_or(false)));
    if !layout.is_some_and(|(_, is_float)| is_float) {
        issues.push(Issue {
            span: declaration.token.span,
            kind: IssueKind::Unsupported {
                name,
                ty: declaration.token.text,
            },
        });
    }
    let layout = if is_row_major.is_none() && layout != declaration.ty.and_then(|ty| resolve_layout(ty, true)) {
        None
    } else {
        layout
    };
    for member in &declaration.members {
        let mut layout = layout.map(|(layout, _)| layout);
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
        if let Some((_, padding)) = placement.filter(|&(_, padding)| padding != 0) {
            issues.push(Issue {
                span: member.span,
                kind: IssueKind::Padding {
                    name,
                    member: member.name,
                    bytes: padding,
                },
            });
        }
    }

    if declaration.is_manual {
        *offset = None;
    } else if declaration.has_error || layout.is_none() || (is_known && offset.is_none()) {
        issues.push(Issue {
            span: declaration.token.span,
            kind: IssueKind::Unknown(name),
        });
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

fn resolve_layout(ty: hlsl::Type<'_>, is_row_major: bool) -> Option<(Layout, bool)> {
    let size = match ty.scalar {
        "double" => 8_usize,
        "float" | "uint" | "int" | "bool" | "half" | "dword" | "min16float" | "min10float" | "min16int"
        | "min12int" | "min16uint" => 4,
        _ => return None,
    };
    let (len, width) = if !ty.is_matrix {
        (1, ty.cols)
    } else if is_row_major {
        (ty.rows, ty.cols)
    } else {
        (ty.cols, ty.rows)
    };
    let width = width * size;
    Some((
        Layout {
            size: (len - 1) * width.div_ceil(16) * 16 + width,
            payload: len * width,
            alignment: if len > 1 { 16 } else { size },
        },
        matches!(ty.scalar, "float" | "half" | "min16float" | "min10float"),
    ))
}
