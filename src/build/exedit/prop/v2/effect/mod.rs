pub(in crate::build::exedit) mod check;
pub(in crate::build::exedit) mod checksection;
pub(in crate::build::exedit) mod color;
pub(in crate::build::exedit) mod data;
pub(in crate::build::exedit) mod dialog;
pub(in crate::build::exedit) mod figure;
pub(in crate::build::exedit) mod file;
pub(in crate::build::exedit) mod folder;
pub(in crate::build::exedit) mod font;
pub(in crate::build::exedit) mod group;
pub(in crate::build::exedit) mod hide;
pub(in crate::build::exedit) mod param;
pub(in crate::build::exedit) mod select;
pub(in crate::build::exedit) mod separator;
pub(in crate::build::exedit) mod string;
pub(in crate::build::exedit) mod text;
pub(in crate::build::exedit) mod track;
pub(in crate::build::exedit) mod trackgroup;
pub(in crate::build::exedit) mod value;

use crate::build::exedit::element::{Binding, State};
use crate::build::exedit::l10n;
use crate::build::source::Pos;
use anyhow::bail;

pub(in crate::build::exedit) enum Prop<'a> {
    Track(track::Prop<'a>),
    Check(check::Prop<'a>),
    Checksection(checksection::Prop<'a>),
    Folder(folder::Prop<'a>),
    Font(font::Prop<'a>),
    Figure(figure::Prop<'a>),
    String(string::Prop<'a>),
    Text(text::Prop<'a>),
    Value(value::Prop<'a>),
    Select(select::Prop<'a>),
    Color(color::Prop<'a>),
    File(file::Prop<'a>),
    Group(group::Prop<'a>),
    Separator(separator::Prop<'a>),
    Hide(hide::Prop<'a>),
    Trackgroup(trackgroup::Prop<'a>),
    Data(data::Prop<'a>),
    Param(param::Prop<'a>),
    Dialog(dialog::Prop<'a>),
}

impl<'a> Prop<'a> {
    pub(in crate::build::exedit) fn parse(
        pos: Pos,
        kind: &'a str,
        var: Option<&'a str>,
        body: Option<&'a str>,
    ) -> anyhow::Result<Option<Self>> {
        Ok(Some(match (kind, var, body) {
            (kind, var, Some(body)) if kind.starts_with("track") && kind != "trackgroup" => Self::Track(
                track::Prop::parse(pos.clone(), kind, var, body)?
                    .ok_or_else(|| anyhow::anyhow!("{pos}: invalid declaration of property '{kind}'"))?,
            ),
            (kind, var, Some(body)) if kind.starts_with("check") && kind != "checksection" => Self::Check(
                check::Prop::parse(pos.clone(), kind, var, body)?
                    .ok_or_else(|| anyhow::anyhow!("{pos}: invalid declaration of property '{kind}'"))?,
            ),
            ("checksection", Some(var), Some(body)) => {
                Self::Checksection(checksection::Prop::parse(pos, kind, var, body)?.unwrap())
            }
            ("folder", Some(var), Some(body)) => Self::Folder(folder::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("font", Some(var), Some(body)) => Self::Font(font::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("figure", Some(var), Some(body)) => Self::Figure(figure::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("string", Some(var), Some(body)) => Self::String(string::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("text", Some(var), Some(body)) => Self::Text(text::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("value", Some(var), Some(body)) => Self::Value(value::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("select", Some(var), Some(body)) => Self::Select(select::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("color", var, Some(body)) => Self::Color(color::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("file", var, Some(body)) => Self::File(file::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("group", var, body) if body.is_some() || var.is_none() => {
                Self::Group(group::Prop::parse(pos, kind, var, body)?.unwrap())
            }
            ("separator", var, Some(body)) => Self::Separator(separator::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("hide", Some(var), body) => Self::Hide(hide::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("trackgroup", Some(var), Some(body)) => {
                Self::Trackgroup(trackgroup::Prop::parse(pos, kind, var, body)?.unwrap())
            }
            ("data", Some(var), Some(body)) => Self::Data(data::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("param", var, Some(body)) => Self::Param(param::Prop::parse(pos, kind, var, body)?.unwrap()),
            ("dialog", var, Some(body)) => Self::Dialog(dialog::Prop::parse(pos, kind, var, body)?.unwrap()),
            _ if body.is_some() && allows_body(kind) => bail!("{pos}: invalid declaration of property '{kind}'"),
            _ => return Ok(None),
        }))
    }

    pub(in crate::build::exedit) fn can_bind(&self) -> bool {
        match self {
            Self::Track(prop) => matches!(prop.id(), track::Id::Var(_)),
            Self::Check(prop) => matches!(prop.id(), check::Id::Var(_)),
            Self::Checksection(_)
            | Self::Folder(_)
            | Self::Font(_)
            | Self::Figure(_)
            | Self::String(_)
            | Self::Text(_)
            | Self::Value(_)
            | Self::Select(_) => true,
            Self::Color(prop) => prop.can_bind(),
            Self::File(prop) => prop.can_bind(),
            Self::Group(_)
            | Self::Separator(_)
            | Self::Hide(_)
            | Self::Trackgroup(_)
            | Self::Data(_)
            | Self::Param(_)
            | Self::Dialog(_) => false,
        }
    }

    fn bind(&mut self, Binding { var, val }: Binding<'a, '_>) -> anyhow::Result<()> {
        if !self.can_bind() {
            bail!("property cannot bind to a variable");
        }

        match self {
            Self::Track(prop) => prop.bind(var, val),
            Self::Check(prop) => prop.bind(var, val),
            Self::Checksection(prop) => prop.bind(var, val),
            Self::Folder(prop) => prop.bind(var, val),
            Self::Font(prop) => prop.bind(var, val),
            Self::Figure(prop) => prop.bind(var, val),
            Self::String(prop) => prop.bind(var, val),
            Self::Text(prop) => prop.bind(var, val),
            Self::Value(prop) => prop.bind(var, val),
            Self::Select(prop) => prop.bind(var, val),
            Self::Color(prop) => prop.bind(var, val),
            Self::File(prop) => prop.bind(var, val),
            Self::Group(_)
            | Self::Separator(_)
            | Self::Hide(_)
            | Self::Trackgroup(_)
            | Self::Data(_)
            | Self::Param(_)
            | Self::Dialog(_) => unreachable!(),
        }
    }

    pub(in crate::build::exedit) fn process(
        mut self,
        binding: Option<Binding<'a, '_>>,
        state: &mut State<'a>,
        l10n: &mut l10n::Collector,
    ) -> anyhow::Result<String> {
        if let Some(binding) = binding {
            self.bind(binding)?;
        }

        macro_rules! finish_keyed {
            ($prop:expr) => {{
                $prop.validate(&mut state.keys, &mut state.vars, &mut state.names)?;
                l10n.push(&[$prop.name()], &[$prop.label()]);
                Ok($prop.to_string())
            }};
        }

        macro_rules! finish_named {
            ($prop:expr) => {{
                $prop.validate(&mut state.vars, &mut state.names)?;
                l10n.push(&[$prop.name()], &[$prop.label()]);
                Ok($prop.to_string())
            }};
        }

        match self {
            Self::Track(prop) => finish_keyed!(prop),
            Self::Check(prop) => finish_keyed!(prop),
            Self::Checksection(prop) => finish_named!(prop),
            Self::Folder(prop) => finish_named!(prop),
            Self::Font(prop) => finish_named!(prop),
            Self::Figure(prop) => finish_named!(prop),
            Self::String(prop) => finish_named!(prop),
            Self::Text(prop) => finish_named!(prop),
            Self::Value(prop) => finish_named!(prop),
            Self::Color(prop) => finish_named!(prop),
            Self::File(prop) => finish_named!(prop),
            Self::Select(prop) => {
                prop.validate(&mut state.vars, &mut state.names)?;
                l10n.push(&[prop.name()], &prop.labels());
                Ok(prop.to_string())
            }
            Self::Group(prop) => {
                prop.validate(&mut state.vars, &mut state.names)?;
                l10n.push(prop.name().as_slice(), prop.label().as_slice());
                Ok(prop.to_string())
            }
            Self::Separator(prop) => {
                prop.validate(&mut state.vars)?;
                l10n.push(&[prop.name()], &[prop.label()]);
                Ok(prop.to_string())
            }
            Self::Hide(prop) => {
                prop.validate(&state.vars)?;
                Ok(prop.to_string())
            }
            Self::Trackgroup(prop) => {
                prop.validate(&state.vars, &mut state.names)?;
                Ok(prop.to_string())
            }
            Self::Data(prop) => {
                prop.validate(&mut state.names)?;
                Ok(prop.to_string())
            }
            Self::Param(prop) => {
                prop.validate(&mut state.vars, &mut state.names)?;
                l10n.push(&prop.names(), &prop.labels());
                Ok(prop.to_string())
            }
            Self::Dialog(prop) => {
                prop.validate(&mut state.vars, &mut state.names)?;
                l10n.push(&prop.names(), &prop.labels());
                Ok(prop.to_string())
            }
        }
    }
}

fn allows_body(kind: &str) -> bool {
    kind.starts_with("track")
        || kind.starts_with("check")
        || matches!(
            kind,
            "color"
                | "file"
                | "folder"
                | "font"
                | "figure"
                | "select"
                | "text"
                | "string"
                | "value"
                | "data"
                | "hide"
                | "group"
                | "separator"
                | "dialog"
                | "param"
        )
}

pub(in crate::build::exedit) fn is_kind(kind: &str, has_var: bool, has_body: bool) -> bool {
    match (has_body, kind, has_var) {
        (true, kind, _) if allows_body(kind) => true,
        (false, "hide", true) | (false, "group", false) => true,
        _ => false,
    }
}

pub(in crate::build::exedit) fn is_bindable(kind: &str, has_var: bool, has_body: bool) -> bool {
    has_var
        && has_body
        && matches!(
            kind,
            "track"
                | "check"
                | "checksection"
                | "folder"
                | "font"
                | "figure"
                | "string"
                | "text"
                | "value"
                | "select"
                | "color"
                | "file"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::exedit::element::State;
    use crate::build::exedit::l10n;
    use crate::build::source::Pos;
    use std::borrow::Cow;
    use std::collections::HashSet;
    use std::path::Path;

    type PropertyCase = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        &'static str,
        &'static [&'static str],
        &'static [&'static str],
    );

    const PROPERTY_CASES: &[PropertyCase] = &[
        (
            "track",
            Some("val"),
            Some("Group::Scale,0,10,5"),
            "--track@val:Group::Scale,0,10,5",
            &["Group::Scale"],
            &["Scale"],
        ),
        (
            "track0",
            None,
            Some("Scale,0,10,5"),
            "--track0:Scale,0,10,5",
            &["Scale"],
            &["Scale"],
        ),
        (
            "check",
            Some("val"),
            Some("Flag,true"),
            "--check@val:Flag,true",
            &["Flag"],
            &["Flag"],
        ),
        ("check0", None, Some("Flag,1"), "--check0:Flag,1", &["Flag"], &["Flag"]),
        (
            "checksection",
            Some("val"),
            Some("Section,true"),
            "--checksection@val:Section,true",
            &["Section"],
            &["Section"],
        ),
        (
            "color",
            Some("val"),
            Some("Color,255"),
            "--color@val:Color,0x0000ff",
            &["Color"],
            &["Color"],
        ),
        ("color", None, Some("255"), "--color:0x0000ff", &["色"], &["色"]),
        (
            "file",
            Some("val"),
            Some("Path"),
            "--file@val:Path",
            &["Path"],
            &["Path"],
        ),
        ("file", None, Some(""), "--file:", &["ファイル"], &["ファイル"]),
        (
            "folder",
            Some("val"),
            Some("Path"),
            "--folder@val:Path",
            &["Path"],
            &["Path"],
        ),
        (
            "font",
            Some("val"),
            Some("Font,Arial"),
            "--font@val:Font,Arial",
            &["Font"],
            &["Font"],
        ),
        (
            "figure",
            Some("val"),
            Some("Figure,Triangle"),
            "--figure@val:Figure,Triangle",
            &["Figure"],
            &["Figure"],
        ),
        (
            "select",
            Some("val"),
            Some("Group::Mode=0,Group::A=0,Other::A=1"),
            "--select@val:Group::Mode=0,Group::A=0,Other::A=1",
            &["Group::Mode"],
            &["Mode", "Group::A", "Other::A"],
        ),
        (
            "string",
            Some("val"),
            Some("Text,abc"),
            "--string@val:Text,abc",
            &["Text"],
            &["Text"],
        ),
        (
            "text",
            Some("val"),
            Some("Text,abc"),
            "--text@val:Text,abc",
            &["Text"],
            &["Text"],
        ),
        (
            "value",
            Some("val"),
            Some("Value,{1,2}"),
            "--value@val:Value,{1,2}",
            &["Value"],
            &["Value"],
        ),
        (
            "group",
            None,
            Some("Group,true"),
            "--group:Group,true",
            &["Group"],
            &["Group"],
        ),
        ("group", None, None, "--group", &[], &[]),
        (
            "separator",
            None,
            Some("Break"),
            "--separator:Break",
            &["Break"],
            &["Break"],
        ),
        (
            "hide",
            Some("target"),
            Some("cond==0"),
            "--hide@target:cond==0",
            &[],
            &[],
        ),
        (
            "trackgroup",
            Some("left,right"),
            Some("Coords"),
            "--trackgroup@left,right:Coords",
            &[],
            &[],
        ),
        ("data", Some("buf"), Some("8"), "--data@buf:8", &[], &[]),
        (
            "param",
            None,
            Some("a=0;b=1"),
            "--param:a=0;b=1;",
            &["a", "b"],
            &["a", "b"],
        ),
        (
            "dialog",
            None,
            Some("Group::A,a=0;Group::A,b=1"),
            "--dialog:Group::A,a=0;Group::A,b=1;",
            &["dialog::Group::A", "Group::A"],
            &["A", "A"],
        ),
    ];

    #[test]
    fn processes_properties() {
        for &(kind, var, body, src, expected_names, expected_labels) in PROPERTY_CASES {
            let pos = Pos {
                file: Path::new("input.anm2").into(),
                line: 2,
                col: 3,
            };
            let mut state = State::default();
            state.vars = HashSet::from(["left", "right", "target", "cond"]);
            let mut l10n = l10n::Format::Full.collector();

            let actual = Prop::parse(pos, kind, var, body)
                .and_then(|prop| prop.map(|prop| prop.process(None, &mut state, &mut l10n)).transpose())
                .unwrap()
                .unwrap();
            if kind == "data" {
                assert!(state.names.contains("buf"));
            }

            assert_eq!(actual, src, "{kind}");
            let mut expected = l10n::Format::Full.collector();
            expected.push(expected_names, expected_labels);
            assert_eq!(l10n.render("input"), expected.render("input"), "{kind}");
        }
    }

    #[test]
    fn binds_only_when_requested() {
        let pos = || Pos {
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        };
        let run = |kind, var, body, binding, state: &mut State<'_>| {
            Prop::parse(pos(), kind, var, body).and_then(|prop| {
                prop.map(|prop| prop.process(binding, state, &mut l10n::Format::None.collector()))
                    .transpose()
            })
        };

        for (var, body, binding) in [
            ("val", "Value,0,10,5", None),
            (
                "_",
                "Value,0,10,_",
                Some(Binding {
                    var: "val",
                    val: Some("5"),
                }),
            ),
            ("_", "Value,0,10,5", Some(Binding { var: "val", val: None })),
        ] {
            let mut state = State::default();
            let src = run("track", Some(var), Some(body), binding, &mut state)
                .unwrap()
                .unwrap();
            assert_eq!(src, "--track@val:Value,0,10,5");
            assert_eq!(state.vars, HashSet::from(["val"]));
            assert!(state.keys.is_empty());
            assert_eq!(state.names, HashSet::from([Cow::Borrowed("Value")]));
        }

        let mut state = State::default();
        let src = run(
            "string",
            Some("_"),
            Some("Text,_"),
            Some(Binding {
                var: "val",
                val: Some("\"abc\""),
            }),
            &mut state,
        )
        .unwrap()
        .unwrap();
        assert_eq!(src, "--string@val:Text,abc");

        assert!(run("track", Some("val"), Some("Value,0,10,_"), None, &mut state).is_err());
        assert!(run("font", None, Some("Font,Arial"), None, &mut state).is_err());
        assert!(run("trackgroup", None, Some("Coords"), None, &mut state).is_err());
        assert!(run("checksection", None, Some("Section,true"), None, &mut state).is_err());
        assert!(run("value", None, Some("Value,0"), None, &mut state).is_err());
        assert!(run("value", Some("val"), None, None, &mut state).unwrap().is_none());
        assert!(run("unknown", None, None, None, &mut state).unwrap().is_none());

        let mut state = State::default();
        assert!(
            run(
                "track",
                Some("val"),
                Some("Value,0,10,5"),
                Some(Binding {
                    var: "val",
                    val: Some("6")
                }),
                &mut state,
            )
            .is_err()
        );
        assert!(state.keys.is_empty());
        assert!(state.vars.is_empty());
        assert!(state.names.is_empty());
        assert!(
            run(
                "data",
                Some("buf"),
                Some("8"),
                Some(Binding {
                    var: "val",
                    val: Some("0")
                }),
                &mut state,
            )
            .is_err()
        );
    }

    #[test]
    fn validates_underscore_without_binding() {
        let pos = || Pos {
            file: Path::new("input.anm2").into(),
            line: 2,
            col: 3,
        };

        for (kind, body, expected) in [
            ("track", "Value,0,10,5", "--track@_:Value,0,10,5"),
            ("check", "Value,true", "--check@_:Value,true"),
            ("checksection", "Value,true", "--checksection@_:Value,true"),
            ("color", "Value,0", "--color@_:Value,0x000000"),
            ("select", "Value=0,A=0", "--select@_:Value=0,A=0"),
            ("value", "Value,_", "--value@_:Value,_"),
            ("font", "Value,_", "--font@_:Value,_"),
            ("figure", "Value,_", "--figure@_:Value,_"),
            ("string", "Value,_", "--string@_:Value,_"),
            ("text", "Value,_", "--text@_:Value,_"),
            ("file", "Value", "--file@_:Value"),
            ("folder", "Value", "--folder@_:Value"),
        ] {
            let mut state = State::default();
            let src = Prop::parse(pos(), kind, Some("_"), Some(body))
                .and_then(|prop| {
                    prop.map(|prop| prop.process(None, &mut state, &mut l10n::Format::None.collector()))
                        .transpose()
                })
                .unwrap()
                .unwrap();
            assert_eq!(src, expected, "{kind}");
            assert_eq!(state.vars, HashSet::from(["_"]), "{kind}");
        }

        for (kind, body, expected) in [
            ("track", "Value,0,10,_", "must be a number"),
            ("check", "Value,_", "must be 'true', 'false', '0', or '1'"),
            ("checksection", "Value,_", "must be 'true' or 'false'"),
            ("color", "Value,_", "must be 'nil' or an integer"),
            ("select", "Value=_,A=0", "must be an integer"),
        ] {
            let mut state = State::default();
            let err = Prop::parse(pos(), kind, Some("_"), Some(body))
                .and_then(|prop| {
                    prop.map(|prop| prop.process(None, &mut state, &mut l10n::Format::None.collector()))
                        .transpose()
                })
                .unwrap_err();
            assert!(err.to_string().contains(expected), "{kind}: {err}");
            assert!(!err.to_string().contains("initializer"), "{kind}: {err}");
        }
    }

    #[test]
    fn handles_input_boundaries() {
        for body in [
            "",
            ",",
            ",,",
            ":",
            "_",
            "0",
            "NaN",
            "\"unterminated",
            "[[unterminated",
            "\u{3042},",
            "Label,0,1,0",
            "Name,,",
            "Name,1,2,3,4,5,6,",
            "x==0",
            "x<",
            "\0",
            "\r\n",
        ] {
            assert!(
                std::panic::catch_unwind(|| {
                    let pos = || Pos {
                        file: Path::new("input.anm2").into(),
                        line: 1,
                        col: 1,
                    };

                    let _ = check::Prop::parse(pos(), "check", Some("value"), body);
                    let _ = checksection::Prop::parse(pos(), "checksection", "value", body);
                    let _ = color::Prop::parse(pos(), "color", Some("value"), body);
                    let _ = data::Prop::parse(pos(), "data", "value", body);
                    let _ = dialog::Prop::parse(pos(), "dialog", None, body);
                    let _ = figure::Prop::parse(pos(), "figure", "value", body);
                    let _ = file::Prop::parse(pos(), "file", Some("value"), body);
                    let _ = folder::Prop::parse(pos(), "folder", "value", body);
                    let _ = font::Prop::parse(pos(), "font", "value", body);
                    let _ = group::Prop::parse(pos(), "group", Some("value"), Some(body));
                    let _ = hide::Prop::parse(pos(), "hide", "value", Some(body));
                    let _ = param::Prop::parse(pos(), "param", None, body);
                    let _ = select::Prop::parse(pos(), "select", "value", body);
                    let _ = separator::Prop::parse(pos(), "separator", Some("value"), body);
                    let _ = string::Prop::parse(pos(), "string", "value", body);
                    let _ = text::Prop::parse(pos(), "text", "value", body);
                    let _ = track::Prop::parse(pos(), "track", Some("value"), body);
                    let _ = trackgroup::Prop::parse(pos(), "trackgroup", "value,other", body);
                    let _ = value::Prop::parse(pos(), "value", "value", body);
                })
                .is_ok(),
                "{body:?}"
            );
        }
    }
}
