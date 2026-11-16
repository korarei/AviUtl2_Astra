pub(in crate::build::exedit) mod param;
pub(in crate::build::exedit) mod speed;
pub(in crate::build::exedit) mod twopoint;

use crate::build::source::Pos;

pub(in crate::build::exedit) enum Prop {
    Param(param::Prop),
    Speed(speed::Prop),
    Twopoint(twopoint::Prop),
}

impl Prop {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: Option<&str>) -> anyhow::Result<Option<Self>> {
        Ok(Some(match (kind, body) {
            ("param", Some(body)) => Self::Param(param::Prop::parse(pos, kind, body)?.unwrap()),
            ("speed", Some(body)) => Self::Speed(speed::Prop::parse(pos, kind, body)?.unwrap()),
            ("twopoint", None) => Self::Twopoint(twopoint::Prop::parse(pos, kind, "")?.unwrap()),
            _ => return Ok(None),
        }))
    }

    pub(in crate::build::exedit) fn process(self) -> anyhow::Result<String> {
        match self {
            Self::Param(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
            Self::Speed(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
            Self::Twopoint(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
        }
    }
}

pub(in crate::build::exedit) fn is_kind(kind: &str, has_body: bool) -> bool {
    matches!((kind, has_body), ("param" | "speed", true) | ("twopoint", false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::source::Pos;
    use std::path::Path;

    #[test]
    fn processes_properties() {
        let pos = || Pos {
            file: Path::new("move.tra").into(),
            line: 2,
            col: 3,
        };
        for (kind, body, expected) in [
            ("param", Some("5"), "--param:5"),
            ("speed", Some("1,0"), "--speed:1,0"),
            ("twopoint", None, "--twopoint"),
        ] {
            let src = Prop::parse(pos(), kind, body)
                .and_then(|prop| prop.map(Prop::process).transpose())
                .unwrap()
                .unwrap();
            assert_eq!(src, expected);
        }
        assert!(
            Prop::parse(pos(), "param", Some("1.5"))
                .and_then(|prop| prop.map(Prop::process).transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "param", Some("2147483648"))
                .and_then(|prop| prop.map(Prop::process).transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "unknown", None)
                .and_then(|prop| prop.map(Prop::process).transpose())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn handles_input_boundaries() {
        for body in ["", ",", ",,", "0", "1,0", "NaN", "\u{3042},0", "Label,0", "\0", "\r\n"] {
            assert!(
                std::panic::catch_unwind(|| {
                    let pos = || Pos {
                        file: Path::new("move.tra").into(),
                        line: 1,
                        col: 1,
                    };

                    let _ = param::Prop::parse(pos(), "param", body);
                    let _ = speed::Prop::parse(pos(), "speed", body);
                    let _ = twopoint::Prop::parse(pos(), "twopoint", body);
                })
                .is_ok(),
                "{body:?}"
            );
        }
    }
}
