pub(super) mod v0;
pub(super) mod v2;

use crate::build::exedit::element::{Binding, State};
use crate::build::exedit::l10n;

pub(super) enum Prop<'a> {
    V2(v2::Prop<'a>),
    V0(v0::Prop<'a>),
}

impl<'a> Prop<'a> {
    pub(super) fn can_bind(&self) -> bool {
        matches!(self, Self::V2(prop) if prop.can_bind())
    }

    pub(super) fn process(
        self,
        binding: Option<Binding<'a, '_>>,
        state: &mut State<'a>,
        l10n: &mut l10n::Collector,
    ) -> anyhow::Result<String> {
        match self {
            Self::V2(prop) => prop.process(binding, state, l10n),
            Self::V0(prop) => prop.process(state, l10n),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::build::exedit::element::Annotation;
    use crate::build::exedit::format;

    fn is_prop(suffix: &str, src: &str) -> anyhow::Result<bool> {
        let Some(annotation) = Annotation::parse(src) else {
            return Ok(false);
        };

        format::is_prop(suffix, &annotation)
    }

    #[test]
    fn rejects_unsupported_properties() {
        for (src, suffix) in [
            ("twopoint", ".anm2"),
            ("speed:1,0", ".anm2"),
            ("track0:Scale,0,100,50,1", ".tra2"),
            ("group", ".tra2"),
            ("group", ".anm"),
            ("track2:Scale,0,100,50,1", ".scn"),
            ("twopoint:extra", ".tra"),
        ] {
            let err = is_prop(suffix, src).unwrap_err();
            assert!(
                err.to_string().contains(&format!("is not supported in '{suffix}'")),
                "{src}: {err}"
            );
        }

        for suffix in [".anm2", ".tra2", ".anm", ".tra"] {
            assert!(is_prop(suffix, "param:0").unwrap());

            for src in [
                "ordinary comment",
                "unknown:value",
                " twopoint",
                "twopoint extra",
                "filter",
            ] {
                assert!(!is_prop(suffix, src).unwrap(), "{suffix}: {src}");
            }
        }
    }
}
