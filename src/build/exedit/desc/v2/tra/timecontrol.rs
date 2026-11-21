use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Desc<'a> {
    pos: Pos,
    rest: &'a str,
}

#[allow(dead_code)]
impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "timecontrol" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: descriptor 'timecontrol' cannot contain control characters");
        }

        if !body.is_empty() {
            tracing::warn!(
                "{pos}: descriptor 'timecontrol' expects 0 arguments, got {}",
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

impl fmt::Display for Desc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--timecontrol")?;

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
    fn parses_timecontrol_desc() {
        let desc = Desc::parse(pos(), "timecontrol", "").unwrap().unwrap();
        assert_eq!(desc.to_string(), "--timecontrol");
        assert_eq!(desc.rest, "");
        desc.validate().unwrap();

        assert!(Desc::parse(pos(), "filter", "").unwrap().is_none());

        for body in ["1", " extra,値:more, "] {
            let desc = Desc::parse(pos(), "timecontrol", body).unwrap().unwrap();
            assert_eq!(desc.rest, body);
            assert_eq!(desc.to_string(), format!("--timecontrol:{body}"));
            desc.validate().unwrap();
        }

        for body in ["\0", "extra\n", "\t"] {
            let err = Desc::parse(pos(), "timecontrol", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra2:2:3:"), "{body:?}: {err}");
        }
    }
}
