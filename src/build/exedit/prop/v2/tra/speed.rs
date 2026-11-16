use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    accel: bool,
    decel: Option<bool>,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "speed" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'speed' cannot contain control characters");
        }

        let parse_flag = |label: &str, arg: &str| match arg.trim() {
            "0" => Ok(false),
            "1" => Ok(true),
            arg => bail!("{pos}: {label} flag of property 'speed' must be '0' or '1', got '{arg}'"),
        };

        let mut args = body.splitn(3, ',');
        let accel = parse_flag("acceleration", args.next().unwrap())?;
        let decel = args.next().map(|arg| parse_flag("deceleration", arg)).transpose()?;

        let rest = args.next();
        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'speed' expects at most 2 arguments, got {}",
                2 + rest.split(',').count()
            );
        }

        Ok(Some(Self {
            pos,
            accel,
            decel,
            rest,
        }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--speed:{}", u8::from(self.accel))?;

        if let Some(decel) = self.decel {
            write!(f, ",{}", u8::from(decel))?;
        }

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
    fn parses_speed_prop() {
        let prop = Prop::parse(pos(), "speed", "0").unwrap().unwrap();
        assert!(!prop.accel);
        assert_eq!(prop.decel, None);
        assert_eq!(prop.to_string(), "--speed:0");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "speed", " 1 , 0 ").unwrap().unwrap();
        assert!(prop.accel);
        assert_eq!(prop.decel, Some(false));
        assert_eq!(prop.to_string(), "--speed:1,0");
        prop.validate().unwrap();

        let prop = Prop::parse(pos(), "speed", "1,1, extra , ").unwrap().unwrap();
        assert!(prop.accel);
        assert_eq!(prop.decel, Some(true));
        assert_eq!(prop.rest, Some(" extra , "));
        assert_eq!(prop.to_string(), "--speed:1,1, extra , ");
        prop.validate().unwrap();

        assert!(Prop::parse(pos(), "param", "0").unwrap().is_none());

        for body in ["", "2", "0,2", "1,", "true", "0\t,1", "0,1\n"] {
            let err = Prop::parse(pos(), "speed", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }
}
