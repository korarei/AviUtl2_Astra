pub(in crate::build::exedit) mod effect;
pub(in crate::build::exedit) mod tra;

use crate::build::exedit::element::State;
use crate::build::exedit::l10n;

pub(in crate::build::exedit) enum Prop<'a> {
    Effect(effect::Prop<'a>),
    Tra(tra::Prop),
}

impl<'a> Prop<'a> {
    pub(super) fn process(self, state: &mut State<'a>, l10n: &mut l10n::Collector) -> anyhow::Result<String> {
        match self {
            Self::Effect(prop) => prop.process(state, l10n),
            Self::Tra(prop) => prop.process(),
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
    fn recognizes_v0_properties() {
        for suffix in [".anm", ".obj", ".cam", ".ANM"] {
            for comment in [
                "track0:Scale,0,100,50,1",
                "track1:Scale,0,100,50,1",
                "track2:Scale,0,100,50,1",
                "track3:Scale,0,100,50,1",
                "check0:Enable,0",
                "color:0xffffff",
                "file:",
                "param:dx=10;",
                "dialog:Label/chk,var=0;",
            ] {
                assert!(is_prop(suffix, comment).unwrap(), "{suffix}: {comment}");
            }

            for comment in [" track0:Scale,0,100,50,1", "information:Effect v1.0", ""] {
                assert!(!is_prop(suffix, comment).unwrap(), "{suffix}: {comment}");
            }

            for comment in [
                "track@x:Scale,0,100,50,1",
                "track 0:Scale,0,100,50,1",
                "track4:Scale,0,100,50,1",
                "check1:Enable,0",
                "value@val:Value,0",
                "group",
                "hide@x",
                "twopoint",
                "speed:1,0",
            ] {
                assert!(is_prop(suffix, comment).is_err(), "{suffix}: {comment}");
            }
        }

        for suffix in [".scn", ".SCN"] {
            for comment in [
                "track0:Scale,0,100,50,1",
                "track1:Scale,0,100,50,1",
                "check0:Enable,0",
                "color:0xffffff",
                "file:",
                "param:dx=10;",
                "dialog:Label/chk,var=0;",
            ] {
                assert!(is_prop(suffix, comment).unwrap(), "{suffix}: {comment}");
            }

            for comment in ["track2:Scale,0,100,50,1", "track3:Scale,0,100,50,1"] {
                assert!(is_prop(suffix, comment).is_err(), "{suffix}: {comment}");
            }
        }

        for comment in ["param:0", "speed:1,0", "twopoint", "twopoint  "] {
            assert!(is_prop(".tra", comment).unwrap(), "{comment}");
        }

        for comment in [" param:0", "twopoint extra", "timecontrol"] {
            assert!(!is_prop(".tra", comment).unwrap(), "{comment}");
        }

        for comment in ["twopoint:1", "track0:Scale,0,100,50,1", "check0:Enable,0"] {
            assert!(is_prop(".tra", comment).is_err(), "{comment}");
        }
    }
}
