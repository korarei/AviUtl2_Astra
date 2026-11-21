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
    var: Option<&'a str>,
    name: Option<&'a str>,
    folding: Option<bool>,
    rest: Vec<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: Option<&'a str>,
        body: Option<&'a str>,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "group" {
            return Ok(None);
        }

        if body.is_some_and(|body| body.chars().any(char::is_control)) {
            bail!("{pos}: property 'group' cannot contain control characters");
        }

        let (name, folding, rest) = match body {
            None => {
                if var.is_some() {
                    bail!("{pos}: property 'group' without ':' cannot have a variable");
                }

                (None, None, Vec::new())
            }
            Some(body) => {
                let mut parts = body.split(',');
                let name = parts.next().unwrap().trim();
                let folding = parts.next();
                let rest: Vec<_> = parts.collect();

                if !rest.is_empty() {
                    tracing::warn!(
                        "{pos}: property 'group' accepts at most 1 argument, got {}",
                        rest.len() + 1
                    );
                }

                let folding = folding
                    .map(|arg| {
                        let arg = arg.trim();
                        match arg {
                            "true" => Ok(true),
                            "false" => Ok(false),
                            _ => bail!("{pos}: folding of property 'group' must be 'true' or 'false', got '{arg}'"),
                        }
                    })
                    .transpose()?;

                (Some(name), folding, rest)
            }
        };

        Ok(Some(Self {
            pos,
            var,
            name,
            folding,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> Option<&'a str> {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> Option<&'a str> {
        self.name
            .map(|name| name.rsplit_once("::").map_or(name, |(_, label)| label))
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        if let Some(var) = self.var {
            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!("{pos}: variable name '{var}' of property 'group' must be a Lua identifier");
            }

            if !vars.insert(var) {
                bail!("{pos}: duplicate variable name '{var}'");
            }
        }

        if let Some(name) = self.name {
            if name.starts_with("effect.") {
                bail!("{pos}: property name must not start with 'effect.', got '{name}'");
            }

            if lua::parse_number(name).is_some() {
                bail!("{pos}: property name must not be a number, got '{name}'");
            }

            if !names.insert(Cow::Borrowed(name)) {
                bail!("{pos}: duplicate property name '{name}'");
            }
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--group")?;

        if let Some(var) = self.var {
            write!(f, "@{var}")?;
        }

        if let Some(name) = self.name {
            write!(f, ":{name}")?;

            if let Some(folding) = self.folding {
                write!(f, ",{folding}")?;
            }

            for arg in &self.rest {
                write!(f, ",{arg}")?;
            }
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
    fn parses_group_prop() {
        let end = Prop::parse(pos(), "group", None, None).unwrap().unwrap();
        assert_eq!(end.var, None);
        assert_eq!(end.name(), None);
        assert_eq!(end.label(), None);
        assert_eq!(end.to_string(), "--group");

        let plain = Prop::parse(pos(), "group", None, Some("  Group  ")).unwrap().unwrap();
        assert_eq!(plain.name(), Some("Group"));
        assert_eq!(plain.label(), Some("Group"));
        assert_eq!(plain.to_string(), "--group:Group");

        let folded = Prop::parse(pos(), "group", Some("grp"), Some("Sub::Group,  false  "))
            .unwrap()
            .unwrap();
        assert_eq!(folded.var, Some("grp"));
        assert_eq!(folded.name(), Some("Sub::Group"));
        assert_eq!(folded.label(), Some("Group"));
        assert_eq!(folded.to_string(), "--group@grp:Sub::Group,false");

        let extra = Prop::parse(pos(), "group", None, Some("Group, true, extra, args"))
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--group:Group,true, extra, args");

        let unnamed = Prop::parse(pos(), "group", None, Some("")).unwrap().unwrap();
        assert_eq!(unnamed.name(), Some(""));
        assert_eq!(unnamed.to_string(), "--group:");

        assert!(Prop::parse(pos(), "separator", None, Some("Group")).unwrap().is_none());

        for (var, body) in [
            (Some("grp"), None),
            (None, Some("Group\n")),
            (None, Some("Group,0")),
            (None, Some("Group,1")),
            (None, Some("Group,")),
        ] {
            assert!(Prop::parse(pos(), "group", var, body).is_err(), "{var:?}:{body:?}");
        }
    }

    #[test]
    fn validates_group_prop() {
        for (var, body) in [
            (None, None),
            (None, Some("Group")),
            (None, Some("1grp")),
            (Some("grp"), Some("Group,true")),
        ] {
            let prop = Prop::parse(pos(), "group", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        let end = Prop::parse(pos(), "group", None, None).unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        end.validate(&mut vars, &mut names).unwrap();
        end.validate(&mut vars, &mut names).unwrap();
        assert!(names.is_empty());

        let unnamed = Prop::parse(pos(), "group", None, Some("")).unwrap().unwrap();
        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        unnamed.validate(&mut vars, &mut names).unwrap();
        assert_eq!(names, HashSet::from([Cow::Borrowed("")]));
        assert!(unnamed.validate(&mut vars, &mut names).is_err());

        for (var, body) in [
            (Some(""), Some("Group")),
            (Some("local"), Some("Group")),
            (Some("1bad"), Some("Group")),
            (None, Some("effect.grp")),
            (None, Some("123")),
        ] {
            let prop = Prop::parse(pos(), "group", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = prop.validate(&mut vars, &mut names).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var:?}: {err}");
        }

        let prop = Prop::parse(pos(), "group", Some("grp"), Some("Group"))
            .unwrap()
            .unwrap();
        let mut vars = HashSet::from(["grp"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Group")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
