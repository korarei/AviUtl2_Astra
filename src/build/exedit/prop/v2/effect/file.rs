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
    is_compat: bool,
    var: &'a str,
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
        if kind != "file" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'file' cannot contain control characters");
        }

        let is_compat = var.is_none();
        let (var, name, rest) = if let Some(var) = var {
            let (name, rest) = body
                .split_once(',')
                .map_or((body.trim(), None), |(name, rest)| (name.trim(), Some(rest)));
            (var, name, rest)
        } else {
            ("file", "ファイル", (!body.trim().is_empty()).then_some(body))
        };

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'file' expects 0 arguments, got {}",
                rest.split(',').count()
            );
        }

        Ok(Some(Self {
            pos,
            is_compat,
            var,
            name,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn can_bind(&self) -> bool {
        !self.is_compat
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
                tracing::warn!("{pos}: initializer of variable '{var}' is ignored by property 'file'");
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

        if self.is_compat {
            tracing::warn!("{pos}: property 'file' uses legacy syntax");
        } else if !lua::is_ident(self.var).map_err(|err| anyhow::anyhow!("{pos}: {err}"))? {
            bail!(
                "{pos}: variable name '{}' of property 'file' must be a Lua identifier",
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
        if self.is_compat {
            write!(f, "--file:{}", self.rest.unwrap_or(""))
        } else if let Some(rest) = self.rest {
            write!(f, "--file@{}:{},{rest}", self.var, self.name)
        } else {
            write!(f, "--file@{}:{}", self.var, self.name)
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
    fn parses_file_prop() {
        let prop = Prop::parse(pos(), "file", Some("path"), "  File  ").unwrap().unwrap();
        assert_eq!(prop.var, "path");
        assert_eq!(prop.name(), "File");
        assert_eq!(prop.label(), "File");
        assert_eq!(prop.to_string(), "--file@path:File");

        let scoped = Prop::parse(pos(), "file", Some("path"), "Group::Sub::File")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.name(), "Group::Sub::File");
        assert_eq!(scoped.label(), "File");

        let legacy = Prop::parse(pos(), "file", None, "  ").unwrap().unwrap();
        assert_eq!(legacy.var, "file");
        assert_eq!(legacy.name(), "ファイル");
        assert_eq!(legacy.label(), "ファイル");
        assert_eq!(legacy.to_string(), "--file:");

        let extra = Prop::parse(pos(), "file", Some("path"), "File,extra , more")
            .unwrap()
            .unwrap();
        assert_eq!(extra.to_string(), "--file@path:File,extra , more");

        let legacy_extra = Prop::parse(pos(), "file", None, "path, more").unwrap().unwrap();
        assert_eq!(legacy_extra.to_string(), "--file:path, more");

        for (kind, var, body) in [("folder", Some("dir"), "Folder"), ("color", Some("col"), "Color,0")] {
            assert!(Prop::parse(pos(), kind, var, body).unwrap().is_none(), "{kind}: {body}");
        }
    }

    #[test]
    fn binds_var() {
        let mut prop = Prop::parse(pos(), "file", Some("_"), "File").unwrap().unwrap();
        prop.bind("path", Some("\"\"")).unwrap();
        assert_eq!(prop.to_string(), "--file@path:File");

        let mut path = Prop::parse(pos(), "file", Some("_"), "File").unwrap().unwrap();
        path.bind("path", Some(r#""C:\\image.png""#)).unwrap();
        assert_eq!(path.to_string(), "--file@path:File");

        let mut compat_bind = Prop::parse(pos(), "file", None, "").unwrap().unwrap();
        compat_bind.bind("file", Some("\"\"")).unwrap();
        assert_eq!(compat_bind.to_string(), "--file:");

        let mut no_init = Prop::parse(pos(), "file", Some("_"), "File").unwrap().unwrap();
        no_init.bind("path", None).unwrap();
        assert_eq!(no_init.to_string(), "--file@path:File");

        for (target, body, var, val) in [
            (None, "", "path", Some("\"\"")),
            (None, "", "file", Some("0")),
            (Some("other"), "File", "path", Some("\"\"")),
            (Some("path"), "File", "path", Some("0")),
            (Some("path"), "File", "path", Some("nil")),
            (Some("path"), "File", "path", Some(r#""a\nb""#)),
            (Some("path"), "File", "path", Some(r#""a\tb""#)),
            (Some("path"), "File", "path", Some(r#""a\0b""#)),
            (Some("path"), "File", "path", Some(r#""\xff""#)),
        ] {
            let mut invalid = Prop::parse(pos(), "file", target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_file_prop() {
        for (var, body) in [(Some("path"), "File"), (Some("path"), "1file"), (None, "")] {
            let prop = Prop::parse(pos(), "file", var, body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            (Some("1invalid"), "File"),
            (Some("path"), "effect.file"),
            (Some("path"), "123"),
        ] {
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "file", var, body)
                .unwrap()
                .unwrap()
                .validate(&mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{err}");
        }

        let prop = Prop::parse(pos(), "file", Some("path"), "File").unwrap().unwrap();
        let mut vars = HashSet::from(["path"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("File")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
