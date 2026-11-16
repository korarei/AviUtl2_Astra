use crate::build::lang::lua;
use crate::build::source::Pos;
use anyhow::bail;
use std::fmt;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(in crate::build::exedit) struct Prop {
    pos: Pos,
    default: i64,
}

#[allow(dead_code)]
impl Prop {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: &str) -> anyhow::Result<Option<Self>> {
        if kind != "color" {
            return Ok(None);
        }

        if body.chars().any(char::is_control) {
            bail!("{pos}: property 'color' cannot contain control characters");
        }

        let mut args = body.split(',');
        let arg = args.next().unwrap().trim();
        let count = args.count() + 1;

        if count > 1 {
            bail!("{pos}: property 'color' accepts at most 1 argument, got {count}");
        }

        let Some(default) = lua::parse_integer(arg) else {
            bail!("{pos}: default value of property 'color' must be an integer, got '{arg}'");
        };

        Ok(Some(Self { pos, default }))
    }

    pub(in crate::build::exedit) fn validate(&self) -> anyhow::Result<()> {
        let pos = &self.pos;
        let default = self.default;

        if !(0..=0xff_ffff).contains(&default) {
            bail!("{pos}: default value of property 'color' must be between 0x000000 and 0xffffff, got {default}");
        }

        Ok(())
    }
}

impl fmt::Display for Prop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "--color:0x{:06x}", self.default)
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
    fn parses_color_prop() {
        let prop = Prop::parse(pos(), "color", " 0xffffff ").unwrap().unwrap();
        assert_eq!(prop.to_string(), "--color:0xffffff");

        let zero = Prop::parse(pos(), "color", "0").unwrap().unwrap();
        assert_eq!(zero.to_string(), "--color:0x000000");

        let bin = Prop::parse(pos(), "color", "+0b1010").unwrap().unwrap();
        assert_eq!(bin.to_string(), "--color:0x00000a");

        for (kind, body) in [("color0", "0"), ("file", ""), ("track0", "Label,0,100,0")] {
            assert!(Prop::parse(pos(), kind, body).unwrap().is_none(), "{kind}: {body}");
        }

        for body in ["", "nil", "abc", "0x1.0", "0,extra"] {
            let err = Prop::parse(pos(), "color", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn validates_color_prop() {
        for body in ["0", "0xffffff", "16777215"] {
            let prop = Prop::parse(pos(), "color", body).unwrap().unwrap();
            assert!(prop.validate().is_ok(), "{body}");
        }

        for body in ["-1", "16777216"] {
            let err = Prop::parse(pos(), "color", body)
                .unwrap()
                .unwrap()
                .validate()
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }
}
