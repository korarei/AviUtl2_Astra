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

        let Some(default) = parse_color(arg) else {
            bail!("{pos}: default value of property 'color' must be a hexadecimal integer, got '{arg}'");
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

fn parse_color(src: &str) -> Option<i64> {
    let invalid = match src.as_bytes() {
        [b'0', b'x', ..] => b'A'..=b'F',
        [b'0', b'X', ..] => b'a'..=b'f',
        _ => return None,
    };
    let digits = &src[2..];

    if digits
        .bytes()
        .any(|byte| !byte.is_ascii_hexdigit() || invalid.contains(&byte))
    {
        return None;
    }

    i64::from_str_radix(digits, 16).ok()
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

        let upper = Prop::parse(pos(), "color", "0XFFFFFF").unwrap().unwrap();
        assert_eq!(upper.to_string(), "--color:0xffffff");

        let short = Prop::parse(pos(), "color", "0xff").unwrap().unwrap();
        assert_eq!(short.to_string(), "--color:0x0000ff");

        let zero = Prop::parse(pos(), "color", "0x0").unwrap().unwrap();
        assert_eq!(zero.to_string(), "--color:0x000000");

        for (kind, body) in [("color0", "0x0"), ("file", ""), ("track0", "Label,0,100,0")] {
            assert!(Prop::parse(pos(), kind, body).unwrap().is_none(), "{kind}: {body}");
        }

        for body in [
            "",
            "nil",
            "abc",
            "0",
            "16777215",
            "+0xffffff",
            "-0xffffff",
            "0b1010",
            "0x",
            "0X",
            "0xFF0000",
            "0Xff0000",
            "0xff_00_00",
            "0x1.0",
            "0x0,extra",
        ] {
            let err = Prop::parse(pos(), "color", body).unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }

    #[test]
    fn validates_color_prop() {
        for body in ["0x0", "0xff", "0XFF", "0xffffff", "0XFFFFFF"] {
            let prop = Prop::parse(pos(), "color", body).unwrap().unwrap();
            assert!(prop.validate().is_ok(), "{body}");
        }

        for body in ["0x1000000", "0X1000000"] {
            let err = Prop::parse(pos(), "color", body)
                .unwrap()
                .unwrap()
                .validate()
                .unwrap_err();
            assert!(err.to_string().starts_with("input.anm:2:3:"), "{body}: {err}");
        }
    }
}
