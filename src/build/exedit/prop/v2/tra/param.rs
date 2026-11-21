use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    name: Option<&'a str>,
    default: f64,
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

        let (name, tail) = match body.split_once(',') {
            Some((head, tail)) => (Some(head.trim()), tail),
            None => (None, body),
        };

        let (arg, rest) = tail
            .split_once(',')
            .map_or((tail, None), |(arg, rest)| (arg, Some(rest)));
        let arg = arg.trim();
        let Some(default) = lua::parse_number(arg).filter(|val| val.is_finite()) else {
            bail!("{pos}: default value of property 'param' must be a number, got '{arg}'");
        };

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'param' expects at most 2 arguments, got {}",
                2 + rest.split(',').count()
            );
        }

        Ok(Some(Self {
            pos,
            name,
            default,
            rest,
        }))
    }

    pub(in crate::build::exedit) fn name(&self) -> Option<&'a str> {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> Option<&'a str> {
        self.name
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;

        if !(f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&self.default) {
            bail!(
                "{pos}: default value of property 'param' must be between {} and {}, got {}",
                i32::MIN,
                i32::MAX,
                self.default
            );
        }

        if let Some(name) = self.name
            && name.starts_with("effect.")
        {
            bail!("{pos}: property name must not start with 'effect.', got '{name}'");
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--param:")?;

        if let Some(name) = self.name {
            write!(f, "{name},")?;
        }

        write!(f, "{}", self.default)?;

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
            file: Path::new("move.tra2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_param_prop() {
        let prop = Prop::parse(pos(), "param", " 1e2 ").unwrap().unwrap();
        assert_eq!(prop.name(), None);
        assert_eq!(prop.label(), None);
        assert_eq!(prop.default, 100.0);
        assert_eq!(prop.to_string(), "--param:100");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "0x55").unwrap().unwrap();
        assert_eq!(prop.default, 85.0);
        assert_eq!(prop.to_string(), "--param:85");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", " Value , -1.5 ").unwrap().unwrap();
        assert_eq!(prop.name(), Some("Value"));
        assert_eq!(prop.label(), Some("Value"));
        assert_eq!(prop.default, -1.5);
        assert_eq!(prop.to_string(), "--param:Value,-1.5");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "Group::Label, 0, extra").unwrap().unwrap();
        assert_eq!(prop.name(), Some("Group::Label"));
        assert_eq!(prop.label(), Some("Group::Label"));
        assert_eq!(prop.rest, Some(" extra"));
        assert_eq!(prop.to_string(), "--param:Group::Label,0, extra");
        prop.validate().unwrap();

        for body in [", 0", "123, 0", "-2147483648", "2147483647", "Value,2147483646.5"] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            prop.validate().unwrap();
        }

        assert!(Prop::parse(pos(), "speed", "0").unwrap().is_none());

        for body in [
            "",
            "Choice/check",
            "Value,NaN",
            "Value,inf",
            "Value,-inf",
            "Value,",
            "1\n",
        ] {
            let err = Prop::parse(pos(), "param", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }

    #[test]
    fn validates_param_prop() {
        for body in [
            "effect.foo,0",
            "-2147483649",
            "2147483648",
            "Value,-2147483648.5",
            "Value,2147483647.5",
        ] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            let err = prop.validate().unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body}: {err}");
        }
    }
}
