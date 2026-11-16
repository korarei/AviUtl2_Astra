pub(in crate::build::exedit) mod effect;
pub(in crate::build::exedit) mod tra;

use crate::build::exedit::element::{Binding, State};
use crate::build::exedit::l10n;

pub(in crate::build::exedit) enum Prop<'a> {
    Effect(effect::Prop<'a>),
    Tra(tra::Prop<'a>),
}

impl<'a> Prop<'a> {
    pub(super) fn can_bind(&self) -> bool {
        matches!(self, Self::Effect(prop) if prop.can_bind())
    }

    pub(super) fn process(
        self,
        binding: Option<Binding<'a, '_>>,
        state: &mut State<'a>,
        l10n: &mut l10n::Collector,
    ) -> anyhow::Result<String> {
        match self {
            Self::Effect(prop) => prop.process(binding, state, l10n),
            Self::Tra(prop) => prop.process(l10n),
        }
    }
}

#[cfg(test)]
mod tests {
    fn is_prop(suffix: &str, src: &str) -> anyhow::Result<bool> {
        let Some(annotation) = crate::build::exedit::element::Annotation::parse(src) else {
            return Ok(false);
        };

        crate::build::exedit::format::is_prop(suffix, &annotation)
    }

    #[test]
    fn recognizes_v2_properties() {
        for suffix in [".anm2", ".obj2", ".cam2", ".scn2", ".ANM2"] {
            for comment in [
                "track@x:Scale,0,100,50,1",
                "track0:Scale,0,100,50,1",
                "track 0:Scale,0,100,50,1",
                "track custom:Scale,0,100,50,1",
                "trackgroup@a,b:Group",
                "check@flag:Enable,true",
                "check0:Enable,0",
                "checksection@sec:Section,true",
                "check custom:Enable,0",
                "color@col:Color,0xffffff",
                "color:0xffffff",
                "file@path:File",
                "file:",
                "folder@dir:Folder",
                "font@font:Font,Arial",
                "figure@fig:Figure,四角形",
                "select@mode:Mode,A=0",
                "value@val:Value,0",
                "string@s:Text,abc",
                "text@t:Text,abc",
                "group:Group,true",
                "group",
                "group  ",
                "separator:Sep",
                "hide@x",
                "hide@x  ",
                "data@buf:100",
                "dialog:Label/chk,var=0;",
                "param:a=1",
            ] {
                assert!(is_prop(suffix, comment).unwrap(), "{suffix}: {comment}");
            }

            for comment in [
                " track@x:Scale,0,100,50,1",
                " group",
                "group extra",
                "information:Effect v1.0",
                "label:Blur",
                "require:2003500",
                "filter",
                "hidemenu",
                "script:LuaJIT",
                "timecontrol",
                "",
            ] {
                assert!(!is_prop(suffix, comment).unwrap(), "{suffix}: {comment}");
            }
        }

        for comment in [
            "param:0",
            "param:Label,0",
            "param:Flag/check,1",
            "param:Mode/select/A=0/B=1,0",
            "speed:1,0",
            "twopoint",
            "twopoint  ",
            "twopoint:",
            "twopoint : extra,more: ",
        ] {
            assert!(is_prop(".tra2", comment).unwrap(), "{comment}");
        }

        for comment in [
            " param:0",
            " twopoint:extra",
            "twopoint,extra",
            "twopoint extra:1",
            "twopoint extra",
            "timecontrol",
            "information:Transition v1.0",
        ] {
            assert!(!is_prop(".tra2", comment).unwrap(), "{comment}");
        }

        for suffix in [".anm2", ".obj2", ".cam2", ".scn2"] {
            for comment in ["twopoint", "speed:1,0"] {
                assert!(is_prop(suffix, comment).is_err(), "{suffix}: {comment}");
            }
        }

        for comment in ["track@x:Scale,0,100,50,1", "track0:Scale,0,100,50,1", "group"] {
            assert!(is_prop(".tra2", comment).is_err(), "{comment}");
        }
    }
}
