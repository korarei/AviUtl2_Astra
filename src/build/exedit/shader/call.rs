use crate::build::lang::lua::TokenKind;
use crate::build::lang::{hlsl, lua};
use crate::build::source::{self, Pos};
use anyhow::bail;
use std::collections::HashSet;

pub(super) fn validate_name(pos: &Pos, kind: &str, name: &str) -> anyhow::Result<()> {
    if name.chars().any(char::is_control) {
        bail!("{pos}: name of '{kind}' cannot contain control characters");
    }

    if name.contains('@') {
        bail!("{pos}: name of '{kind}' cannot contain '@', got '{name}'");
    }

    let bytes = name.as_bytes();
    if bytes
        .first()
        .is_none_or(|byte| !byte.is_ascii_alphabetic() && *byte != b'_')
        || !bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
    {
        bail!("{pos}: name of '{kind}' must match [A-Za-z_][A-Za-z0-9_]*, got '{name}'");
    }

    if !matches!(
        hlsl::Lexer::new(name).next(),
        Some(Ok(hlsl::Token {
            kind: hlsl::TokenKind::Ident(_),
            ..
        }))
    ) {
        bail!("{pos}: name of '{kind}' cannot be an HLSL keyword or reserved word, got '{name}'");
    }

    Ok(())
}

pub(super) fn validate(
    map: &source::Map,
    tokens: &[lua::Token<'_>],
    calls: &[lua::Call<'_>],
    shaders: &HashSet<(&str, &[u8])>,
) -> anyhow::Result<()> {
    for call in calls {
        let Some(reference) = call.reference.as_deref() else {
            continue;
        };

        let ["obj", kind @ ("pixelshader" | "computeshader")] = reference else {
            continue;
        };

        let kind = *kind;
        let pos = map.locate(call.span.st);
        let Some(arg) = call.args.first() else {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        };

        let st = tokens.partition_point(|token| token.span.st < arg.st);
        let ed = tokens.partition_point(|token| token.span.st < arg.ed);
        let mut arg_tokens = tokens[st..ed].iter().filter(|token| !token.kind.is_trivia());
        let Some(TokenKind::String(name)) = arg_tokens.next().map(|token| &token.kind) else {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        };

        if arg_tokens.next().is_some() {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        }

        if !name.val.contains(&b'@') && !shaders.contains(&(kind, name.val.as_ref())) {
            bail!("{pos}: undefined {kind} '{}'", String::from_utf8_lossy(&name.val));
        }
    }

    Ok(())
}
