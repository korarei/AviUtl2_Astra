pub(in crate::build::exedit) mod check;
pub(in crate::build::exedit) mod color;
pub(in crate::build::exedit) mod dialog;
pub(in crate::build::exedit) mod file;
pub(in crate::build::exedit) mod param;
pub(in crate::build::exedit) mod track;

use crate::build::exedit::element::State;
use crate::build::exedit::l10n;
use crate::build::source::Pos;

pub(in crate::build::exedit) enum Prop<'a> {
    Track(track::Prop<'a>),
    Check(check::Prop<'a>),
    Color(color::Prop),
    File(file::Prop),
    Param(param::Prop<'a>),
    Dialog(dialog::Prop<'a>),
}

impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &'a str,
        body: Option<&'a str>,
    ) -> anyhow::Result<Option<Self>> {
        let Some(body) = body else {
            return Ok(None);
        };

        Ok(Some(match kind {
            "track0" | "track1" | "track2" | "track3" => Self::Track(track::Prop::parse(pos, kind, body)?.unwrap()),
            "check0" => Self::Check(check::Prop::parse(pos, kind, body)?.unwrap()),
            "color" => Self::Color(color::Prop::parse(pos, kind, body)?.unwrap()),
            "file" => Self::File(file::Prop::parse(pos, kind, body)?.unwrap()),
            "param" => Self::Param(param::Prop::parse(pos, kind, body)?.unwrap()),
            "dialog" => Self::Dialog(dialog::Prop::parse(pos, kind, body)?.unwrap()),
            _ => return Ok(None),
        }))
    }

    pub(in crate::build::exedit) fn process(
        self,
        state: &mut State<'a>,
        l10n: &mut l10n::Collector,
    ) -> anyhow::Result<String> {
        match self {
            Self::Track(prop) => {
                prop.validate(&mut state.keys)?;
                l10n.push(&[prop.name()], &[prop.name()]);
                Ok(prop.to_string())
            }
            Self::Check(prop) => {
                prop.validate(&mut state.keys)?;
                l10n.push(&[prop.name()], &[prop.name()]);
                Ok(prop.to_string())
            }
            Self::Color(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
            Self::File(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
            Self::Param(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
            Self::Dialog(prop) => {
                prop.validate()?;
                Ok(prop.to_string())
            }
        }
    }
}

pub(in crate::build::exedit) fn is_kind(kind: &str, has_body: bool) -> bool {
    has_body
        && matches!(
            kind,
            "track0" | "track1" | "track2" | "track3" | "check0" | "color" | "file" | "param" | "dialog"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::exedit::element::State;
    use crate::build::exedit::l10n;
    use crate::build::source::Pos;
    use std::collections::HashSet;
    use std::path::Path;

    #[test]
    fn processes_properties() {
        let mut state = State::default();
        for (kind, body, expected, names) in [
            ("track0", "Value,0,10,5", "--track0:Value,0,10,5", &["Value"][..]),
            ("check0", "Enabled,1", "--check0:Enabled,1", &["Enabled"][..]),
            ("color", "255", "--color:0x0000ff", &[][..]),
            ("file", "", "--file:", &[][..]),
            ("param", "a=1", "--param:a=1;", &[][..]),
            ("dialog", "A,a=1", "--dialog:A,a=1;", &[][..]),
        ] {
            let pos = Pos {
                file: Path::new("input.anm").into(),
                line: 2,
                col: 3,
            };
            let mut l10n = l10n::Format::Full.collector();
            let src = Prop::parse(pos, kind, Some(body))
                .and_then(|prop| prop.map(|prop| prop.process(&mut state, &mut l10n)).transpose())
                .unwrap()
                .unwrap();
            assert_eq!(src, expected);

            let mut expected = l10n::Format::Full.collector();
            expected.push(names, names);
            assert_eq!(l10n.render("input"), expected.render("input"));
        }
        assert_eq!(state.keys, HashSet::from(["track0", "check0"]));

        let pos = || Pos {
            file: Path::new("input.anm").into(),
            line: 2,
            col: 3,
        };
        assert!(
            Prop::parse(pos(), "track0", Some("Other,0,10,5"))
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut state, &mut l10n::Format::None.collector()))
                    .transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "file", Some("extra"))
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut state, &mut l10n::Format::None.collector()))
                    .transpose())
                .is_err()
        );
        assert!(
            Prop::parse(pos(), "unknown", None)
                .and_then(|prop| prop
                    .map(|prop| prop.process(&mut state, &mut l10n::Format::None.collector()))
                    .transpose())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn handles_input_boundaries() {
        for body in ["", ",", ",,", "_", "0", "NaN", "\u{3042},", "Label,0,1,0", "\0", "\r\n"] {
            assert!(
                std::panic::catch_unwind(|| {
                    let pos = || Pos {
                        file: Path::new("input.anm").into(),
                        line: 1,
                        col: 1,
                    };

                    let _ = check::Prop::parse(pos(), "check0", body);
                    let _ = color::Prop::parse(pos(), "color", body);
                    let _ = dialog::Prop::parse(pos(), "dialog", body);
                    let _ = file::Prop::parse(pos(), "file", body);
                    let _ = param::Prop::parse(pos(), "param", body);
                    let _ = track::Prop::parse(pos(), "track0", body);
                })
                .is_ok(),
                "{body:?}"
            );
        }
    }
}
