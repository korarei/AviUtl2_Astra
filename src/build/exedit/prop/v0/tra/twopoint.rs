use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop {
    pos: Pos,
}

#[allow(dead_code)]
impl Prop {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &str) -> anyhow::Result<Option<Self>> {
        if kind != "twopoint" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'twopoint' cannot contain control characters");
        }

        if !body.trim().is_empty() {
            bail!(
                "{pos}: property 'twopoint' expects 0 arguments, got {}",
                body.split(',').count()
            );
        }

        Ok(Some(Self { pos }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Prop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("--twopoint")
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
    fn parses_twopoint_prop() {
        let prop = Prop::parse(pos(), "twopoint", "  ").unwrap().unwrap();
        assert_eq!(prop.to_string(), "--twopoint");
        prop.validate().unwrap();

        assert!(Prop::parse(pos(), "speed", "0,0").unwrap().is_none());

        for body in ["1", "extra", "a,b", "\0", "\n"] {
            let err = Prop::parse(pos(), "twopoint", body).unwrap_err();
            assert!(err.to_string().starts_with("move.tra:2:3:"), "{body:?}: {err}");
        }
    }
}
