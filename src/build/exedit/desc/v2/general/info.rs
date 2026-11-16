use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Desc<'a> {
    pos: Pos,
    text: &'a str,
}

#[allow(dead_code)]
impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "information" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: descriptor 'information' cannot contain control characters");
        }

        let text = body.trim();
        if text.is_empty() {
            bail!("{pos}: descriptor 'information' cannot be empty");
        }

        Ok(Some(Self { pos, text }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Desc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--information:{}", self.text)
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
    fn parses_info_desc() {
        let desc = Desc::parse(pos(), "information", "  Effect v1.0, by Author  ")
            .unwrap()
            .unwrap();
        assert_eq!(desc.to_string(), "--information:Effect v1.0, by Author");

        assert!(Desc::parse(pos(), "label", "Effect v1.0").unwrap().is_none());

        for body in ["", "   ", "Effect\n"] {
            assert!(Desc::parse(pos(), "information", body).is_err(), "{body}");
        }
    }

    #[test]
    fn validates_info_desc() {
        for body in ["Effect v1.0", "a, b, c"] {
            let desc = Desc::parse(pos(), "information", body).unwrap().unwrap();
            desc.validate().unwrap();
        }
    }
}
