use super::element::{Annotation, Binding, State};
use super::format::{self, Spec};
use super::{header, l10n, shader};
use crate::build::lang::lua::{self, TokenKind};
use crate::build::source;

pub(super) struct Output {
    pub map: source::Map,
    pub l10n: l10n::Collector,
}

struct Context<'s> {
    map: &'s source::Map,
    src: &'s str,
    assigns: &'s [lua::Assign],
    state: State<'s>,
    dst: source::Map,
    l10n: l10n::Collector,
    curr: usize,
}

pub(super) fn run<F: Spec>(suffix: &str, map: &source::Map, shader: &shader::Processor<'_>) -> anyhow::Result<Output> {
    let src = map.text();
    let (tokens, analysis) = parse_lua(map, &src)?;
    let mut shaders = Vec::new();
    let mut ctx = Context {
        map,
        src: &src,
        assigns: &analysis.assigns,
        state: State::default(),
        dst: source::Map::default(),
        l10n: F::L10N.collector(),
        curr: 0,
    };

    for token in &tokens {
        let TokenKind::Comment(comment) = &token.kind else {
            continue;
        };

        if comment.bracket.is_some() {
            if let Some((shader_map, block)) = shader.process(map, &src, token, F::HAS_SHADER)? {
                shaders.push(block);
                ctx.dst.copy(map, ctx.curr..token.span.st);
                ctx.dst.copy(&shader_map, ..);
                ctx.curr = token.span.ed;
            } else if let Some(annotation) = Annotation::parse(comment.content.trim_start())
                && format::is_known::<F>(&annotation)
            {
                tracing::warn!(
                    "{}: property '{}' must use '--...'; leaving it as comment",
                    map.locate(token.span.st),
                    annotation.kind
                );
            }

            continue;
        }

        process_annotation::<F>(suffix, token, comment.content, &mut ctx)?;
    }

    ctx.dst.copy(map, ctx.curr..map.len());

    if F::HAS_SHADER {
        shader::validate(&shaders, map, &tokens, &analysis.calls)?;
    }

    Ok(Output {
        map: ctx.dst,
        l10n: ctx.l10n,
    })
}

fn parse_lua<'s>(map: &source::Map, src: &'s str) -> anyhow::Result<(Vec<lua::Token<'s>>, lua::Analysis<'s>)> {
    let tokens = lua::Lexer::new_at(src, header::end(src))
        .map_err(|err| anyhow::anyhow!("{}: {err}", map.locate(err.offset)))?
        .filter(|token| {
            token.as_ref().map_or(true, |token| {
                !matches!(token.kind, TokenKind::Whitespace | TokenKind::Newline)
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
    let analysis = lua::Parser::new(src, &tokens)
        .parse()
        .map_err(|err| anyhow::anyhow!("{}: {}", map.locate(err.offset), err.message))?;
    Ok((tokens, analysis))
}

fn process_annotation<'s, F: Spec>(
    suffix: &str,
    token: &lua::Token<'s>,
    raw: &'s str,
    ctx: &mut Context<'s>,
) -> anyhow::Result<()> {
    let Some(annotation) = Annotation::parse(raw.trim_start()) else {
        return Ok(());
    };
    let pos = ctx.map.locate(token.span.st);

    if (annotation.body.is_some() || annotation.var.is_some())
        && annotation.kind.bytes().any(|byte| byte.is_ascii_uppercase())
    {
        tracing::warn!(
            "{pos}: property kind must be lowercase, got '{}'; leaving it as comment",
            annotation.kind
        );
        return Ok(());
    }

    if F::HAS_SHADER && annotation.var.is_some() && matches!(annotation.kind, "pixelshader" | "computeshader") {
        tracing::warn!("{pos}: shader block must use '--[[...]]'; leaving it as comment");
        return Ok(());
    }

    let prefix = ctx.src[..token.span.st].rsplit('\n').next().unwrap();
    let is_inline = !prefix.trim().is_empty();
    let has_space = raw.starts_with(char::is_whitespace);

    if has_space {
        if format::is_known::<F>(&annotation) {
            tracing::warn!(
                "{pos}: whitespace between '--' and property '{}' is not allowed; leaving it as comment",
                annotation.kind
            );
        }

        return Ok(());
    }

    if !is_inline && !prefix.is_empty() {
        if format::is_known::<F>(&annotation) {
            tracing::warn!(
                "{pos}: whitespace before '--' for property '{}' is not allowed; leaving it as comment",
                annotation.kind
            );
        }

        return Ok(());
    }

    let Some(element) = format::parse::<F>(suffix, pos.clone(), annotation)? else {
        if is_inline && annotation.var.is_some() && annotation.body.is_some() && !annotation.kind.starts_with('-') {
            tracing::warn!(
                "{pos}: unsupported property '{}'; leaving it as comment",
                annotation.kind
            );
        }

        return Ok(());
    };
    let (st, binding) = if is_inline {
        let Some((st, binding)) = element
            .can_bind()
            .then(|| find_binding(ctx.src, ctx.assigns, token.span.st, ctx.curr))
            .flatten()
        else {
            tracing::warn!(
                "{pos}: property '{}' must follow a single local variable declaration; leaving it as comment",
                annotation.kind
            );
            return Ok(());
        };
        (st, Some(binding))
    } else {
        (token.span.st, None)
    };

    let text = element.process(binding, &mut ctx.state, &mut ctx.l10n)?;
    ctx.dst.copy(ctx.map, ctx.curr..st);
    ctx.dst.emit(text, ctx.map, token.span.st..token.span.ed);
    ctx.curr = token.span.ed;
    Ok(())
}

fn find_binding<'a>(src: &'a str, assigns: &[lua::Assign], ed: usize, curr: usize) -> Option<(usize, Binding<'a, 'a>)> {
    let assign = assigns[..assigns.partition_point(|assign| assign.span.ed <= ed)].last()?;
    let [target] = assign.targets.as_slice() else {
        return None;
    };
    let line_st = src[..assign.span.st].rfind('\n').map_or(0, |at| at + 1);
    let prefix = &src[line_st..assign.span.st];
    let st = if prefix.contains(';') { line_st } else { assign.span.st };

    if !src[assign.span.st..target.st].starts_with("local")
        || assign.vals.len() > 1
        || st < curr
        || !prefix
            .chars()
            .chain(src[assign.span.ed..ed].chars())
            .all(|c| c.is_whitespace() || c == ';')
    {
        return None;
    }

    Some((
        st,
        Binding {
            var: &src[target.st..target.ed],
            val: assign.vals.first().map(|val| &src[val.st..val.ed]),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::super::format;
    use super::*;
    use crate::build::source::Pos;
    use crate::config::BuildTarget;
    use indexmap::IndexMap;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    struct TestWriter(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for TestWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().write(bytes)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn replaces_elements_and_preserves_origins() -> anyhow::Result<()> {
        let src = "local value = \"日本語\" --string@value:名前, _\n--require: 2003500,extra\nreturn value\n";
        let map = source::Map::new(
            src,
            Pos {
                file: Path::new("input.anm2").into(),
                line: 1,
                col: 1,
            },
        );
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.anm2" }))?;
        let vars = IndexMap::new();
        let output = run::<format::v2::Anm>(
            ".anm2",
            &map,
            &shader::Processor {
                target: &target,
                include_dirs: &[],
                vars: &vars,
            },
        )?;
        let text = output.map.text();
        assert_eq!(
            text,
            "--string@value:名前,日本語\n--require:2003500,extra\nreturn value\n"
        );
        assert_eq!(
            output.l10n.render("input"),
            "[input]\ninput=\n名前=\n\n[Tips.input]\neffect.name=\n名前=\n\n"
        );
        assert_eq!(
            output.map.locate(0).to_string(),
            map.locate(src.find("--string").unwrap()).to_string()
        );
        assert_eq!(
            output.map.locate(text.find("return").unwrap()).to_string(),
            map.locate(src.find("return").unwrap()).to_string()
        );
        Ok(())
    }

    #[test]
    fn leaves_spaced_indented_and_unbound_annotations_as_comments() -> anyhow::Result<()> {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Arc::clone(&bytes);
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .with_writer(move || TestWriter(Arc::clone(&writer)))
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.lua" }))?;
        let vars = IndexMap::new();
        let shader = shader::Processor {
            target: &target,
            include_dirs: &[],
            vars: &vars,
        };

        for (suffix, src, expected_warns) in [
            (
                ".anm2",
                "-- color: red\n",
                &["whitespace between '--' and property 'color'"] as &[&str],
            ),
            (
                ".anm2",
                "  --hidemenu:extra\n",
                &["whitespace before '--' for property 'hidemenu'"],
            ),
            (
                ".anm2",
                "foo() --track0:Label,0,10,0\n",
                &["property 'track0' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "local x = 1 --data@buf:4\n",
                &["property 'data' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "aaa = 0 --track@x:Label,0,10,0\n",
                &["property 'track' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "obj.aaa = 0 --track@x:Label,0,10,0\n",
                &["property 'track' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "local a, b = 0, 0 --track@x:Label,0,10,0\n",
                &["property 'track' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "if aaaa then --track@x:Label,0,10,0\nend\n",
                &["property 'track' must follow a single local variable declaration"],
            ),
            (
                ".anm2",
                "if aaaa then -- track@x:Label,0,10,0\nend\n",
                &["whitespace between '--' and property 'track' is not allowed"],
            ),
            (
                ".anm2",
                "--pixelshader@main:float4 main() : SV_Target { return 0; }\n",
                &["shader block must use '--[[...]]'"],
            ),
            (
                ".anm2",
                "-- computeshader@main\n",
                &["shader block must use '--[[...]]'"],
            ),
            (
                ".anm2",
                "--[[track@x:Scale,0,10,0]]\n",
                &["property 'track' must use '--...'"],
            ),
            (".anm2", "--[[filter]]\n", &["property 'filter' must use '--...'"]),
            (".anm2", "--[[speed:1,0]]\n", &[]),
            (
                ".anm2",
                "--[[track@x:Scale,0,10,0]]\n",
                &["property 'track' must use '--...'"],
            ),
            (".anm2", "--[[filter]]\n", &["property 'filter' must use '--...'"]),
            (".anm2", "--[[speed:1,0]]\n", &[]),
            (".anm2", "-- pixelshader memo\n", &[]),
            (".anm2", "local x = 1 -- foo@bar: baz\n", &[]),
            (".anm", "-- filter: blur\n", &[]),
            (".tra2", "  --group: note\n", &[]),
        ] {
            bytes.lock().unwrap().clear();
            let map = source::Map::new(
                src,
                Pos {
                    file: Path::new("input.lua").into(),
                    line: 1,
                    col: 1,
                },
            );
            let output = match suffix {
                ".anm2" => run::<format::v2::Anm>(suffix, &map, &shader)?,
                ".anm" => run::<format::v0::Effect>(suffix, &map, &shader)?,
                ".tra2" => run::<format::v2::Tra>(suffix, &map, &shader)?,
                _ => unreachable!(),
            };
            assert_eq!(output.map.text(), src, "{suffix}: {src}");
            let warnings = String::from_utf8(bytes.lock().unwrap().clone())?;
            assert_eq!(
                warnings.lines().count(),
                expected_warns.len(),
                "{suffix}: {src}: {warnings}"
            );

            for expected in expected_warns {
                assert!(warnings.contains(expected), "{suffix}: {src}: {warnings}");
            }
        }

        Ok(())
    }
}
