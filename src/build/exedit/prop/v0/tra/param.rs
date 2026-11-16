use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop {
    pos: Pos,
    default: i64,
}

#[allow(dead_code)]
impl Prop {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &str) -> anyhow::Result<Option<Self>> {
        if kind != "param" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'param' cannot contain control characters");
        }

        let mut args = body.split(',');
        let arg = args.next().unwrap().trim();

        let extra = args.count();
        if extra > 0 {
            bail!("{pos}: property 'param' expects 1 argument, got {}", 1 + extra);
        }

        let Some(default) = lua::parse_integer(arg) else {
            bail!("{pos}: default value of property 'param' must be an integer, got '{arg}'");
        };

        Ok(Some(Self { pos, default }))
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;

        if i32::try_from(self.default).is_err() {
            bail!(
                "{pos}: default value of property 'param' must be between {} and {}, got {}",
                i32::MIN,
                i32::MAX,
                self.default
            );
        }

        Ok(())
    }
}

impl fmt::Display for Prop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--param:{}", self.default)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("move.tra").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_param_prop() {
        let prop = Prop::parse(pos(), "param", " -15 ").unwrap().unwrap();
        assert_eq!(prop.default, -15);
        assert_eq!(prop.to_string(), "--param:-15");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "100").unwrap().unwrap();
        assert_eq!(prop.default, 100);
        assert_eq!(prop.to_string(), "--param:100");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "param", "0x55").unwrap().unwrap();
        assert_eq!(prop.default, 85);
        assert_eq!(prop.to_string(), "--param:85");
        prop.validate().unwrap();

        for body in ["-2147483648", "2147483647", "0b101", "1_000"] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            prop.validate().unwrap();
        }

        assert!(Prop::parse(pos(), "speed", "0").unwrap().is_none());

        for body in [
            "",
            "Label,0",
            "0,1",
            "NaN",
            "inf",
            "-inf",
            "abc",
            "0\n",
            "-1.5",
            "1.0",
            "1e2",
            "0x1p0",
            "-2147483647.5",
        ] {
            let err = Prop::parse(pos(), "param", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra:2:3:"), "{body:?}: {err}");
        }
    }

    #[test]
    fn validates_param_prop() {
        for body in [
            "-2147483649",
            "2147483648",
            "-9223372036854775808",
            "9223372036854775807",
        ] {
            let prop = Prop::parse(pos(), "param", body).unwrap().unwrap();
            let err = prop.validate().unwrap_err();
            assert!(err.to_string().starts_with("move.tra:2:3:"), "{body}: {err}");
        }
    }
}
