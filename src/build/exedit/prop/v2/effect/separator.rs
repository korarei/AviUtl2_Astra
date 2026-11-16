use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: Option<&'a str>,
    name: &'a str,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: Option<&'a str>,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "separator" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'separator' cannot contain control characters");
        }

        let (name, rest) = body
            .split_once(',')
            .map_or((body.trim(), None), |(name, rest)| (name.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'separator' expects 0 arguments, got {}",
                rest.split(',').count()
            );
        }

        Ok(Some(Self { pos, var, name, rest }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> &'a str {
        self.name.rsplit_once("::").map_or(self.name, |(_, label)| label)
    }

    pub(in crate::build::exedit) fn validate(&self, vars: &mut HashSet<&'a str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if let Some(var) = self.var {
            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!("{pos}: variable name '{var}' of property 'separator' must be a Lua identifier");
            }

            if !vars.insert(var) {
                bail!("{pos}: duplicate variable name '{var}'");
            }
        }

        if self.name.starts_with("effect.") {
            bail!(
                "{pos}: property name must not start with 'effect.', got '{}'",
                self.name
            );
        }

        if lua::parse_number(self.name).is_some() {
            bail!("{pos}: property name must not be a number, got '{}'", self.name);
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.var {
            Some(var) => write!(f, "--separator@{var}:{}", self.name)?,
            None => write!(f, "--separator:{}", self.name)?,
        }

        if let Some(rest) = self.rest {
            write!(f, ",{rest}")?;
        }

        Ok(())
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
    fn parses_separator_prop() {
        let plain = Prop::parse(pos(), "separator", None, "  Section  ").unwrap().unwrap();
        assert_eq!(plain.var, None);
        assert_eq!(plain.name(), "Section");
        assert_eq!(plain.label(), "Section");
        assert_eq!(plain.to_string(), "--separator:Section");

        let scoped = Prop::parse(pos(), "separator", Some("sep"), "Group::Sub::Section")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.var, Some("sep"));
        assert_eq!(scoped.name(), "Group::Sub::Section");
        assert_eq!(scoped.label(), "Section");
        assert_eq!(scoped.to_string(), "--separator@sep:Group::Sub::Section");

        let empty = Prop::parse(pos(), "separator", None, "").unwrap().unwrap();
        assert_eq!(empty.name(), "");
        assert_eq!(empty.to_string(), "--separator:");

        let extra = Prop::parse(pos(), "separator", Some("sep"), "Section, extra, args")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--separator@sep:Section, extra, args");

        assert!(Prop::parse(pos(), "group", None, "Section").unwrap().is_none());
        assert!(Prop::parse(pos(), "separator", None, "Section\n").is_err());
    }

    #[test]
    fn validates_separator_prop() {
        for (var, body) in [(None, "Section"), (None, "1sep"), (Some("sep"), "Section"), (None, "")] {
            let prop = Prop::parse(pos(), "separator", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            prop.validate(&mut vars).unwrap();
        }

        for (var, body) in [
            (Some(""), "Section"),
            (Some("local"), "Section"),
            (Some("1bad"), "Section"),
            (None, "effect.sep"),
            (None, "123"),
        ] {
            let prop = Prop::parse(pos(), "separator", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let err = prop.validate(&mut vars).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var:?}: {err}");
        }

        let prop = Prop::parse(pos(), "separator", Some("sep"), "Section")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::from(["sep"]);
        assert!(prop.validate(&mut vars).is_err());
    }
}
