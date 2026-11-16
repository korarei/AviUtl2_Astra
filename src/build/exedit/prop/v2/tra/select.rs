use crate::build::source::Pos;
use anyhow::bail;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    name: &'a str,
    default: i64,
    items: Vec<Item<'a>>,
    rest: Option<&'a str>,
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

        let Some((head, tail)) = body.split_once(',') else {
            return Ok(None);
        };

        let mut parts = head.split('/');
        let name = parts.next().unwrap().trim();
        let Some("select") = parts.next().map(str::trim) else {
            return Ok(None);
        };

        let mut items = Vec::new();
        for item in parts {
            let Some((name, val)) = item.rsplit_once('=') else {
                bail!("{pos}: item of 'select' must contain '=', got '{item}'");
            };

            let val = val.trim();
            let Ok(val) = val.parse::<i64>() else {
                bail!("{pos}: item value of property 'select' must be an integer, got '{val}'");
            };

            items.push(Item { name: name.trim(), val });
        }

        if items.is_empty() {
            bail!("{pos}: property 'select' requires at least 1 item, got 0");
        }

        let (arg, rest) = tail
            .split_once(',')
            .map_or((tail, None), |(arg, rest)| (arg, Some(rest)));
        let arg = arg.trim();
        let Ok(default) = arg.parse::<i64>() else {
            bail!("{pos}: default value of property 'select' must be an integer, got '{arg}'");
        };

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'select' expects at most 2 arguments, got {}",
                2 + rest.split(',').count()
            );
        }

        Ok(Some(Self {
            pos,
            name,
            default,
            items,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn labels(&self) -> Vec<&'a str> {
        std::iter::once(self.name)
            .chain(self.items.iter().map(|item| item.name))
            .collect()
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.name.starts_with("effect.") {
            bail!(
                "{pos}: property name must not start with 'effect.', got '{}'",
                self.name
            );
        }

        let mut item_vals = HashSet::new();

        for item in &self.items {
            if !item_vals.insert(item.val) {
                bail!("{pos}: duplicate item value '{}' in property 'select'", item.val);
            }
        }

        if !item_vals.contains(&self.default) {
            bail!(
                "{pos}: default value '{}' of property 'select' is not in items",
                self.default
            );
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--param:{}/select", self.name)?;

        for item in &self.items {
            write!(f, "/{item}")?;
        }

        write!(f, ",{}", self.default)?;

        if let Some(rest) = self.rest {
            write!(f, ",{rest}")?;
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
            file: Path::new("move.tra2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_select_prop() {
        let prop = Prop::parse(pos(), "param", " Choice / select / first = 0 / second = 2 , 2 ")
            .unwrap()
            .unwrap();
        assert_eq!(prop.name(), "Choice");
        assert_eq!(prop.labels(), ["Choice", "first", "second"]);
        assert_eq!(prop.default, 2);
        assert_eq!(prop.to_string(), "--param:Choice/select/first=0/second=2,2");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "Grp::Mode/select/A=1/A=2,1,extra")
            .unwrap()
            .unwrap();
        assert_eq!(prop.name(), "Grp::Mode");
        assert_eq!(prop.labels(), ["Grp::Mode", "A", "A"]);
        let scoped = Prop::parse(pos(), "param", "Group::Mode/select/Group::A=0/Group::A=1,0")
            .unwrap()
            .unwrap();
        assert_eq!(scoped.labels(), ["Group::Mode", "Group::A", "Group::A"]);

        assert_eq!(prop.rest, Some("extra"));
        assert_eq!(prop.to_string(), "--param:Grp::Mode/select/A=1/A=2,1,extra");
        prop.validate().unwrap();

        for body in ["Mode/select/effect.a=0,0", "123/select/A=0,0", "Mode/select/123=0,0"] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            prop.validate().unwrap();
        }

        assert!(Prop::parse(pos(), "speed", "0").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Mode/select/A=0").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Flag/check,1").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Value,0").unwrap().is_none());

        for body in [
            "Mode/select,0",
            "Mode/select/,0",
            "Mode/select/A,0",
            "Mode/select/A=1.5,0",
            "Mode/select/A=0,abc",
            "Mode/select/A=0,",
        ] {
            let err = Prop::parse(pos(), "param", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }

    #[test]
    fn allows_duplicate_item_names() {
        for body in [
            "Mode/select/A=0/A=1,0",
            "/select/=0/=1,0",
            "Group::Mode/select/Group::A=0/Other::A=1,0",
        ] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            prop.validate().unwrap();
        }

        for body in ["Mode/select/A=0/B=0,0", "Mode/select/=0/=0,0"] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            assert!(prop.validate().is_err(), "{body}");
        }
    }

    #[test]
    fn validates_select_prop() {
        for body in ["effect.foo/select/A=0,0", "Mode/select/A=1/B=1,1", "Mode/select/A=0,2"] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            let err = prop.validate().unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body}: {err}");
        }
    }
}
