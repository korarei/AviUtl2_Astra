use crate::build::lang::astra;
use crate::build::source;
use crate::config::BuildTarget;
use anyhow::bail;
use indexmap::IndexMap;

pub(super) fn end(src: &str) -> usize {
    let rest = src.trim_start();
    if rest.starts_with('@') {
        src.len() - rest.len() + rest.find('\n').map_or(rest.len(), |at| at + 1)
    } else {
        0
    }
}

pub(super) fn extract(
    input: &source::Map,
    vars: &IndexMap<String, String>,
    is_bundled: bool,
) -> anyhow::Result<(Option<source::Map>, source::Map)> {
    let text = input.text();
    let src = text.as_ref();
    let rest = src.trim_start();
    let st = src.len() - rest.len();
    let first_ed = rest.find('\n').unwrap_or(rest.len());
    let first = &rest[..first_ed];
    let body_st = st + first_ed + usize::from(first_ed < rest.len());

    if first.starts_with('@') {
        if !is_bundled {
            bail!("{}: single target cannot have '@' header", input.locate(st));
        }

        let ed = st + first.trim_end().len();
        let header = astra::expand(&src[st..ed], vars, astra::Escape::None)
            .map_err(|err| anyhow::anyhow!("{}: {err}", input.locate(st + err.offset)))?;
        let mut dst = source::Map::default();
        dst.apply(input, st..ed, header.to_edits(st));
        return Ok((Some(dst), input.slice(body_st..)));
    }

    if let Some(tail) = first
        .strip_prefix("--")
        .map(|tail| tail.trim_start_matches('-').trim_start())
        .filter(|tail| tail.starts_with('@'))
    {
        if !is_bundled {
            tracing::warn!(
                "{}: single target has '--@' on the first line; leaving it as comment",
                input.locate(st)
            );
            return Ok((None, input.clone()));
        }

        let name_st = st + first.len() - tail.len() + 1;
        let ed = st + first.trim_end().len();
        let name = astra::expand(&src[name_st..ed], vars, astra::Escape::None)
            .map_err(|err| anyhow::anyhow!("{}: {err}", input.locate(name_st + err.offset)))?;
        let mut dst = source::Map::default();
        dst.emit(format!("@{}", name.text.trim()), input, st..ed);
        return Ok((Some(dst), input.slice(body_st..)));
    }

    Ok((None, input.clone()))
}

pub(super) fn normalize(
    header: Option<source::Map>,
    body: &source::Map,
    target: &BuildTarget,
    is_bundled: bool,
) -> anyhow::Result<source::Map> {
    let raw = body.text();
    let st = raw.len() - raw.trim_start().len();
    let text = raw.trim();

    if header.is_none() && text.is_empty() {
        return Ok(body.slice(0..0));
    }

    let mut dst = source::Map::default();
    let tail = body.slice(st..st + text.len());

    if let Some(header) = &header {
        dst.copy(header, ..);
    } else if is_bundled {
        let name = target.name();
        if name.is_empty() {
            bail!("failed to determine target name for target '{}'", target.path());
        }

        dst.emit(format!("@{name}"), &tail, 0..0);
    }

    if !text.is_empty() {
        if !dst.is_empty() {
            dst.emit("\n", &tail, 0..0);
        }

        if text.starts_with("--") {
            let indent = raw[..st].rfind('\n').map_or(0, |at| at + 1);
            dst.copy(body, indent..st);
        }

        dst.copy(&tail, ..);
        dst.emit("\n", &tail, tail.len()..tail.len());
    } else if let Some(header) = &header {
        dst.emit("\n", header, header.len()..header.len());
    }

    Ok(dst)
}
