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
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "folder" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'folder' cannot contain control characters");
        }

        let (name, rest) = body
            .split_once(',')
            .map_or((body.trim(), None), |(name, rest)| (name.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'folder' expects 0 arguments, got {}",
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

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.var != "_" && self.var != var {
            bail!("{pos}: variable name '{}' conflicts with '{var}'", self.var);
        }

        if let Some(val) = val {
            let Some(text) = lua::parse_string(val).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? else {
                bail!("{pos}: initializer of variable '{var}' must be a string, got '{val}'");
            };

            if text.chars().any(char::is_control) {
                bail!("{pos}: initializer of variable '{var}' cannot contain control characters");
            }

            if !text.is_empty() {
                tracing::warn!("{pos}: initializer of variable '{var}' is ignored by property 'folder'");
            }
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
                "{pos}: variable name '{}' of property 'folder' must be a Lua identifier",
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
        if let Some(rest) = self.rest {
            write!(f, "--folder@{}:{},{rest}", self.var, self.name)
        } else {
            write!(f, "--folder@{}:{}", self.var, self.name)
        }
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
    fn parses_folder_prop() {
        let prop = Prop::parse(pos(), "folder", "dir", "  Folder  ").unwrap().unwrap();
        assert_eq!(prop.var, "dir");
        assert_eq!(prop.name(), "Folder");
        assert_eq!(prop.label(), "Folder");
        assert_eq!(prop.to_string(), "--folder@dir:Folder");

        let scoped = Prop::parse(pos(), "folder", "dir", "Group::Sub::Folder")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::Folder");
        assert_eq!(scoped.label(), "Folder");

        let extra = Prop::parse(pos(), "folder", "dir", "Folder, extra, args")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--folder@dir:Folder, extra, args");

        assert!(Prop::parse(pos(), "file", "dir", "Folder").unwrap().is_none());
    }

    #[test]
    fn binds_folder_prop() {
        let mut prop = Prop::parse(pos(), "folder", "_", "Folder").unwrap().unwrap();
        prop.bind("dir", Some(r#""""#)).unwrap();
        assert_eq!(prop.var, "dir");
        assert_eq!(prop.to_string(), "--folder@dir:Folder");

        let mut prop = Prop::parse(pos(), "folder", "dir", "Folder").unwrap().unwrap();
        prop.bind("dir", None).unwrap();
        prop.bind("dir", Some(r#""C:\\folder""#)).unwrap();

        assert!(prop.bind("other", Some(r#""""#)).is_err());
        assert!(prop.bind("dir", Some("123")).is_err());
        assert!(prop.bind("dir", Some("nil")).is_err());
        assert!(prop.bind("dir", Some(r#""bad\npath""#)).is_err());
        assert!(prop.bind("dir", Some(r#""bad\0path""#)).is_err());
        assert!(prop.bind("dir", Some(r#""\xff""#)).is_err());
    }

    #[test]
    fn validates_folder_prop() {
        for body in ["Folder", "1dir"] {
            let prop = Prop::parse(pos(), "folder", "dir", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("", "Folder"),
            ("local", "Folder"),
            ("1bad", "Folder"),
            ("dir", "effect.dir"),
            ("dir", "123"),
        ] {
            let prop = Prop::parse(pos(), "folder", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            assert!(prop.validate(&mut vars, &mut names).is_err(), "{var}: {body}");
        }

        let prop = Prop::parse(pos(), "folder", "dir", "Folder").unwrap().unwrap();
        let mut vars = HashSet::from(["dir"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Folder")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
