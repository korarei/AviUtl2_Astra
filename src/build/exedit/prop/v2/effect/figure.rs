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
    var: &'a str,
    name: &'a str,
    default: Option<Cow<'a, str>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "figure" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'figure' cannot contain control characters");
        }

        let Some((name, arg)) = body.split_once(',') else {
            bail!("{pos}: property 'figure' requires at least 1 argument, got 0");
        };

        let arg = arg.trim();
        let default = (arg != "_").then_some(Cow::Borrowed(arg));

        Ok(Some(Self {
            pos,
            var,
            name: name.trim(),
            default,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> &'a str {
        self.name.rsplit_once("::").map_or(self.name, |(_, label)| label)
    }

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.var != "_" && self.var != var {
            bail!("{pos}: variable name '{}' conflicts with '{var}'", self.var);
        }

        if let Some(Cow::Borrowed(default)) = &mut self.default {
            *default = default.trim();

            if *default == "_" {
                self.default = None;
            }
        }

        if let Some(val) = val {
            let Some(text) = lua::parse_string(val).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? else {
                bail!("{pos}: initializer of variable '{var}' must be a string, got '{val}'");
            };

            if text.chars().any(char::is_control) {
                bail!("{pos}: initializer of variable '{var}' cannot contain control characters");
            }

            match &self.default {
                None => self.default = Some(Cow::Owned(text.into_owned())),
                Some(default) if default.as_ref() != text.as_ref() => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                Some(_) => {}
            }
        } else if self.default.is_none() {
            bail!("{pos}: variable '{var}' requires an initializer");
        }

        self.var = var;

        Ok(())
    }

    pub(in crate::build::exedit) fn validate(
        &self,
        vars: &mut HashSet<&'a str>,
        names: &mut HashSet<Cow<'a, str>>,
    ) -> anyhow::Result<()> {
        let pos = &self.pos;

        if !lua::is_ident(self.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
            bail!(
                "{pos}: variable name '{}' of property 'figure' must be a Lua identifier",
                self.var
            );
        }

        if !vars.insert(self.var) {
            bail!("{pos}: duplicate variable name '{}'", self.var);
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
        let default = self.default.as_deref().unwrap_or("_");
        write!(f, "--figure@{}:{},{default}", self.var, self.name)
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
    fn parses_figure_prop() {
        let prop = Prop::parse(pos(), "figure", "fig", "  Figure ,  円  ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "fig");
        assert_eq!(prop.name(), "Figure");
        assert_eq!(prop.label(), "Figure");
        assert_eq!(prop.to_string(), "--figure@fig:Figure,円");

        let scoped = Prop::parse(pos(), "figure", "fig", "Group::Sub::Figure,_")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Figure");
        assert_eq!(scoped.label(), "Figure");
        assert_eq!(scoped.to_string(), "--figure@fig:Group::Sub::Figure,_");

        let comma = Prop::parse(pos(), "figure", "fig", "Figure,  a,  b.fig ")
            .unwrap()
            .unwrap();
        assert_eq!(comma.to_string(), "--figure@fig:Figure,a,  b.fig");

        assert!(Prop::parse(pos(), "font", "fig", "Figure,円").unwrap().is_none());

        assert!(Prop::parse(pos(), "figure", "fig", "Figure").is_err());
        assert!(Prop::parse(pos(), "figure", "fig", "Figure,a\tb").is_err());
    }

    #[test]
    fn binds_figure_prop() {
        let mut prop = Prop::parse(pos(), "figure", "_", "Figure,_").unwrap().unwrap();
        prop.bind("fig", Some(r#""円""#)).unwrap();
        assert_eq!(prop.var, "fig");
        assert_eq!(prop.to_string(), "--figure@fig:Figure,円");

        let mut explicit = Prop::parse(pos(), "figure", "_", "Figure,円").unwrap().unwrap();
        explicit.bind("fig", None).unwrap();
        assert_eq!(explicit.to_string(), "--figure@fig:Figure,円");

        let mut unbound = Prop::parse(pos(), "figure", "_", "Figure,_").unwrap().unwrap();
        assert!(unbound.bind("fig", None).is_err());
        assert!(unbound.bind("fig", Some(r#""bad\nfig""#)).is_err());
        assert!(unbound.bind("fig", Some(r#""bad\0fig""#)).is_err());
        assert!(unbound.bind("fig", Some(r#""\xff""#)).is_err());
        assert!(prop.bind("other", Some(r#""円""#)).is_err());
        assert!(prop.bind("fig", Some(r#""四角形""#)).is_err());
        assert!(prop.bind("fig", Some("123")).is_err());
    }

    #[test]
    fn validates_figure_prop() {
        for body in ["Figure,円", "1fig,円", "Figure,_"] {
            let prop = Prop::parse(pos(), "figure", "fig", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("", "Figure,円"),
            ("local", "Figure,円"),
            ("1bad", "Figure,円"),
            ("fig", "effect.fig,円"),
            ("fig", "123,円"),
        ] {
            let prop = Prop::parse(pos(), "figure", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{var}: {body}");
        }

        let prop = Prop::parse(pos(), "figure", "fig", "Figure,円").unwrap().unwrap();
        let mut vars = HashSet::from(["fig"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Figure")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
