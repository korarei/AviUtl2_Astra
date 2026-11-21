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
    default: Option<bool>,
    keyframed: Option<bool>,
    rest: Vec<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "checksection" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'checksection' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let name = parts.next().unwrap().trim();
        let count = parts.clone().count();

        if count < 1 {
            bail!("{pos}: property 'checksection' requires at least 1 argument, got {count}");
        }

        if count > 2 {
            tracing::warn!("{pos}: property 'checksection' accepts at most 2 arguments, got {count}");
        }

        let arg = parts.next().unwrap().trim();
        let default = match arg {
            "_" => None,
            "true" => Some(true),
            "false" => Some(false),
            _ => bail!("{pos}: default value of property 'checksection' must be 'true' or 'false', got '{arg}'"),
        };

        let keyframed = parts
            .next()
            .map(|arg| {
                let arg = arg.trim();
                match arg {
                    "true" => Ok(true),
                    "false" => Ok(false),
                    _ => bail!("{pos}: keyframed of property 'checksection' must be 'true' or 'false', got '{arg}'"),
                }
            })
            .transpose()?;

        let rest = parts.collect();

        Ok(Some(Self {
            pos,
            var,
            name,
            default,
            keyframed,
            rest,
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

        if let Some(val) = val {
            let truth = match val {
                "true" => true,
                "false" => false,
                _ => bail!("{pos}: initializer of variable '{var}' must be 'true' or 'false', got '{val}'"),
            };

            match self.default {
                Some(default) if default != truth => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                None => self.default = Some(truth),
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
                "{pos}: variable name '{}' of property 'checksection' must be a Lua identifier",
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

        if self.default.is_none() {
            bail!("{pos}: default value of property 'checksection' must be 'true' or 'false', got '_'");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "--checksection@{}:{},{}",
            self.var,
            self.name,
            self.default.unwrap_or(false)
        )?;

        if let Some(keyframed) = self.keyframed {
            write!(f, ",{keyframed}")?;
        }

        for arg in &self.rest {
            write!(f, ",{arg}")?;
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
    fn parses_checksection_prop() {
        let prop = Prop::parse(pos(), "checksection", "section", "  Section  , false ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "section");
        assert_eq!(prop.name(), "Section");
        assert_eq!(prop.label(), "Section");
        assert_eq!(prop.to_string(), "--checksection@section:Section,false");

        let keyframed = Prop::parse(pos(), "checksection", "section", "Group::Sub::Section, true , true ")
            .unwrap()
            .unwrap();
        assert_eq!(keyframed.name(), "Group::Sub::Section");
        assert_eq!(keyframed.label(), "Section");
        assert_eq!(
            keyframed.to_string(),
            "--checksection@section:Group::Sub::Section,true,true"
        );

        let extra = Prop::parse(pos(), "checksection", "section", "Section,false,true,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(
            extra.to_string(),
            "--checksection@section:Section,false,true,extra , more"
        );

        assert!(Prop::parse(pos(), "check", "flag", "Enable,true").unwrap().is_none());

        for body in [
            "Section",
            "Section,",
            "Section,0",
            "Section,1",
            "Section,true,0",
            "Section,true,_",
        ] {
            let err = Prop::parse(pos(), "checksection", "section", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut prop = Prop::parse(pos(), "checksection", "_", "Section, _").unwrap().unwrap();
        prop.bind("section", Some("false")).unwrap();
        assert_eq!(prop.to_string(), "--checksection@section:Section,false");

        let mut matching = Prop::parse(pos(), "checksection", "section", "Section,true")
            .unwrap()
            .unwrap();
        assert!(matching.bind("section", Some("true")).is_ok());

        let mut no_init = Prop::parse(pos(), "checksection", "_", "Section,true")
            .unwrap()
            .unwrap();
        no_init.bind("section", None).unwrap();
        assert_eq!(no_init.to_string(), "--checksection@section:Section,true");

        for (target, body, var, val) in [
            ("other", "Section,_", "section", Some("true")),
            ("section", "Section,_", "section", None),
            ("section", "Section,_", "section", Some("0")),
            ("section", "Section,_", "section", Some("1")),
            ("section", "Section,true", "section", Some("false")),
        ] {
            let mut invalid = Prop::parse(pos(), "checksection", target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_checksection_prop() {
        for (var, body) in [
            ("section", "Section,true"),
            ("section", "1section,true"),
            ("section", "Section,false,true"),
        ] {
            let prop = Prop::parse(pos(), "checksection", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("1invalid", "Section,true"),
            ("section", "effect.foo,true"),
            ("section", "123,true"),
            ("section", "Section,_"),
        ] {
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "checksection", var, body)
                .unwrap()
                .unwrap()
                .validate(&mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var}: {body}: {err}");
        }

        let prop = Prop::parse(pos(), "checksection", "section", "Section,true")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::from(["section"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Section")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
