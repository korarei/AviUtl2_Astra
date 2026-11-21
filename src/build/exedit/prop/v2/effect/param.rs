use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    items: Vec<Item<'a>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: Option<&'a str>,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "param" {
            return Ok(None);
        }

        if var.is_some() {
            bail!("{pos}: property 'param' cannot have a variable");
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'param' cannot contain control characters");
        }

        let body = body.trim();
        let body = body.strip_suffix(';').unwrap_or(body);
        if body.is_empty() {
            bail!("{pos}: property 'param' requires at least 1 item");
        }

        let mut items = Vec::new();
        for item in body.split(';') {
            let Some((var, val)) = item.split_once('=') else {
                bail!("{pos}: item of 'param' must contain '=', got '{item}'");
            };

            items.push(Item {
                var: var.trim(),
                val: val.trim(),
            });
        }

        Ok(Some(Self { pos, items }))
    }

    pub(in crate::build::exedit) fn names(&self) -> Vec<&'a str> {
        self.items.iter().map(|item| item.var).collect()
    }

    pub(in crate::build::exedit) fn labels(&self) -> Vec<&'a str> {
        self.names()
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        tracing::warn!("{pos}: property 'param' uses legacy syntax");

        for item in &self.items {
            let var = item.var;

            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!("{pos}: variable name '{var}' of property 'param' must be a Lua identifier");
            }

            if !vars.insert(var) {
                bail!("{pos}: duplicate variable name '{var}'");
            }

            if lua::parse_number(var).is_some() {
                bail!("{pos}: property name must not be a number, got '{var}'");
            }

            if !names.insert(Cow::Borrowed(var)) {
                bail!("{pos}: duplicate property name '{var}'");
            }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_param_prop() {
        let prop = Prop::parse(pos(), "param", None, " dx = 10 ; dy = 20 ; tbl = {1, 2} ; ")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        prop.validate(&mut vars, &mut names).unwrap();
        assert_eq!(prop.names(), ["dx", "dy", "tbl"]);
        assert_eq!(prop.labels(), ["dx", "dy", "tbl"]);
        assert_eq!(vars, HashSet::from(["dx", "dy", "tbl"]));
        assert_eq!(
            names,
            HashSet::from([Cow::Borrowed("dx"), Cow::Borrowed("dy"), Cow::Borrowed("tbl")])
        );
        assert_eq!(prop.to_string(), "--param:dx=10;dy=20;tbl={1, 2};");

        let without_trailing = Prop::parse(pos(), "param", None, "dx=10").unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        without_trailing.validate(&mut vars, &mut names).unwrap();
        assert_eq!(without_trailing.to_string(), "--param:dx=10;");

        assert!(Prop::parse(pos(), "dialog", None, "dx=10").unwrap().is_none());
    }

    #[test]
    fn allows_long_param_without_v0_limits() {
        let long_val = "0".repeat(300);
        let body = format!("x={long_val}");
        let prop = Prop::parse(pos(), "param", None, &body).unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        prop.validate(&mut vars, &mut names).unwrap();
    }

    #[test]
    fn rejects_invalid_param_prop() {
        assert!(Prop::parse(pos(), "param", Some("x"), "dx=10").is_err());
        assert!(Prop::parse(pos(), "param", None, "dx=10\0").is_err());
        assert!(Prop::parse(pos(), "param", None, "  ").is_err());
        assert!(Prop::parse(pos(), "param", None, ";").is_err());
        assert!(Prop::parse(pos(), "param", None, "dx").is_err());
        assert!(Prop::parse(pos(), "param", None, "dx=10;;dy=20").is_err());

        for body in ["1x=10", "local=10", "inf=10", "dx=10;dy=20;dx=30"] {
            let prop = Prop::parse(pos(), "param", None, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{body}");
        }

        let prop = Prop::parse(pos(), "param", None, "dx=10").unwrap().unwrap();
        let mut vars = HashSet::from(["dx"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("dx")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
