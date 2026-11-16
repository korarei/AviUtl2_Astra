pub(in crate::build::exedit) mod check;
pub(in crate::build::exedit) mod param;
pub(in crate::build::exedit) mod select;
pub(in crate::build::exedit) mod speed;
pub(in crate::build::exedit) mod twopoint;

use crate::build::exedit::l10n;
use crate::build::source::Pos;

pub(in crate::build::exedit) enum Prop<'a> {
    Check(check::Prop<'a>),
    Select(select::Prop<'a>),
    Param(param::Prop<'a>),
    Speed(speed::Prop<'a>),
    Twopoint(twopoint::Prop<'a>),
}

impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: Option<&'a str>) -> anyhow::Result<Option<Self>> {
        match (kind, body) {
            ("param", Some(body)) => {
                if let Some(prop) = check::Prop::parse(pos.clone(), kind, body)? {
                    return Ok(Some(Self::Check(prop)));
                }

                if let Some(prop) = select::Prop::parse(pos.clone(), kind, body)? {
                    return Ok(Some(Self::Select(prop)));
                }

                Ok(Some(Self::Param(param::Prop::parse(pos, kind, body)?.unwrap())))
            }
            ("speed", Some(body)) => Ok(Some(Self::Speed(speed::Prop::parse(pos, kind, body)?.unwrap()))),
            ("twopoint", body) => Ok(Some(Self::Twopoint(
                twopoint::Prop::parse(pos, kind, body.unwrap_or_default())?.unwrap(),
            ))),
            _ => Ok(None),
        }
    }

    pub(in crate::build::exedit) fn process(self, l10n: &mut l10n::Collector) -> anyhow::Result<String> {
        match self {
            Self::Check(prop) => {
                prop.validate()?;
                l10n.push(&[prop.name()], &[prop.label()]);
                Ok(prop.to_string())
            }
            Self::Select(prop) => {
                prop.validate()?;
                l10n.push(&[prop.name()], &prop.labels());
                Ok(prop.to_string())
            }
            Self::Param(prop) => {
                prop.validate()?;
                l10n.push(prop.name().as_slice(), prop.label().as_slice());
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
    kind == "twopoint" || (has_body && matches!(kind, "param" | "speed"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::exedit::l10n;
    use crate::build::source::Pos;
    use std::path::Path;

    #[test]
    fn processes_param_variants() {
        let pos = || Pos {
            file: Path::new("move.tra2").into(),
            line: 2,
            col: 3,
        };
        for (kind, body, expected, names, labels) in [
            ("param", "5", "--param:5", &[][..], &[][..]),
            (
                "param",
                "Group::Value,5",
                "--param:Group::Value,5",
                &["Group::Value"][..],
                &["Group::Value"][..],
            ),
            (
                "param",
                "Group::Flag/check,1",
                "--param:Group::Flag/check,1",
                &["Group::Flag"][..],
                &["Group::Flag"][..],
            ),
            (
                "param",
                "Group::Mode/select/Group::A=0/Group::A=1,1,extra",
                "--param:Group::Mode/select/Group::A=0/Group::A=1,1,extra",
                &["Group::Mode"][..],
                &["Group::Mode", "Group::A", "Group::A"][..],
            ),
            ("speed", "1,0", "--speed:1,0", &[][..], &[][..]),
            ("twopoint", "extra", "--twopoint:extra", &[][..], &[][..]),
        ] {
            let mut l10n = l10n::Format::Full.collector();
            let src = Prop::parse(pos(), kind, Some(body))
                .and_then(|prop| prop.map(|prop| prop.process(&mut l10n)).transpose())
                .unwrap()
                .unwrap();
            assert_eq!(src, expected);

            let mut expected = l10n::Format::Full.collector();
            expected.push(names, labels);
            assert_eq!(l10n.render("input"), expected.render("input"));
        }

        assert!(
            Prop::parse(pos(), "param", Some("Mode/select/A=0/B=0,0"))
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut l10n::Format::None.collector()))
                    .transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "param", Some("Mode/check,2"))
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut l10n::Format::None.collector()))
                    .transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "unknown", None)
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut l10n::Format::None.collector()))
                    .transpose())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn handles_input_boundaries() {
        for body in [
            "",
            ",",
            ",,",
            "0",
            "1,0",
            "NaN",
            "\u{3042},0",
            "Flag/check,1",
            "Mode/select/A=0,0",
            "Label,0,extra",
            "\0",
            "\r\n",
        ] {
            assert!(
                std::panic::catch_unwind(|| {
                    let pos = || Pos {
                        file: Path::new("move.tra2").into(),
                        line: 1,
                        col: 1,
                    };

                    let _ = check::Prop::parse(pos(), "param", body);
                    let _ = param::Prop::parse(pos(), "param", body);
                    let _ = select::Prop::parse(pos(), "param", body);
                    let _ = speed::Prop::parse(pos(), "speed", body);
                    let _ = twopoint::Prop::parse(pos(), "twopoint", body);
                })
                .is_ok(),
                "{body:?}"
            );
        }
    }
}
