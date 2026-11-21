use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    rest: &'a str,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "twopoint" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'twopoint' cannot contain control characters");
        }

        if !body.is_empty() {
            tracing::warn!(
                "{pos}: property 'twopoint' expects 0 arguments, got {}",
                body.split(',').count()
            );
        }

        Ok(Some(Self { pos, rest: body }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--twopoint")?;

        if !self.rest.is_empty() {
            write!(f, ":{}", self.rest)?;
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
    fn parses_twopoint_prop() {
        let prop = Prop::parse(pos(), "twopoint", "").unwrap().unwrap();
        assert_eq!(prop.to_string(), "--twopoint");
        assert_eq!(prop.rest, "");
        prop.validate().unwrap();

        assert!(Prop::parse(pos(), "speed", "0,0").unwrap().is_none());

        for body in ["1", " extra,値:more, "] {
            let prop = Prop::parse(pos(), "twopoint", body).unwrap().unwrap();
            assert_eq!(prop.rest, body);
            assert_eq!(prop.to_string(), format!("--twopoint:{body}"));
            prop.validate().unwrap();
        }

        for body in ["\0", "extra\n", "\t"] {
            let err = Prop::parse(pos(), "twopoint", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }
}
