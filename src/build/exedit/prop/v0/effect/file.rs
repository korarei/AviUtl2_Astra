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
        if kind != "file" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'file' cannot contain control characters");
        }

        let arg = body.trim();
        if !arg.is_empty() {
            bail!("{pos}: argument of property 'file' must be empty, got '{arg}'");
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
        f.write_str("--file:")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn pos() -> Pos {
        Pos {
            file: Path::new("input.anm").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_file_prop() {
        for body in ["", "  "] {
            let prop = Prop::parse(pos(), "file", body).unwrap().unwrap();
            assert_eq!(prop.to_string(), "--file:");
            prop.validate().unwrap();
        }

        for (kind, body) in [("file0", ""), ("color", "0"), ("check0", "Enable,0")] {
            assert!(Prop::parse(pos(), kind, body).unwrap().is_none(), "{kind}: {body}");
        }

        for body in ["path", "a,b"] {
            let err = Prop::parse(pos(), "file", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }
}
