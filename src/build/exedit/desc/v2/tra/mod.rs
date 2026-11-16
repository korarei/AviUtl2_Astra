pub(in crate::build::exedit) mod timecontrol;

use crate::build::source::Pos;

pub(in crate::build::exedit) fn parse<'a>(
    pos: Pos,
    kind: &str,
    body: &'a str,
) -> anyhow::Result<Option<timecontrol::Desc<'a>>> {
    timecontrol::Desc::parse(pos, kind, body)
}

pub(in crate::build::exedit) fn is_kind(kind: &str) -> bool {
    kind == "timecontrol"
}
