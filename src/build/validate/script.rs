use super::super::hlsl;
use super::super::lua::{self, TokenKind};
use super::super::script::{Property, Shader};
use super::super::source;
use super::shader;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;

#[allow(clippy::too_many_lines)]
pub(in crate::build) fn validate_modern_props(
    props: &[Property<'_>],
    vars: &mut HashSet<String>,
) -> anyhow::Result<()> {
    for prop in props {
        let kind = prop.kind.to_ascii_lowercase();
        let pos = &prop.pos;

        let Some(var) = prop.var else {
            match kind.as_str() {
                _ if prop.is_track()
                    || prop.is_check()
                    || matches!(kind.as_str(), "color" | "file" | "param" | "dialog") =>
                {
                    tracing::warn!("{pos}: '{kind}' uses legacy syntax");
                }
                "group" => {
                    if let Some(rest) = prop.rest
                        && !rest.is_empty()
                    {
                        let mut args = rest.split(',').map(str::trim);
                        let folding = args.next().unwrap();
                        if !matches!(folding, "true" | "false") {
                            bail!("{pos}: folding of '{kind}' must be 'true' or 'false', got '{folding}'");
                        }

                        let count = args.count() + 1;
                        if count >= 2 {
                            tracing::warn!(
                                "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                                count
                            );
                        }
                    }
                }
                "script" => {
                    if let Some(rest) = prop.rest
                        && !rest.is_empty()
                    {
                        tracing::warn!("{pos}: '{kind}' expects 0 arguments, got {}", rest.split(',').count());
                    }

                    match prop.key.to_ascii_lowercase().as_str() {
                        "luajit" => {}
                        "lua" => tracing::warn!("{pos}: runtime of '{kind}' is 'Lua'"),
                        _ => {
                            bail!(
                                "{pos}: runtime of '{kind}' must be 'LuaJIT' or 'Lua', got '{}'",
                                prop.key
                            );
                        }
                    }
                }
                _ => {}
            }

            continue;
        };

        let validate = |var: &str| -> anyhow::Result<()> {
            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
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

        let args = || prop.rest.into_iter().flat_map(|rest| rest.split(',').map(str::trim));

        match kind.as_str() {
            "check" => {
                let mut args = args();
                let Some(default) = args.next() else {
                    bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                };

                let count = args.count() + 1;
                if count > 1 {
                    tracing::warn!(
                        "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                        count
                    );
                }

                if !matches!(default, "true" | "false" | "0" | "1") {
                    bail!("{pos}: default value of '{kind}' must be 'true', 'false', '0', or '1', got '{default}'");
                }
            }
            "checksection" => {
                let mut args = args();
                let Some(default) = args.next() else {
                    bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                };

                let count = args.clone().count() + 1;
                if count > 2 {
                    tracing::warn!(
                        "{pos}: '{kind}' has too many arguments; expected at most 2, got {}",
                        count
                    );
                }

                if !matches!(default, "true" | "false") {
                    bail!("{pos}: default value of 'checksection' must be 'true' or 'false', got '{default}'");
                }

                if let Some(arg) = args.next()
                    && !matches!(arg, "true" | "false")
                {
                    bail!("{pos}: keyframed of 'checksection' must be 'true' or 'false', got '{arg}'");
                }
            }
            "select" => {
                let default = if let Some((_, val)) = prop.key.rsplit_once('=') {
                    let val = val.trim();
                    val.parse::<i64>().map_err(|_| {
                        anyhow::anyhow!("{pos}: default value of '{kind}' must be an integer, got '{val}'")
                    })?
                } else {
                    0
                };

                let rest = prop.rest.unwrap_or("");
                if rest.is_empty() {
                    bail!("{pos}: '{kind}' requires at least 1 item, got 0");
                }

                validate_select(rest.split(','), &default.to_string())
                    .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
            }
            "color" => {
                let mut args = args();
                let Some(default) = args.next() else {
                    bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                };

                let count = args.count() + 1;
                if count > 1 {
                    tracing::warn!(
                        "{pos}: '{kind}' has too many arguments; expected at most 1, got {}",
                        count
                    );
                }

                if default != "nil" && !parse_int(default).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                    bail!(
                        "{pos}: default value of '{kind}' must be 'nil' or an integer between \
                         0x000000 and 0xffffff, got '{default}'"
                    );
                }
            }
            "value" => {
                if prop.rest.is_none_or(str::is_empty) {
                    bail!("{pos}: '{kind}' requires at least 1 argument, got 0");
                }
            }
            "file" | "folder" => {
                if let Some(rest) = prop.rest
                    && !rest.is_empty()
                {
                    tracing::warn!("{pos}: '{kind}' expects 0 arguments, got {}", rest.split(',').count());
                }
            }
            "font" | "figure" | "string" | "text" => {
                let count = args().count();
                if count != 1 {
                    tracing::warn!("{pos}: '{kind}' expects 1 argument, got {count}");
                }
            }
            "data" if !prop.key.parse::<i64>().is_ok_and(|val| (0..=16_000).contains(&val)) => {
                bail!(
                    "{pos}: size of '{kind}' must be an integer between 0 and 16000, got '{}'",
                    prop.key
                );
            }
            _ => {}
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
pub(in crate::build) fn validate_compat_props(
    props: &[Property<'_>],
    vars: &mut HashSet<String>,
) -> anyhow::Result<()> {
    let mut exclusive = None;
    let mut kinds = HashSet::new();

    for prop in props {
        if prop.var.is_some() {
            continue;
        }

        let kind = prop.kind.to_ascii_lowercase();
        let pos = &prop.pos;

        if matches!(kind.as_str(), "color" | "file" | "param" | "dialog") {
            if let Some(prev) = exclusive {
                bail!(
                    "{pos}: '{kind}' cannot coexist with '{prev}'; \
                     only one color, file, param, or dialog is allowed"
                );
            }

            exclusive = Some(prop.kind);
        }

        if (prop.is_track() || prop.is_check()) && !kinds.insert(kind.clone()) {
            bail!("{pos}: duplicate property '{kind}'");
        }

        if matches!(kind.as_str(), "color" | "file") && !vars.insert(kind.clone()) {
            bail!("{pos}: duplicate variable name '{kind}'");
        }

        match kind.as_str() {
            _ if prop.is_track() => validate_track(prop)?,
            _ if prop.is_check() => {
                if !matches!(prop.rest, Some("0" | "1")) {
                    bail!(
                        "{}: default value of '{kind}' must be '0' or '1', got '{}'",
                        prop.pos,
                        prop.rest.unwrap_or("")
                    );
                }
            }
            "color" => {
                if prop.rest.is_some_and(|rest| !rest.is_empty()) {
                    bail!(
                        "{pos}: argument of 'color' must be empty, got '{}'",
                        prop.rest.unwrap_or("")
                    );
                }

                if prop.key != "nil" && !parse_int(prop.key).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                    bail!(
                        "{pos}: default value of 'color' must be 'nil' or an integer \
                         between 0x000000 and 0xffffff, got '{}'",
                        prop.key
                    );
                }
            }
            "file" => {
                if prop.rest.is_some_and(|rest| !rest.is_empty()) {
                    bail!(
                        "{}: argument of 'file' must be empty, got '{}'",
                        prop.pos,
                        prop.rest.unwrap_or("")
                    );
                }
            }
            "param" => {
                if prop.rest.is_some_and(|rest| !rest.is_empty()) {
                    bail!(
                        "{}: argument of 'param' must be empty, got '{}'",
                        prop.pos,
                        prop.rest.unwrap_or("")
                    );
                }

                for item in prop.key.split(';') {
                    let Some((var, _)) = item.split_once('=') else {
                        bail!("{}: item of 'param' must contain '=', got '{item}'", prop.pos);
                    };

                    let var = var.trim();
                    if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
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
                    let Some((name, assign)) = item.split_once(',') else {
                        bail!("{pos}: item of 'dialog' must contain ',', got '{item}'");
                    };

                    let Some((var, val)) = assign.split_once('=') else {
                        bail!("{}: item of 'dialog' must contain '=', got '{item}'", prop.pos);
                    };

                    let var = var.trim();
                    if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
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
                            let val = val.trim();
                            if !matches!(val, "0" | "1") {
                                bail!("{pos}: value of '/chk' in 'dialog' must be '0' or '1', got '{val}'");
                            }
                        }
                        Some("col") => {
                            let val = val.trim();
                            if val != "nil" && !parse_int(val).is_some_and(|v| (0..=0xff_ffff).contains(&v)) {
                                bail!(
                                    "{pos}: argument of 'color' must be 'nil' or an integer \
                                     between 0x000000 and 0xffffff, got '{val}'"
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

#[allow(clippy::match_same_arms)]
pub(in crate::build) fn validate_tra_props(props: &[Property<'_>], is_modern: bool) -> anyhow::Result<()> {
    for prop in props {
        if prop.var.is_some() {
            continue;
        }

        match prop.kind.to_ascii_lowercase().as_str() {
            "param" if prop.rest.is_none() => {
                if !prop.key.parse::<f64>().is_ok_and(f64::is_finite) {
                    bail!(
                        "{}: default value of 'param' must be a number, got '{}'",
                        prop.pos,
                        prop.key
                    );
                }
            }
            "speed" => {
                let pos = &prop.pos;
                if !matches!(prop.key, "0" | "1") {
                    bail!("{pos}: acceleration of 'speed' must be '0' or '1', got '{}'", prop.key);
                }

                let Some(rest) = prop.rest else {
                    continue;
                };

                let mut args = rest.split(',').map(str::trim);
                let decel = args.next().unwrap_or("");
                let count = args.count() + 2;
                if count > 2 {
                    tracing::warn!("{pos}: 'speed' has too many arguments; expected at most 2, got {count}");
                }

                if !matches!(decel, "0" | "1") {
                    bail!("{pos}: deceleration of 'speed' must be '0' or '1', got '{decel}'");
                }
            }
            _ => {}
        }
    }

    if !is_modern {
        return Ok(());
    }

    for prop in props {
        if prop.var.is_some() || !prop.kind.eq_ignore_ascii_case("param") {
            continue;
        }

        let Some(rest) = prop.rest else {
            continue;
        };

        let mut args = rest.split(',').map(str::trim);
        let default = args.next().unwrap_or("");
        let count = args.count() + 1;
        let pos = &prop.pos;

        if count > 1 {
            tracing::warn!("{pos}: 'param' has too many arguments; expected at most 1, got {count}");
        }

        let key = prop.key.split('/').collect::<Vec<_>>();

        match key.as_slice() {
            [_] => {
                if !default.parse::<f64>().is_ok_and(f64::is_finite) {
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

pub(in crate::build) fn validate_legacy_props(props: &[Property<'_>], suffix: &str) -> anyhow::Result<()> {
    for prop in props {
        let kind = prop.kind.to_ascii_lowercase();
        let pos = &prop.pos;

        if prop.is_track() {
            if !matches!(kind.as_str(), "track0" | "track1")
                && (suffix.eq_ignore_ascii_case(".scn") || !matches!(kind.as_str(), "track2" | "track3"))
            {
                bail!("{pos}: '{kind}' is not supported in '{suffix}'");
            }

            if let Some(rest) = prop.rest {
                let count = rest.split(',').count();
                if count > 4 {
                    bail!("{pos}: '{kind}' has too many arguments; expected at most 4, got {count}");
                }

                if let Some(step) = rest.split(',').nth(3).map(str::trim)
                    && !step
                        .parse::<f64>()
                        .is_ok_and(|step| [1.0, 0.1, 0.01].iter().any(|&val| (step - val).abs() < f64::EPSILON))
                {
                    bail!("{pos}: step of '{kind}' must be 1, 0.1, or 0.01, got '{step}'");
                }
            }
        } else if prop.is_check() && kind != "check0" {
            bail!("{pos}: '{kind}' is not supported in '{suffix}'");
        } else if kind == "color" {
            if !parse_int(prop.key).is_some_and(|val| (0..=0xff_ffff).contains(&val)) {
                bail!(
                    "{pos}: default value of 'color' must be an integer between 0x000000 and 0xffffff, got '{}'",
                    prop.key
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

#[allow(clippy::too_many_lines)]
pub(in crate::build) fn validate_shader(
    map: &source::Map,
    src: &str,
    defs: &[Shader<'_>],
    tokens: &[lua::Token<'_>],
) -> anyhow::Result<()> {
    let locate = |at| map.locate(at);
    let mut shaders = HashSet::new();

    for Shader { pos, kind, name, body } in defs {
        let (kind, name, body) = (*kind, *name, *body);
        if name.chars().any(char::is_control) {
            bail!("{pos}: name of '{kind}' cannot contain control characters");
        }

        if name.contains('@') {
            bail!("{pos}: name of '{kind}' cannot contain '@', got '{name}'");
        }

        let mut lexer = hlsl::Lexer::new(name);
        let token = lexer
            .next()
            .transpose()
            .map_err(|err| anyhow::anyhow!("{pos}: {err}"))?;
        if !name.is_ascii()
            || token.as_ref().is_none_or(|token| {
                !matches!(
                    token.kind,
                    hlsl::TokenKind::Ident(_)
                        | hlsl::TokenKind::Keyword(_)
                        | hlsl::TokenKind::Modifier(_)
                        | hlsl::TokenKind::Type(_)
                        | hlsl::TokenKind::Vector
                        | hlsl::TokenKind::Matrix
                )
            })
            || lexer.next().is_some()
        {
            bail!("{pos}: name of '{kind}' must match [A-Za-z_][A-Za-z0-9_]*, got '{name}'");
        }

        if token.is_some_and(|token| !matches!(token.kind, hlsl::TokenKind::Ident(_))) {
            bail!("{pos}: name of '{kind}' cannot be an HLSL keyword or reserved word, got '{name}'");
        }

        if !shaders.insert((kind, name.as_bytes())) {
            bail!("{pos}: duplicate {kind} definition '{name}'");
        }

        let body_st = body.as_ptr() as usize - src.as_ptr() as usize;
        let mut tokens = hlsl::Lexer::new(body)
            .filter(|token| {
                token.as_ref().map_or(true, |token| {
                    !matches!(
                        token.kind,
                        hlsl::TokenKind::Whitespace | hlsl::TokenKind::Newline | hlsl::TokenKind::Comment(_)
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| anyhow::anyhow!("{}: {}", locate(body_st + err.offset), err.message))?;
        let issues = shader::validate_cbuffers(body, &tokens);
        tokens.retain(|token| !matches!(token.kind, hlsl::TokenKind::Directive(_)));
        let has_entry = shader::has_entry(body, &tokens, name, kind)
            .map_err(|err| anyhow::anyhow!("{}: {}", locate(body_st + err.offset), err.message))?;

        let issues = issues.map_err(|err| anyhow::anyhow!("{}: {}", locate(body_st + err.offset), err.message))?;

        for (level, span, msg) in issues {
            let pos = locate(body_st + span.st);
            match level {
                tracing::Level::WARN => tracing::warn!("{pos}: {msg}"),
                tracing::Level::INFO => tracing::info!("{pos}: {msg}"),
                _ => tracing::debug!("{pos}: {msg}"),
            }
        }

        if !has_entry {
            bail!("{pos}: entry point '{name}' is not defined in '{kind}' block");
        }
    }

    let analysis = lua::Parser::new(tokens)
        .parse()
        .map_err(|err| anyhow::anyhow!("{}: {}", locate(err.offset), err.message))?;

    for call in analysis.calls {
        let Some(reference) = call.reference.as_deref() else {
            continue;
        };

        let ["obj", kind @ ("pixelshader" | "computeshader")] = reference else {
            continue;
        };

        let kind = *kind;
        let pos = locate(call.span.st);
        let Some(arg) = call.args.first() else {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        };

        let mut tokens = lua::Lexer::new(&src[arg.st..arg.ed]).filter(|token| {
            token.as_ref().map_or(true, |token| {
                !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Newline | TokenKind::Comment(_)
                )
            })
        });
        let Some(TokenKind::String(name)) = tokens.next().transpose()?.map(|token| token.kind) else {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        };

        if tokens.next().is_some() {
            bail!("{pos}: first argument of '{kind}' must be a string literal");
        }

        if !name.val.contains(&b'@') && !shaders.contains(&(kind, name.val.as_ref())) {
            bail!("{pos}: undefined {kind} '{}'", String::from_utf8_lossy(&name.val));
        }
    }

    Ok(())
}

fn validate_track(prop: &Property<'_>) -> anyhow::Result<()> {
    let pos = &prop.pos;
    let kind = prop.kind;
    let mut args = prop.rest.into_iter().flat_map(|rest| rest.split(',')).map(str::trim);
    let count = args.clone().count();

    if count < 3 {
        bail!("{pos}: '{kind}' requires at least 3 arguments, got {count}");
    }

    if count > 6 {
        tracing::warn!("{pos}: '{kind}' has too many arguments; expected at most 6, got {count}");
    }

    let parse = |name: &str, arg: &str| -> anyhow::Result<f64> {
        let Some(val) = arg.parse::<f64>().ok().filter(|v| v.is_finite()) else {
            bail!("{pos}: {name} of '{kind}' must be a number, got '{arg}'");
        };
        Ok(val)
    };

    let min = parse("min", args.next().unwrap())?;
    let max = parse("max", args.next().unwrap())?;
    let default = parse("default value", args.next().unwrap())?;
    let step = args.next().map_or(Ok(0.1), |arg| parse("step", arg))?;

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

    if let Some(arg) = args.nth(1) {
        parse("sensitivity", arg)?;
    }

    Ok(())
}

fn validate_select<'a>(items: impl IntoIterator<Item = &'a str>, default: &str) -> anyhow::Result<()> {
    let mut vals = HashSet::new();
    let Ok(default) = default.parse::<i64>() else {
        bail!("default value of 'select' must be an integer, got '{default}'");
    };

    for item in items {
        let Some((name, val)) = item.rsplit_once('=') else {
            bail!("item of 'select' must contain '=', got '{item}'");
        };

        if name.trim().starts_with("effect.") {
            bail!(
                "item name of 'select' must not start with 'effect.', got '{}'",
                name.trim()
            );
        }

        let Ok(val) = val.trim().parse::<i64>() else {
            bail!("item value of 'select' must be an integer, got '{}'", val.trim());
        };

        if !vals.insert(val) {
            bail!("item value of 'select' must be unique, got '{val}'");
        }
    }

    if !vals.contains(&default) {
        bail!("default value of 'select' must match an item value, got '{default}'");
    }

    Ok(())
}

pub(in crate::build) fn parse_int(src: &str) -> Option<i64> {
    src.strip_prefix("0x")
        .or_else(|| src.strip_prefix("0X"))
        .and_then(|hex| i64::from_str_radix(hex, 16).ok())
        .or_else(|| src.parse::<i64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::source::Position;
    use std::path::Path;

    #[test]
    fn validate_track_formats() {
        let make_prop = |kind: &'static str| Property {
            pos: Position {
                file: Path::new("test.anm").into(),
                line: 1,
                col: 1,
            },
            kind,
            var: None,
            key: "val",
            rest: Some("0,100,0,1"),
        };

        let legacy_supported = [
            make_prop("track0"),
            make_prop("track1"),
            make_prop("track2"),
            make_prop("track3"),
            Property {
                pos: Position {
                    file: Path::new("test.anm").into(),
                    line: 1,
                    col: 1,
                },
                kind: "check0",
                var: None,
                key: "chk",
                rest: Some("0"),
            },
        ];
        assert!(validate_legacy_props(&legacy_supported, ".anm").is_ok());

        let legacy_unsupported = [make_prop("track 0")];
        assert!(validate_legacy_props(&legacy_unsupported, ".anm").is_err());

        let legacy_custom = [make_prop("track custom")];
        assert!(validate_legacy_props(&legacy_custom, ".anm").is_err());

        let legacy_check_custom = [Property {
            pos: Position {
                file: Path::new("test.anm").into(),
                line: 1,
                col: 1,
            },
            kind: "check custom",
            var: None,
            key: "chk",
            rest: Some("0"),
        }];
        assert!(validate_legacy_props(&legacy_check_custom, ".anm").is_err());

        let mut vars = HashSet::new();
        let compat_props = [
            make_prop("track 0"),
            make_prop("track custom"),
            make_prop("trackaaaaa"),
            make_prop("track AAA"),
            Property {
                pos: Position {
                    file: Path::new("test.anm").into(),
                    line: 1,
                    col: 1,
                },
                kind: "check custom",
                var: None,
                key: "chk",
                rest: Some("1"),
            },
        ];
        assert!(validate_compat_props(&compat_props, &mut vars).is_ok());
    }

    fn build_prop<'a>(kind: &'a str, var: Option<&'a str>, key: &'a str, rest: Option<&'a str>) -> Property<'a> {
        Property {
            pos: Position {
                file: Path::new("input.anm2").into(),
                line: 2,
                col: 3,
            },
            kind,
            var,
            key,
            rest,
        }
    }

    #[test]
    fn parse_integers() {
        for (text, expected) in [
            ("0", 0),
            ("-1", -1),
            ("0xffffff", 0xffffff),
            ("0XFF", 255),
            ("9223372036854775807", i64::MAX),
            ("-9223372036854775808", i64::MIN),
        ] {
            assert_eq!(parse_int(text), Some(expected), "{text}");
        }

        for text in ["", "1.0", "0x", "0xGG", "9223372036854775808", " 1"] {
            assert_eq!(parse_int(text), None, "{text}");
        }
    }

    #[test]
    fn validate_tracks() {
        for rest in ["0,100,0", "-1,1,1,1", "0,1,0,0.000000001", "0,1,1,0.1,unused,2"] {
            assert!(
                validate_track(&build_prop("track", Some("value"), "Value", Some(rest))).is_ok(),
                "{rest}"
            );
        }

        for rest in [
            "",
            "0,1",
            "0,1,2",
            "2,1,1",
            "NaN,1,0",
            "0,inf,0",
            "0,1,NaN",
            "0,1,0,0",
            "0,1,0,0.2",
            "0,1,0,NaN",
            "0,1,0,1,unused,inf",
        ] {
            let err = validate_track(&build_prop("track", Some("value"), "Value", Some(rest))).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{rest}: {err}");
        }
    }

    #[test]
    fn validate_selects() {
        assert!(validate_select(["first=0", "second=2"], "2").is_ok());
        assert!(validate_select(["name=with=equals=1"], "1").is_ok());

        for (items, default) in [
            (vec!["first=0"], "1"),
            (vec!["first=0", "second=0"], "0"),
            (vec!["effect.hidden=0"], "0"),
            (vec!["first"], "0"),
            (vec!["first=1.0"], "0"),
            (vec!["first=0"], "1.0"),
        ] {
            assert!(validate_select(items, default).is_err());
        }
    }

    #[test]
    fn validate_legacy_limits() {
        for suffix in [".anm", ".scn", ".ANM"] {
            assert!(validate_legacy_props(&[build_prop("track0", None, "Value", Some("0,1,0,0.01"))], suffix).is_ok());
        }
        assert!(validate_legacy_props(&[build_prop("track2", None, "Value", Some("0,1,0"))], ".scn").is_err());

        for (kind, key, rest) in [
            ("color", "-1", None),
            ("color", "16777216", None),
            ("check1", "Flag", Some("0")),
            ("track0", "Value", Some("0,1,0,0.001")),
            ("track0", "Value", Some("0,1,0,1,2")),
        ] {
            assert!(
                validate_legacy_props(&[build_prop(kind, None, key, rest)], ".anm").is_err(),
                "{kind}: {key}"
            );
        }

        for key in ["0", "0xffffff"] {
            assert!(validate_legacy_props(&[build_prop("color", None, key, None)], ".anm").is_ok());
        }

        for len in [254, 255, 256] {
            let key = "a".repeat(len);
            assert_eq!(
                validate_legacy_props(&[build_prop("param", None, &key, None)], ".anm").is_ok(),
                len <= 255
            );
        }
        assert!(validate_legacy_props(&[build_prop("param", None, "😀", None)], ".anm").is_err());

        for len in [16, 17] {
            let key = vec!["item"; len].join(";");
            assert_eq!(
                validate_legacy_props(&[build_prop("dialog", None, &key, None)], ".anm").is_ok(),
                len <= 16
            );
        }

        for ctrl in ["chk", "col", "fig"] {
            for len in [4, 5] {
                let key = vec![format!("item/{ctrl}"); len].join(";");
                assert_eq!(
                    validate_legacy_props(&[build_prop("dialog", None, &key, None)], ".anm").is_ok(),
                    len <= 4
                );
            }
        }
    }

    #[test]
    fn resolve_shader_calls() {
        let validate = |src: &str| {
            let map = source::Map::new(
                src,
                Position {
                    file: Path::new("input.lua").into(),
                    line: 1,
                    col: 1,
                },
            );
            let tokens = lua::Lexer::new(src).collect::<Result<Vec<_>, _>>().unwrap();
            validate_shader(&map, src, &[], &tokens)
        };

        for text in [
            "obj.pixelshader('external@file')",
            "local api = obj.computeshader; api('external@file')",
            "local api = obj.pixelshader; local function helper() api('external@file') end",
            "local function pixelshader() end; pixelshader('unknown')",
            "function pixelshader() end; pixelshader('unknown')",
            "local pixelshader = other; pixelshader('unknown')",
            "local api = obj.pixelshader; do local api = other; api('unknown') end",
            "local api = obj.pixelshader; function helper(api) api('unknown') end",
            "object.pixelshader('unknown')",
            "pixelshader('unknown')",
        ] {
            assert!(validate(text).is_ok(), "{text}");
        }

        for (text, message) in [
            ("obj.pixelshader('unknown')", "undefined pixelshader"),
            (
                "local api = obj.computeshader; api('unknown')",
                "undefined computeshader",
            ),
            ("obj.pixelshader()", "must be a string literal"),
            ("obj.pixelshader(1)", "must be a string literal"),
            ("obj.pixelshader('x' .. '@file')", "must be a string literal"),
        ] {
            let err = validate(text).unwrap_err();
            assert!(err.to_string().contains(message), "{text}: {err}");
            assert!(err.to_string().starts_with("input.lua:1:"), "{err}");
        }
    }

    #[test]
    fn validate_compat_duplicates() {
        for props in [
            vec![
                build_prop("track0", None, "A", Some("0,1,0")),
                build_prop("track0", None, "B", Some("0,1,0")),
            ],
            vec![
                build_prop("color", None, "0", None),
                build_prop("file", None, "path", None),
            ],
            vec![build_prop("param", None, "value=0;value=1", None)],
            vec![build_prop("param", None, "local=0", None)],
            vec![build_prop("dialog", None, "Flag/chk,value=2", None)],
            vec![build_prop("dialog", None, "Color/col,value=-1", None)],
        ] {
            assert!(validate_compat_props(&props, &mut HashSet::new()).is_err());
        }

        let mut vars = HashSet::from(["value".to_owned()]);
        assert!(validate_compat_props(&[build_prop("param", None, "value=0", None)], &mut vars).is_err());
        assert!(
            validate_compat_props(
                &[build_prop("param", None, "first=0;second=1", None)],
                &mut HashSet::new()
            )
            .is_ok()
        );
    }

    #[test]
    fn validate_modern_values() {
        for (kind, key, rest) in [
            ("check", "Flag", Some("true")),
            ("checksection", "Section", Some("false,true")),
            ("color", "Color", Some("nil")),
            ("select", "Choice=2", Some("first=0,second=2")),
            ("value", "Value", Some("{}")),
            ("data", "16000", None),
            ("hide", "Hidden", None),
            ("trackgroup", "Group", None),
        ] {
            assert!(
                validate_modern_props(&[build_prop(kind, Some("value"), key, rest)], &mut HashSet::new()).is_ok(),
                "{kind}"
            );
        }

        for (kind, key, rest) in [
            ("check", "Flag", None),
            ("check", "Flag", Some("2")),
            ("checksection", "Section", Some("0")),
            ("checksection", "Section", Some("true,0")),
            ("color", "Color", Some("-1")),
            ("select", "Choice=2", Some("first=0")),
            ("select", "Choice", None),
            ("value", "Value", None),
            ("data", "-1", None),
            ("data", "16001", None),
            ("script", "other", None),
            ("group", "Group", Some("0")),
        ] {
            let var = if matches!(kind, "script" | "group") {
                None
            } else {
                Some("value")
            };
            assert!(
                validate_modern_props(&[build_prop(kind, var, key, rest)], &mut HashSet::new()).is_err(),
                "{kind}"
            );
        }

        for var in ["local", "1value", "a.b", ""] {
            assert!(
                validate_modern_props(
                    &[build_prop("value", Some(var), "Value", Some("0"))],
                    &mut HashSet::new()
                )
                .is_err(),
                "{var}"
            );
        }

        let props = [
            build_prop("check", Some("value"), "A", Some("true")),
            build_prop("value", Some("value"), "B", Some("0")),
        ];
        assert!(validate_modern_props(&props, &mut HashSet::new()).is_err());
    }

    #[test]
    fn validate_transition_values() {
        for is_modern in [false, true] {
            for key in ["0", "-1.5", "1e2"] {
                assert!(validate_tra_props(&[build_prop("param", None, key, None)], is_modern).is_ok());
            }

            for key in ["NaN", "inf", "-inf", "invalid"] {
                assert!(
                    validate_tra_props(&[build_prop("param", None, key, None)], is_modern).is_err(),
                    "{key}"
                );
            }

            for (key, rest) in [("0", None), ("1", Some("0")), ("1", Some("1"))] {
                assert!(validate_tra_props(&[build_prop("speed", None, key, rest)], is_modern).is_ok());
            }

            for (key, rest) in [("2", None), ("0", Some("2")), ("1", Some(""))] {
                assert!(validate_tra_props(&[build_prop("speed", None, key, rest)], is_modern).is_err());
            }
        }

        for (key, rest) in [
            ("Value", "1.5"),
            ("Flag/check", "1"),
            ("Choice/select/first=0/second=2", "2"),
        ] {
            assert!(
                validate_tra_props(&[build_prop("param", None, key, Some(rest))], true).is_ok(),
                "{key}"
            );
        }

        for (key, rest) in [("Value", "NaN"), ("Flag/check", "true"), ("Choice/select/first=0", "1")] {
            assert!(
                validate_tra_props(&[build_prop("param", None, key, Some(rest))], true).is_err(),
                "{key}"
            );
        }
    }
}
