mod desc;
mod element;
mod format;
mod header;
mod l10n;
mod process;
mod prop;
mod shader;

use crate::build::lang::lua as lang_lua;
use crate::build::script::lua;
use crate::build::source::{self, Pos};
use crate::config::BuildTarget;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Output {
    pub script: String,
    pub l10n: String,
}

impl Output {
    pub fn append(&mut self, src: &Self) {
        self.script.push_str(&src.script);
        self.l10n.push_str(&src.l10n);
    }

    pub fn replace(&mut self, from: &str, to: &str) {
        self.script = self.script.replace(from, to);
        self.l10n = self.l10n.replace(from, to);
    }
}

pub fn build(
    src: &str,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    is_bundled: bool,
    suffix: &str,
) -> anyhow::Result<Output> {
    let input = source::Map::new(
        src,
        Pos {
            file: Path::new(target.path()).into(),
            line: 1,
            col: 1,
        },
    );
    let (header, input) = header::extract(&input, vars, is_bundled)?;
    format::dispatch!(suffix, |F| build_for::<F>(
        suffix,
        header,
        &input,
        target,
        include_dirs,
        vars,
        is_bundled
    ))
}

fn build_for<F: format::Spec>(
    suffix: &str,
    header: Option<source::Map>,
    input: &source::Map,
    target: &BuildTarget,
    include_dirs: &[PathBuf],
    vars: &IndexMap<String, String>,
    is_bundled: bool,
) -> anyhow::Result<Output> {
    let body = lua::build(input, target, include_dirs, vars, |comment, ctx| {
        if comment.bracket.is_some() {
            return Ok(false);
        }

        let is_inline = !ctx.prefix.is_empty();
        if is_inline && !is_local_decl(ctx.tokens) {
            return Ok(false);
        }

        let Some(annotation) = element::Annotation::parse(comment.content) else {
            return Ok(false);
        };

        format::should_expand::<F>(suffix, &annotation, is_inline)
    })?;
    let map = header::normalize(header, &body, target, is_bundled)?;
    let output = process::run::<F>(
        suffix,
        &map,
        &shader::Processor {
            target,
            include_dirs,
            vars,
        },
    )?;

    Ok(Output {
        l10n: output.l10n.render(target.key()),
        script: output.map.text().into_owned(),
    })
}

fn is_local_decl(tokens: &[lang_lua::Token<'_>]) -> bool {
    let st = tokens
        .iter()
        .position(|token| token.kind != lang_lua::TokenKind::Semicolon)
        .unwrap_or(tokens.len());
    let ed = tokens
        .iter()
        .rposition(|token| token.kind != lang_lua::TokenKind::Semicolon)
        .map_or(0, |at| at + 1);

    let [first, second, rest @ ..] = &tokens[st.min(ed)..ed] else {
        return false;
    };

    if first.kind != lang_lua::TokenKind::Local || second.kind.name().is_none() {
        return false;
    }

    match rest {
        [] => true,
        [assign, expr @ ..] if assign.kind == lang_lua::TokenKind::Assign => {
            let mut depth = 0usize;

            for token in expr {
                match token.kind {
                    lang_lua::TokenKind::LParen | lang_lua::TokenKind::LBracket | lang_lua::TokenKind::LBrace => {
                        depth += 1;
                    }
                    lang_lua::TokenKind::RParen | lang_lua::TokenKind::RBracket | lang_lua::TokenKind::RBrace => {
                        depth = depth.saturating_sub(1);
                    }
                    lang_lua::TokenKind::Comma | lang_lua::TokenKind::Semicolon if depth == 0 => return false,
                    _ => {}
                }
            }

            depth == 0
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combine_outputs() {
        let mut dst = Output {
            script: "first".to_owned(),
            l10n: "First=\n".to_owned(),
        };
        dst.append(&Output {
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
    fn selective_comments_header_and_shader_isolation() {
        let vars = IndexMap::from([
            ("HEADER".to_owned(), "MyEffect".to_owned()),
            ("MAX".to_owned(), "100".to_owned()),
            ("SHADER".to_owned(), "psmain".to_owned()),
        ]);

        let bundled_src = concat!(
            "--@${HEADER}\n",
            "--#define LOCAL_MAX ${MAX}\n",
            "--#define SHADER shadowed\n",
            "--track@x:Scale,0,${LOCAL_MAX},50,1\n",
            "---@type ${MISSING}\n",
            "-- ${MISSING}\n",
            "--[[pixelshader@${SHADER}:\n",
            "// ${MISSING}\n",
            "--#ifndef LOCAL_MAX\n",
            "float4 psmain() : SV_Target { return 0; }\n",
            "--#endif\n",
            "]]\n",
            "local y = x\n",
            "local z = 1 --track@_:Bound,0,${MAX},_,1\n",
            "local u = 0 --group:${MISSING}\n",
        );
        let target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "input.anm2" })).unwrap();
        let bundled = build(bundled_src, &target, &[], &vars, true, ".anm2").unwrap();
        assert_eq!(
            bundled.script,
            concat!(
                "@MyEffect\n",
                "--track@x:Scale,0,100,50,1\n",
                "---@type ${MISSING}\n",
                "-- ${MISSING}\n",
                "--[[pixelshader@psmain:\n",
                "// ${MISSING}\n",
                "float4 psmain() : SV_Target { return 0; }\n",
                "]]\n",
                "local y = x\n",
                "--track@z:Bound,0,100,1,1\n",
                "local u = 0 --group:${MISSING}\n",
            )
        );

        let single_src = "--@${MISSING}\nlocal y = 1\n";
        let single = build(single_src, &target, &[], &vars, false, ".anm2").unwrap();
        assert_eq!(single.script, single_src);
    }

    #[test]
    fn selective_expansion_for_prop_and_desc_annotations() {
        let vars = IndexMap::from([
            ("INFO".to_owned(), "v1.0".to_owned()),
            ("LABEL".to_owned(), "Blur".to_owned()),
            ("REQ".to_owned(), "2003500".to_owned()),
            ("RUNTIME".to_owned(), "luaJIT".to_owned()),
            ("CAT".to_owned(), "General".to_owned()),
            ("ACC".to_owned(), "1".to_owned()),
            ("DEC".to_owned(), "0".to_owned()),
            ("VAR".to_owned(), "x".to_owned()),
        ]);

        let anm_target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "fx.anm2" })).unwrap();
        let anm_src = concat!(
            "--information:Effect ${INFO}\n",
            "--label:${LABEL}\n",
            "--require:${REQ}\n",
            "--script:${RUNTIME}\n",
            "--filter\n",
            "--hidemenu\n",
            "--group:Settings,true\n",
            "--track@x:Scale,0,100,50,1\n",
            "--hide@${VAR}\n",
            "--group\n",
            "--filter out ${MISSING}\n",
            "-- ${MISSING}\n",
            "--[[track@x:${MISSING}]]\n",
            "--[[speed:${MISSING}]]\n",
        );
        let anm = build(anm_src, &anm_target, &[], &vars, false, ".anm2").unwrap();
        assert_eq!(
            anm.script,
            concat!(
                "--information:Effect v1.0\n",
                "--label:Blur\n",
                "--require:2003500\n",
                "--script:luaJIT\n",
                "--filter\n",
                "--hidemenu\n",
                "--group:Settings,true\n",
                "--track@x:Scale,0,100,50,1\n",
                "--hide@x\n",
                "--group\n",
                "--filter out ${MISSING}\n",
                "-- ${MISSING}\n",
                "--[[track@x:${MISSING}]]\n",
                "--[[speed:${MISSING}]]\n",
            )
        );
        assert!(anm.l10n.contains("Settings=\n"));
        assert!(anm.l10n.contains("Scale=\n"));

        let tra_target: BuildTarget = serde_json::from_value(serde_json::json!({ "path": "move.tra2" })).unwrap();
        let tra_src = concat!(
            "--twopoint\n",
            "--timecontrol\n",
            "--speed:${ACC},${DEC}\n",
            "--param:${CAT},0\n",
        );
        let tra = build(tra_src, &tra_target, &[], &vars, false, ".tra2").unwrap();
        assert_eq!(
            tra.script,
            concat!(
                "--twopoint\n",
                "--timecontrol\n",
                "--speed:1,0\n",
                "--param:General,0\n",
            )
        );
        assert!(tra.l10n.contains("General=\n"));
    }
}
