use crate::build::source::Pos;
use anyhow::bail;
use std::collections::HashSet;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop<'a> {
    pos: Pos,
    name: &'a str,
    default: bool,
}

#[allow(dead_code)]
impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &'a str, body: &'a str) -> anyhow::Result<Option<Self>> {
        if kind != "check0" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property '{kind}' cannot contain control characters");
        }

        let mut parts = body.split(',');
        let name = parts.next().unwrap().trim();
        let Some(arg) = parts.next() else {
            bail!("{pos}: property '{kind}' requires at least 1 argument, got 0");
        };
        let count = parts.count() + 1;

        if count > 1 {
            bail!("{pos}: property '{kind}' accepts at most 1 argument, got {count}");
        }

        let arg = arg.trim();
        let default = match arg {
            "1" => true,
            "0" => false,
            _ => bail!("{pos}: default value of property '{kind}' must be '0' or '1', got '{arg}'"),
        };

        Ok(Some(Self { pos, name, default }))
    }

    pub(in crate::build::exedit) fn name(&self) -> &'a str {
        self.name
    }

    pub(in crate::build::exedit) fn validate(&self, keys: &mut HashSet<&'a str>) -> anyhow::Result<()> {
        if !keys.insert("check0") {
            bail!("{}: duplicate property key 'check0'", self.pos);
        }

        Ok(())
    }
}

impl fmt::Display for Prop<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--check0:{},{}", self.name, u8::from(self.default))
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
    fn parses_check_prop() {
        let prop = Prop::parse(pos(), "check0", "  Enable  , 1 ").unwrap().unwrap();
        assert_eq!(prop.name(), "Enable");
        assert_eq!(prop.to_string(), "--check0:Enable,1");
        prop.validate(&mut HashSet::new()).unwrap();

        let off = Prop::parse(pos(), "check0", "Enable,0").unwrap().unwrap();
        assert_eq!(off.to_string(), "--check0:Enable,0");
        off.validate(&mut HashSet::new()).unwrap();

        for (kind, body) in [
            ("check1", "Enable,0"),
            ("check@flag", "Enable,true"),
            ("track0", "Label,0,100,0"),
        ] {
            assert!(Prop::parse(pos(), kind, body).unwrap().is_none(), "{kind}: {body}");
        }

        for body in ["Enable", "Enable,", "Enable,true", "Enable,2", "Enable,0,extra"] {
            let err = Prop::parse(pos(), "check0", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn rejects_duplicate_keys() {
        let mut keys = HashSet::from(["track0"]);
        let prop = Prop::parse(pos(), "check0", "Enable,1").unwrap().unwrap();
        prop.validate(&mut keys).unwrap();
        assert_eq!(keys, HashSet::from(["track0", "check0"]));

        let duplicate = Prop::parse(pos(), "check0", "Other,0").unwrap().unwrap();
        let err = duplicate.validate(&mut keys).unwrap_err();
        assert_eq!(err.to_string(), "input.anm:2:3: duplicate property key 'check0'");
    }
}
