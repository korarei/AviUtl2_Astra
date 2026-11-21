use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Desc<'a> {
    pos: Pos,
    ver: u32,
    rest: Option<&'a str>,
}

#[allow(dead_code)]
impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "require" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: descriptor 'require' cannot contain control characters");
        }

        if body.trim().is_empty() {
            bail!("{pos}: descriptor 'require' requires 1 argument, got 0");
        }

        let (ver, rest) = body
            .split_once(',')
            .map_or((body.trim(), None), |(ver, rest)| (ver.trim(), Some(rest)));

        if let Some(rest) = rest {
            tracing::warn!(
                "{pos}: descriptor 'require' expects 1 argument, got {}",
                rest.split(',').count() + 1
            );
        }

        let Ok(ver) = ver.parse::<u32>() else {
            bail!("{pos}: version of descriptor 'require' must be a 32-bit unsigned integer, got '{ver}'");
        };

        Ok(Some(Self { pos, ver, rest }))
    }

    #[expect(clippy::unused_self, clippy::unnecessary_wraps)]
    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

impl fmt::Display for Desc<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--require:{}", self.ver)?;

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
    fn parses_require_desc() {
        for (body, expected) in [
            ("  2003500  ", "--require:2003500"),
            ("0", "--require:0"),
            ("4294967295", "--require:4294967295"),
            ("2003500, extra, args", "--require:2003500, extra, args"),
        ] {
            let desc = Desc::parse(pos(), "require", body).unwrap().unwrap();
            assert_eq!(desc.to_string(), expected);
            desc.validate().unwrap();
        }

        assert!(Desc::parse(pos(), "script", "2003500").unwrap().is_none());

        for body in [
            "",
            "   ",
            ", 2003500",
            "-1",
            "4294967296",
            "abc",
            "1.5",
            "0x100",
            "2003500\n",
        ] {
            let err = Desc::parse(pos(), "require", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm2:2:3:"), "{body}: {err}");
        }
    }
}
