pub(in crate::build::exedit) mod anm;
pub(in crate::build::exedit) mod general;
pub(in crate::build::exedit) mod tra;

pub(in crate::build::exedit) enum Desc<'a> {
    General(general::Desc<'a>),
    Filter(anm::filter::Desc<'a>),
    Timecontrol(tra::timecontrol::Desc<'a>),
}

impl<'a> Desc<'a> {
    pub(in crate::build::exedit) fn process(self) -> anyhow::Result<String> {
        match self {
            Self::General(desc) => desc.process(),
            Self::Filter(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
            Self::Timecontrol(desc) => {
                desc.validate()?;
                Ok(desc.to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    fn is_desc(suffix: &str, src: &str) -> anyhow::Result<bool> {
        let Some(annotation) = crate::build::exedit::element::Annotation::parse(src) else {
            return Ok(false);
        };

        crate::build::exedit::format::is_desc(suffix, &annotation)
    }

    #[test]
    fn recognizes_v2_descriptors() {
        for suffix in [".anm2", ".obj2", ".cam2", ".scn2", ".tra2", ".ANM2"] {
            for comment in [
                "information:Effect v1.0",
                "label:Blur",
                "require:2003500",
                "hidemenu",
                "hidemenu:",
                "hidemenu : extra,more: ",
                "script:LuaJIT",
            ] {
                assert!(is_desc(suffix, comment).unwrap(), "{suffix}: {comment}");
            }

            for comment in [
                " information:Effect v1.0",
                "information@x:Effect",
                "hidemenu,extra",
                "hidemenu extra:1",
                " hidemenu:extra",
                "track@x:Scale,0,100,50,1",
                "param:a=1",
                "twopoint",
                "speed:1,0",
                "unknown:value",
                "",
            ] {
                assert!(!is_desc(suffix, comment).unwrap(), "{suffix}: {comment}");
            }
        }

        for comment in ["filter", "filter  ", "filter:", "filter : extra,more: "] {
            assert!(is_desc(".anm2", comment).unwrap(), "{comment}");

            for suffix in [".obj2", ".cam2", ".scn2", ".tra2", ".anm"] {
                assert!(is_desc(suffix, comment).is_err(), "{suffix}: {comment}");
            }
        }
        for comment in ["timecontrol", "timecontrol:", "timecontrol : extra,more: "] {
            assert!(is_desc(".tra2", comment).unwrap(), "{comment}");

            for suffix in [".anm2", ".obj2", ".cam2", ".scn2", ".tra"] {
                assert!(is_desc(suffix, comment).is_err(), "{suffix}: {comment}");
            }
        }

        for (kind, suffix) in [("filter", ".anm2"), ("timecontrol", ".tra2")] {
            for comment in [
                format!("{kind},extra"),
                format!("{kind} extra:1"),
                format!(" {kind}:extra"),
            ] {
                assert!(!is_desc(suffix, &comment).unwrap(), "{suffix}: {comment}");
            }
        }
    }
}
