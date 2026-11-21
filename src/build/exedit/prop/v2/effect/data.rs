use crate::build::source::Pos;
use anyhow::bail;
use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    name: &'a str,
    size: i64,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &str,
        name: &'a str,
        body: &'a str,
    ) -> anyhow::Result<Option<Self>> {
        if kind != "data" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'data' cannot contain control characters");
        }

        if body.trim().is_empty() {
            bail!("{pos}: property 'data' requires 1 argument, got 0");
        }

        let name = name.trim();
        let (size, rest) = body
            .split_once(',')
            .map_or((body.trim(), None), |(size, rest)| (size.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: property 'data' expects 1 argument, got {}",
                rest.split(',').count() + 1
            );
        }

        let Ok(size) = size.parse::<i64>() else {
            bail!("{pos}: size of property 'data' must be an integer, got '{size}'");
        };

        Ok(Some(Self { pos, name, size, rest }))
    }

    pub(in crate::build::exedit) fn validate(&self, names: &mut HashSet<Cow<'a, str>>) -> anyhow::Result<()> {
        let pos = &self.pos;

        if !(0..=16_000).contains(&self.size) {
            bail!(
                "{pos}: size of property 'data' must be between 0 and 16000, got {}",
                self.size
            );
        }

        if !names.insert(Cow::Borrowed(self.name)) {
            bail!("{pos}: duplicate property name '{}'", self.name);
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--data@{}:{}", self.name, self.size)?;

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
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        }
    }

    #[test]
    fn parses_data_prop() {
        let prop = Prop::parse(pos(), "data", "  buf  ", "  1024  ").unwrap().unwrap();
        assert_eq!(prop.name, "buf");
        assert_eq!(prop.to_string(), "--data@buf:1024");

        let extra = Prop::parse(pos(), "data", "buf", "1024, extra, args").unwrap().unwrap();
        assert_eq!(extra.to_string(), "--data@buf:1024, extra, args");

        assert!(Prop::parse(pos(), "file", "buf", "1024").unwrap().is_none());

        for body in ["", "abc", "1.5", "0x400", "1024\n"] {
            assert!(Prop::parse(pos(), "data", "buf", body).is_err(), "{body}");
        }
    }

    #[test]
    fn validates_data_prop() {
        for (name, size) in [
            ("buf", "0"),
            ("buf", "1024"),
            ("buf", "16000"),
            ("", "1024"),
            ("effect.buf", "1024"),
            ("1buf", "1024"),
        ] {
            let prop = Prop::parse(pos(), "data", name, size).unwrap().unwrap();
            prop.validate(&mut HashSet::new()).unwrap();
        }

        for (name, size) in [("buf", "-1"), ("buf", "16001")] {
            let prop = Prop::parse(pos(), "data", name, size).unwrap().unwrap();
            let err = prop.validate(&mut HashSet::new()).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{name}:{size}: {err}");
        }
    }

    #[test]
    fn rejects_duplicate_names() {
        for name in ["buf", ""] {
            let prop = Prop::parse(pos(), "data", name, "8").unwrap().unwrap();
            let mut names = HashSet::new();
            prop.validate(&mut names).unwrap();
            assert_eq!(names, HashSet::from([Cow::Borrowed(name)]));

            let err = prop.validate(&mut names).unwrap_err();
            assert_eq!(
                err.to_string(),
                format!("input.anm2:2:3: duplicate property name '{name}'")
            );

            let mut names = HashSet::from([Cow::Owned(name.to_owned())]);
            assert!(prop.validate(&mut names).is_err());
        }
    }
}
