pub(in crate::build::exedit) mod hidemenu;
pub(in crate::build::exedit) mod info;
pub(in crate::build::exedit) mod label;
pub(in crate::build::exedit) mod require;
pub(in crate::build::exedit) mod script;

use crate::build::source::Pos;

pub(in crate::build::exedit) enum Desc<'a> {
    Hidemenu(hidemenu::Desc<'a>),
    Info(info::Desc<'a>),
    Label(label::Desc<'a>),
    Require(require::Desc<'a>),
    Script(script::Desc<'a>),
}

impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn parse(pos: Pos, kind: &str, body: Option<&'a str>) -> anyhow::Result<Option<Self>> {
        Ok(Some(match (kind, body) {
            ("hidemenu", body) => Self::Hidemenu(hidemenu::Desc::parse(pos, kind, body.unwrap_or_default())?.unwrap()),
            ("information", Some(body)) => Self::Info(info::Desc::parse(pos, kind, body)?.unwrap()),
            ("label", Some(body)) => Self::Label(label::Desc::parse(pos, kind, body)?.unwrap()),
            ("require", Some(body)) => Self::Require(require::Desc::parse(pos, kind, body)?.unwrap()),
            ("script", Some(body)) => Self::Script(script::Desc::parse(pos, kind, body)?.unwrap()),
            _ => return Ok(None),
        }))
    }

    pub(super) fn process(self) -> anyhow::Result<String> {
        match self {
            Self::Hidemenu(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
            Self::Info(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
            Self::Label(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
            Self::Require(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
            Self::Script(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
        }
    }
}

pub(in crate::build::exedit) fn is_kind(kind: &str, has_body: bool) -> bool {
    kind == "hidemenu" || (has_body && matches!(kind, "information" | "label" | "require" | "script"))
}
