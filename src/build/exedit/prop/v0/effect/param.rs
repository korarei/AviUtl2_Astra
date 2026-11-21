use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::collections::HashSet;
use std::fmt::{self, Write};

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    items: Vec<Item<'a>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "param" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'param' cannot contain control characters");
        }

        let src = body.trim();
        let src = src.strip_suffix(';').map_or(src, str::trim_end);

        if src.is_empty() {
            bail!("{pos}: property 'param' requires at least 1 item, got 0");
        }

        let items = src
            .split(';')
            .map(|item| {
                let Some((var, val)) = item.split_once('=') else {
                    bail!("{pos}: item of property 'param' must contain '=', got '{item}'");
                };

                let var = var.trim();
                let val = val.trim();

                if var.is_empty() {
                    bail!("{pos}: variable name of item in property 'param' cannot be empty");
                }

                if val.is_empty() {
                    bail!("{pos}: default value of '{var}' in property 'param' cannot be empty");
                }

                Ok(Item { var, val })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        Ok(Some(Self { pos, items }))
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;
        let mut body = String::new();
        let mut vars = HashSet::new();

        for item in &self.items {
            if !lua::is_ident(item.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!(
                    "{pos}: variable name '{}' of property 'param' must be a Lua identifier",
                    item.var
                );
            }

            if !vars.insert(item.var) {
                bail!("{pos}: duplicate variable name '{}' in property 'param'", item.var);
            }

            write!(body, "{item};").unwrap();
        }

        let (bytes, _, has_unmappable) = encoding_rs::SHIFT_JIS.encode(&body);

        if has_unmappable {
            bail!("{pos}: property 'param' contains characters that cannot be encoded in Shift_JIS");
        }

        if bytes.len() > 255 {
            bail!(
                "{pos}: property 'param' must be at most 255 bytes in Shift_JIS, got {}",
                bytes.len()
            );
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--param:")?;

        for item in &self.items {
            write!(f, "{item};")?;
        }

        Ok(())
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
struct Item<'a> {
    var: &'a str,
    val: &'a str,
}

impl fmt::Display for Item<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.var, self.val)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("input.anm").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_param_prop() {
        let prop = Prop::parse(pos(), "param", " dx = 10 ; dy = 20 ; ").unwrap().unwrap();
        assert_eq!(prop.to_string(), "--param:dx=10;dy=20;");

        let single = Prop::parse(pos(), "param", "t={1,2}").unwrap().unwrap();
        assert_eq!(single.to_string(), "--param:t={1,2};");

        assert!(Prop::parse(pos(), "dialog", "dx=10").unwrap().is_none());

        for body in ["", "   ", ";", "dx", "dx=10;;dy=20", "=10", "dx=", "dx=10\n"] {
            let err = Prop::parse(pos(), "param", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn validates_param_prop() {
        let valid = Prop::parse(pos(), "param", "dx=10;dy=20;").unwrap().unwrap();
        valid.validate().unwrap();

        let exact_255 = format!("a={}", "0".repeat(252));
        Prop::parse(pos(), "param", &exact_255)
            .unwrap()
            .unwrap()
            .validate()
            .unwrap();

        let over_255 = format!("a={}", "0".repeat(253));
        let sjis_over = format!("a=\"{}\"", "あ".repeat(126));

        for body in ["local=1", "1a=1", "dx=10;dx=20", "a=\"👍\"", &over_255, &sjis_over] {
            let err = Prop::parse(pos(), "param", body)
                .unwrap()
                .unwrap()
                .validate()
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }
}
