mod call;
mod hlsl;
mod layout;

use crate::build::lang::lua::{self, TokenKind};
use crate::build::lang::{self, astra};
use crate::build::source::{self, Pos};
use crate::config::BuildTarget;
use anyhow::bail;
use indexmap::IndexMap;
use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::Range;
use std::path::PathBuf;

pub(super) struct Processor<'a> {
    pub target: &'a BuildTarget,
    pub include_dirs: &'a [PathBuf],
    pub vars: &'a IndexMap<String, String>,
}

impl Processor<'_> {
    pub(super) fn process<'s>(
        &self,
        map: &source::Map,
        src: &'s str,
        token: &lua::Token<'s>,
        can_define: bool,
    ) -> anyhow::Result<Option<(source::Map, Block<'s>)>> {
        let TokenKind::Comment(comment) = &token.kind else {
            return Ok(None);
        };
        let Some(bracket) = comment.bracket else {
            return Ok(None);
        };
        let Some(spans) = parse(map, src, token.span.st, comment.content, bracket.level) else {
            return Ok(None);
        };

        if !can_define {
            bail!(
                "{}: shader block '{}' is not supported in this format",
                spans.pos,
                spans.kind
            );
        }

        let mut dst = source::Map::default();
        dst.copy(map, token.span.st..spans.name.start);
        let name = astra::expand(&src[spans.name.clone()], self.vars, astra::Escape::None)
            .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(spans.name.start + err.offset)))?;
        dst.apply(map, spans.name.clone(), name.to_edits(spans.name.start));
        dst.copy(map, spans.name.end..spans.body.start);
        let body = normalize(map.slice(spans.body.clone()))?;
        let body = crate::build::shader::hlsl::build(&body, self.target, self.include_dirs, self.vars)?;
        let body = if !src[spans.body.clone()].ends_with('\n') && body.ends_with('\n') {
            body.slice(..body.len() - 1)
        } else {
            body
        };
        dst.copy(&body, ..);
        dst.copy(map, spans.body.end..token.span.ed);
        Ok(Some((
            dst,
            Block {
                pos: spans.pos,
                kind: spans.kind,
                name: name.text,
                body,
            },
        )))
    }
}

fn parse<'s>(map: &source::Map, src: &str, st: usize, content: &'s str, level: usize) -> Option<Spans<'s>> {
    let (kind, rest) = content.split_once('@')?;
    let has_space = kind.starts_with(char::is_whitespace);
    let kind = kind.trim();

    if !matches!(kind, "pixelshader" | "computeshader") {
        if kind
            .as_bytes()
            .get(kind.len().saturating_sub(6)..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(b"shader"))
        {
            tracing::warn!(
                "{}: unknown shader kind '{kind}'; expected 'pixelshader' or 'computeshader'; leaving it as comment",
                map.locate(st)
            );
        }
        return None;
    }

    let Some((name, body)) = rest.split_once(':') else {
        tracing::warn!(
            "{}: missing ':' after '{kind}' name; leaving it as comment",
            map.locate(st)
        );
        return None;
    };

    let prefix = src[..st].rsplit('\n').next().unwrap();
    let has_indent = !prefix.is_empty() && prefix.trim().is_empty();
    let pos = map.locate(st);

    if level != 0 {
        tracing::warn!("{pos}: shader block must use '--[[...]]'; leaving it as comment");
    }

    if has_indent {
        tracing::warn!("{pos}: whitespace before '--' for shader is not allowed; leaving it as comment");
    }

    if has_space {
        tracing::warn!("{pos}: whitespace before shader kind is not allowed; leaving it as comment");
    }

    if level != 0 || has_indent || has_space {
        return None;
    }

    if !prefix.trim().is_empty() {
        tracing::warn!("{pos}: shader block must start on its own line; leaving it as comment");
        return None;
    }

    Some(Spans {
        pos,
        kind,
        name: {
            let st = name.as_ptr() as usize - src.as_ptr() as usize;
            st..st + name.len()
        },
        body: {
            let st = body.as_ptr() as usize - src.as_ptr() as usize;
            st..st + body.len()
        },
    })
}

fn normalize(map: source::Map) -> anyhow::Result<source::Map> {
    let text = map.text();
    let mut dst = None;
    let mut curr = 0;
    let mut is_line_st = true;
    let mut lexer = lang::hlsl::Lexer::new(&text);

    while let Some(token) = lexer.next() {
        let token = token.map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;

        if token.kind == lang::hlsl::TokenKind::Other('-') && is_line_st && text[token.span.st..].starts_with("--") {
            let st = token.span.st;
            let line_ed = st + text[st..].find('\n').unwrap_or(text.len() - st);

            if text[st + 2..line_ed].trim_start().starts_with('#') {
                let dst = dst.get_or_insert_with(source::Map::default);
                dst.copy(&map, curr..st);
                dst.emit("//", &map, st..st + 2);
                curr = st + 2;

                let next = line_ed.saturating_add(1).min(text.len());
                is_line_st = true;
                lexer = lang::hlsl::Lexer::new_at(&text, next).expect("newline end must be a valid UTF-8 boundary");
                continue;
            } else if text[st..].starts_with("--[") {
                let token = lua::Lexer::new(&text[st..])
                    .next()
                    .unwrap()
                    .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(st + err.offset), err.message))?;

                if let TokenKind::Comment(comment) = token.kind
                    && let Some(bracket) = comment
                        .bracket
                        .filter(|_| comment.content.trim_start().starts_with('#'))
                {
                    if let Some(at) = comment.content.find("*/") {
                        bail!(
                            "{}: Lua directive contains an HLSL block comment terminator",
                            map.locate(st + bracket.span.st + at)
                        );
                    }

                    let dst = dst.get_or_insert_with(source::Map::default);
                    dst.copy(&map, curr..st);
                    dst.emit(
                        format!("{:<width$}", "/*", width = bracket.span.st),
                        &map,
                        st..st + bracket.span.st,
                    );
                    dst.copy(&map, st + bracket.span.st..st + bracket.span.ed);
                    dst.emit(
                        format!("{:<width$}", "*/", width = token.span.ed - bracket.span.ed),
                        &map,
                        st + bracket.span.ed..st + token.span.ed,
                    );
                    curr = st + token.span.ed;
                    is_line_st = false;
                    lexer = lang::hlsl::Lexer::new_at(&text, curr).expect("comment end must be a valid UTF-8 boundary");
                    continue;
                }
            }
        }

        is_line_st = match token.kind {
            lang::hlsl::TokenKind::Newline | lang::hlsl::TokenKind::Directive(_) => true,
            lang::hlsl::TokenKind::Whitespace => is_line_st,
            lang::hlsl::TokenKind::Ident(ident) if !ident.is_ascii() => is_line_st && ident.trim().is_empty(),
            _ => false,
        };
    }

    let Some(mut dst) = dst else {
        return Ok(map);
    };

    dst.copy(&map, curr..map.len());
    Ok(dst)
}

pub(super) fn validate(
    blocks: &[Block<'_>],
    call_map: &source::Map,
    tokens: &[lua::Token<'_>],
    calls: &[lua::Call<'_>],
) -> anyhow::Result<()> {
    let mut shaders = HashSet::new();

    for Block { pos, kind, name, body } in blocks {
        let (kind, name) = (*kind, name.trim());
        call::validate_name(pos, kind, name)?;

        if !shaders.insert((kind, name.as_bytes())) {
            bail!("{pos}: duplicate {kind} definition '{name}'");
        }

        hlsl::validate_body(pos, kind, name, body)?;
    }

    call::validate(call_map, tokens, calls, &shaders)
}

struct Spans<'a> {
    pos: Pos,
    kind: &'a str,
    name: Range<usize>,
    body: Range<usize>,
}

pub(super) struct Block<'a> {
    pub pos: Pos,
    pub kind: &'a str,
    pub name: Cow<'a, str>,
    pub body: source::Map,
}
