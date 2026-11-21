use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    name: &'a str,
    default: bool,
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
        let Some("check") = parts.next().map(str::trim) else {
            return Ok(None);
        };

        if parts.next().is_some() {
            bail!("{pos}: property 'check' expects no extra qualifiers after '/check', got '{head}'");
        }

        let (arg, rest) = tail
            .split_once(',')
            .map_or((tail, None), |(arg, rest)| (arg, Some(rest)));
        let arg = arg.trim();
        let default = match arg {
            "0" => false,
            "1" => true,
            _ => bail!("{pos}: default value of property 'check' must be '0' or '1', got '{arg}'"),
        };

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'check' expects at most 2 arguments, got {}",
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

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn label(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;

        if self.name.starts_with("effect.") {
            bail!(
                "{pos}: property name must not start with 'effect.', got '{}'",
                self.name
            );
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--param:{}/check,{}", self.name, u8::from(self.default))?;

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
    fn parses_check_prop() {
        let prop = Prop::parse(pos(), "param", " Flag / check , 1 ").unwrap().unwrap();
        assert_eq!(prop.name(), "Flag");
        assert_eq!(prop.label(), "Flag");
        assert!(prop.default);
        assert_eq!(prop.to_string(), "--param:Flag/check,1");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "Group::Enable/check, 0, extra")
            .unwrap()
            .unwrap();
        assert_eq!(prop.name(), "Group::Enable");
        assert_eq!(prop.label(), "Group::Enable");
        assert!(!prop.default);
        assert_eq!(prop.rest, Some(" extra"));
        assert_eq!(prop.to_string(), "--param:Group::Enable/check,0, extra");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "123/check,0").unwrap().unwrap();
        prop.validate().unwrap();

        assert!(Prop::parse(pos(), "speed", "0").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Flag/check").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Flag,1").unwrap().is_none());
        assert!(Prop::parse(pos(), "param", "Mode/select/A=0,0").unwrap().is_none());

        for body in ["Flag/check/extra,1", "Flag/check,true", "Flag/check,2", "Flag/check,"] {
            let err = Prop::parse(pos(), "param", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }

    #[test]
    fn validates_check_prop() {
        let prop = Prop::parse(pos(), "param", "effect.foo/check,1").unwrap().unwrap();
        let err = prop.validate().unwrap_err();
        assert!(err.to_string().starts_with("move.tra2:2:3:"), "{err}");
    }
}
