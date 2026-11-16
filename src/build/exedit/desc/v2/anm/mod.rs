pub(in crate::build::exedit) mod filter;

use crate::build::source::Pos;

pub(in crate::build::exedit) fn parse<'a>(
    pos: Pos,
    kind: &str,
    body: &'a str,
) -> anyhow::Result<Option<filter::Desc<'a>>> {
    filter::Desc::parse(pos, kind, body)
}

pub(in crate::build::exedit) fn is_kind(kind: &str) -> bool {
    kind == "filter"
}
