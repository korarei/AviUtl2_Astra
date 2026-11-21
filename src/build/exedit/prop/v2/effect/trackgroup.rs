use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::{Borrow, Cow};
use std::collections::HashSet;
use std::fmt;
use std::hash::Hash;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    vars: Vec<&'a str>,
    name: &'a str,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        vars: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "trackgroup" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'trackgroup' cannot contain control characters");
        }

        let (name, rest) = body
            .split_once(',')
            .map_or((body, None), |(name, rest)| (name, Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'trackgroup' expects 0 arguments, got {}",
                rest.split(',').count()
            );
        }

        Ok(Some(Self {
            pos,
            vars: vars.split(',').map(str::trim).collect(),
            name: name.trim(),
            rest,
        }))
    }

    pub(in crate::build::exedit) fn validate<S>(
        &self,
        vars: &HashSet<S>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()>
    where
        S: Borrow<str> + Eq + Hash,
    {
        let pos = &self.pos;

        if !(2..=3).contains(&self.vars.len()) {
            bail!(
                "{pos}: property 'trackgroup' requires 2 or 3 variables, got {}",
                self.vars.len()
            );
        }

        for &var in &self.vars {
            if !lua::is_ident(var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
                bail!("{pos}: target variable '{var}' of property 'trackgroup' must be a Lua identifier");
            }

            if !vars.contains(var) {
                bail!("{pos}: undefined target variable '{var}' of property 'trackgroup'");
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

        if !names.insert(Cow::Borrowed(self.name)) {
            bail!("{pos}: duplicate property name '{}'", self.name);
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--trackgroup@")?;

        for (i, var) in self.vars.iter().enumerate() {
            if i > 0 {
                f.write_str(",")?;
            }

            f.write_str(var)?;
        }

        write!(f, ":{}", self.name)?;

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
    fn parses_trackgroup_prop() {
        let prop = Prop::parse(pos(), "trackgroup", "x , y , z", "  Group  ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.to_string(), "--trackgroup@x,y,z:Group");

        let extra = Prop::parse(pos(), "trackgroup", "a,b", "Group,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--trackgroup@a,b:Group,extra , more");

        assert!(Prop::parse(pos(), "track", "a", "Label,0,10,0").unwrap().is_none());
    }

    #[test]
    fn validates_trackgroup_prop() {
        let pool = HashSet::from(["x", "y", "z", "w"]);
        for (vars, body) in [("x,y", "Pos"), ("x,y", "1grp"), ("x,y,z", "Pos")] {
            let prop = Prop::parse(pos(), "trackgroup", vars, body).unwrap().unwrap();
            let mut names = HashSet::new();
            prop.validate(&pool, &mut names).unwrap();
        }

        let owned = HashSet::from(["x".to_owned(), "y".to_owned()]);
        let prop = Prop::parse(pos(), "trackgroup", "x,y", "Pos").unwrap().unwrap();
        let mut names = HashSet::new();
        prop.validate(&owned, &mut names).unwrap();

        for (vars, body) in [
            ("", "Group"),
            ("x", "Group"),
            ("x,", "Group"),
            ("1invalid,y", "Group"),
            ("x,local", "Group"),
            ("x,v", "Group"),
            ("x,y,z,w", "Group"),
            ("x,y", "effect.grp"),
            ("x,y", "123"),
        ] {
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "trackgroup", vars, body)
                .unwrap()
                .unwrap()
                .validate(&pool, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{vars}: {err}");
        }

        let prop = Prop::parse(pos(), "trackgroup", "x,y", "Pos").unwrap().unwrap();
        let mut names = HashSet::from([Cow::Borrowed("Pos")]);
        assert!(prop.validate(&pool, &mut names).is_err());
    }
}
