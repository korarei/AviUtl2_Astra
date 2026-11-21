use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Default {
    Omitted,
    Unset,
    Int(i64),
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    var: &'a str,
    name: &'a str,
    default: Default,
    items: Vec<Item<'a>>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        var: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "select" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'select' cannot contain control characters");
        }

        let Some((head, rest)) = body.split_once(',') else {
            bail!("{pos}: property 'select' requires at least 1 item, got 0");
        };

        if rest.trim().is_empty() {
            bail!("{pos}: property 'select' requires at least 1 item, got 0");
        }

        let (name, default) = match head.rsplit_once('=') {
            Some((name, val)) => {
                let name = name.trim();
                let val = val.trim();
                let default = if val == "_" {
                    Default::Unset
                } else if let Ok(val) = val.parse::<i64>() {
                    Default::Int(val)
                } else {
                    bail!("{pos}: default value of property 'select' must be an integer, got '{val}'");
                };
                (name, default)
            }
            None => (head.trim(), Default::Omitted),
        };

        let mut items = Vec::new();
        for item in rest.split(',') {
            let Some((name, val)) = item.rsplit_once('=') else {
                bail!("{pos}: item of 'select' must contain '=', got '{item}'");
            };

            let val = val.trim();
            let Ok(val) = val.parse::<i64>() else {
                bail!("{pos}: item value of property 'select' must be an integer, got '{val}'");
            };

            items.push(Item { name: name.trim(), val });
        }

        Ok(Some(Self {
            pos,
            var,
            name,
            default,
            items,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn labels(&self) -> Vec<&'a str> {
        std::iter::once(self.name.rsplit_once("::").map_or(self.name, |(_, label)| label))
            .chain(self.items.iter().map(|item| item.name))
            .collect()
    }

    pub(in crate::build::exedit) fn bind(&mut self, var: &'a str, val: Option<&str>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.var != "_" && self.var != var {
            bail!("{pos}: variable name '{}' conflicts with '{var}'", self.var);
        }

        if let Some(val) = val {
            let Some(num) = lua::parse_integer(val) else {
                bail!("{pos}: initializer of variable '{var}' must be an integer, got '{val}'");
            };

            match self.default {
                Default::Omitted if !self.items.iter().any(|item| item.val == num) => {
                    bail!("{pos}: initializer of variable '{var}' must match a select item value, got '{val}'");
                }
                Default::Unset => self.default = Default::Int(num),
                Default::Int(default) if default != num => {
                    bail!("{pos}: default value '{default}' conflicts with '{val}'");
                }
                Default::Omitted | Default::Int(_) => {}
            }
        } else if self.default == Default::Unset {
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
                "{pos}: variable name '{}' of property 'select' must be a Lua identifier",
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

        if self.default == Default::Unset {
            bail!("{pos}: default value of property 'select' must be an integer, got '_'");
        }

        let mut item_names = HashSet::new();
        let mut item_vals = HashSet::new();

        for item in &self.items {
            if lua::parse_number(item.name).is_some() {
                bail!(
                    "{pos}: item name of property 'select' must not be a number, got '{}'",
                    item.name
                );
            }

            if !item_names.insert(item.name) {
                bail!("{pos}: duplicate item '{}' in property 'select'", item.name);
            }

            if !item_vals.insert(item.val) {
                bail!("{pos}: duplicate item value {} in property 'select'", item.val);
            }
        }

        if let Default::Int(default) = self.default
            && !item_vals.contains(&default)
        {
            bail!("{pos}: default value {default} of property 'select' must match an item value");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--select@{}:{}", self.var, self.name)?;

        match self.default {
            Default::Omitted => {}
            Default::Unset => {
                let min = self.items.iter().map(|item| item.val).min().unwrap_or(0);
                write!(f, "={min}")?;
            }
            Default::Int(default) => write!(f, "={default}")?,
        }

        for item in &self.items {
            write!(f, ",{item}")?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Item<'a> {
    name: &'a str,
    val: i64,
}

impl fmt::Display for Item<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}={}", self.name, self.val)
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
    fn parses_select_prop() {
        let prop = Prop::parse(pos(), "select", "mode", " Group::Choice = 2 , first = 0 , second = 2 ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.var, "mode");
        assert_eq!(prop.name(), "Group::Choice");
        assert_eq!(prop.labels(), ["Choice", "first", "second"]);
        assert_eq!(prop.to_string(), "--select@mode:Group::Choice=2,first=0,second=2");

        let scoped = Prop::parse(pos(), "select", "mode", "Group::Choice=0,Group::A=0,Other::A=1")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.labels(), ["Choice", "Group::A", "Other::A"]);

        let omitted_default = Prop::parse(pos(), "select", "mode", "Choice,second=2,first=1")
            .unwrap()
            .unwrap();
        assert_eq!(omitted_default.to_string(), "--select@mode:Choice,second=2,first=1");

        let equals_in_item = Prop::parse(pos(), "select", "mode", "Choice=1,a=b=1").unwrap().unwrap();
        assert_eq!(equals_in_item.labels(), ["Choice", "a=b"]);
        assert_eq!(equals_in_item.to_string(), "--select@mode:Choice=1,a=b=1");

        let unbound = Prop::parse(pos(), "select", "_", "Choice=_,second=2,first=1")
            .unwrap()
            .unwrap();
        assert_eq!(unbound.to_string(), "--select@_:Choice=1,second=2,first=1");

        assert!(Prop::parse(pos(), "check", "mode", "Choice,true").unwrap().is_none());

        for body in [
            "Choice",
            "Choice,",
            "Choice=abc,first=0",
            "Choice=1.0,first=0",
            "Choice=0,first",
            "Choice=0,first=1.0",
            "Choice=0,first=0\n",
        ] {
            let err = Prop::parse(pos(), "select", "mode", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn binds_var_and_default() {
        let mut prop = Prop::parse(pos(), "select", "_", "Choice = _, first = 1, second = 2")
            .unwrap()
            .unwrap();
        prop.bind("mode", Some("2")).unwrap();
        assert_eq!(prop.to_string(), "--select@mode:Choice=2,first=1,second=2");

        let mut explicit = Prop::parse(pos(), "select", "_", "Choice=2,first=1,second=2")
            .unwrap()
            .unwrap();
        explicit.bind("mode", None).unwrap();
        assert_eq!(explicit.to_string(), "--select@mode:Choice=2,first=1,second=2");

        let mut no_default = Prop::parse(pos(), "select", "_", "Choice,second=2,first=1")
            .unwrap()
            .unwrap();
        no_default.bind("mode", None).unwrap();
        assert_eq!(no_default.to_string(), "--select@mode:Choice,second=2,first=1");

        let mut matching_omitted = Prop::parse(pos(), "select", "_", "Choice,second=2,first=1")
            .unwrap()
            .unwrap();
        matching_omitted.bind("mode", Some("2")).unwrap();
        assert_eq!(matching_omitted.to_string(), "--select@mode:Choice,second=2,first=1");

        let mut matching = Prop::parse(pos(), "select", "mode", "Choice=2,first=2")
            .unwrap()
            .unwrap();
        matching.bind("mode", Some("0x2")).unwrap();

        for (target, body, var, val) in [
            ("other", "Choice=_,first=0", "mode", Some("0")),
            ("mode", "Choice=_,first=0", "mode", None),
            ("mode", "Choice=_,first=0", "mode", Some("1.5")),
            ("mode", "Choice=_,first=0", "mode", Some("true")),
            ("mode", "Choice=0,first=0", "mode", Some("1")),
            ("mode", "Choice,second=3,first=1", "mode", Some("2")),
            ("mode", "Choice,second=3,first=1", "mode", Some("4")),
        ] {
            let mut invalid = Prop::parse(pos(), "select", target, body).unwrap().unwrap();
            assert!(invalid.bind(var, val).is_err(), "{body}: {var}={val:?}");
        }
    }

    #[test]
    fn validates_name_scopes() {
        let mut vars = HashSet::new();
        let mut names = HashSet::new();

        for (var, body) in [
            ("first", "First=0,Group::A=0,Other::A=1"),
            ("second", "Second=1,Group::A=1"),
        ] {
            let prop = Prop::parse(pos(), "select", var, body).unwrap().unwrap();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for body in [
            "Choice=0,=0,=1",
            "Choice=0,Group::A=0,Group::A=1",
            "Choice=0,Group::A=0,Other::A=0",
        ] {
            let prop = Prop::parse(pos(), "select", "mode", body).unwrap().unwrap();
            assert!(
                prop.validate(&mut HashSet::new(), &mut HashSet::new()).is_err(),
                "{body}"
            );
        }

        let mut vars = HashSet::new();
        let mut names = HashSet::new();
        let first = Prop::parse(pos(), "select", "first", "=0,=0").unwrap().unwrap();
        first.validate(&mut vars, &mut names).unwrap();
        let second = Prop::parse(pos(), "select", "second", "=1,=1").unwrap().unwrap();
        assert!(second.validate(&mut vars, &mut names).is_err());
    }

    #[test]
    fn validates_select_prop() {
        for body in [
            "Choice=2,first=0,second=2",
            "Choice,second=2,first=1",
            "1choice=0,1item=0",
            "Choice=0,effect.item=0",
        ] {
            let prop = Prop::parse(pos(), "select", "mode", body).unwrap().unwrap();
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            prop.validate(&mut vars, &mut names).unwrap();
        }

        for (var, body) in [
            ("1invalid", "Choice=0,first=0"),
            ("local", "Choice=0,first=0"),
            ("mode", "effect.choice=0,first=0"),
            ("mode", "123=0,first=0"),
            ("mode", "Choice=_,first=0"),
            ("mode", "Choice=1,first=0"),
            ("mode", "Choice=0,123=0"),
            ("mode", "Choice=0,first=0,first=1"),
            ("mode", "Choice=0,first=0,second=0"),
        ] {
            let mut vars = HashSet::new();
            let mut names = HashSet::new();
            let err = Prop::parse(pos(), "select", var, body)
                .unwrap()
                .unwrap()
                .validate(&mut vars, &mut names)
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{var}: {body}: {err}");
        }

        let prop = Prop::parse(pos(), "select", "mode", "Choice=0,first=0")
            .unwrap()
            .unwrap();
        let mut vars = HashSet::from(["mode"]);
        let mut names = HashSet::new();
        assert!(prop.validate(&mut vars, &mut names).is_err());

        let mut vars = HashSet::new();
        let mut names = HashSet::from([Cow::Borrowed("Choice")]);
        assert!(prop.validate(&mut vars, &mut names).is_err());
    }
}
