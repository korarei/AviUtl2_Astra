use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Desc<'a> {
    pos: Pos,
    name: &'a str,
}

#[allow(dead_code)]
impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "label" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: descriptor 'label' cannot contain control characters");
        }

        let name = body.trim();
        if name.is_empty() {
            bail!("{pos}: descriptor 'label' cannot be empty");
        }

        Ok(Some(Self { pos, name }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Desc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--label:{}", self.name)
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
    fn parses_label_desc() {
        let desc = Desc::parse(pos(), "label", "  Blur  ").unwrap().unwrap();
        assert_eq!(desc.to_string(), "--label:Blur");

        let scoped = Desc::parse(pos(), "label", "Group::Sub::Blur").unwrap().unwrap();
        assert_eq!(scoped.to_string(), "--label:Group::Sub::Blur");

        let comma = Desc::parse(pos(), "label", "  Blur, Effects  ").unwrap().unwrap();
        assert_eq!(comma.to_string(), "--label:Blur, Effects");

        assert!(Desc::parse(pos(), "information", "Blur").unwrap().is_none());

        for body in ["", "   ", "Blur\n"] {
            assert!(Desc::parse(pos(), "label", body).is_err(), "{body}");
        }
    }

    #[test]
    fn validates_label_desc() {
        for body in ["Blur", "Group::Blur"] {
            let desc = Desc::parse(pos(), "label", body).unwrap().unwrap();
            desc.validate().unwrap();
        }
    }
}
