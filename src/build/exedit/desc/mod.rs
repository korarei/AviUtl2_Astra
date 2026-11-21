pub(super) mod v2;

#[cfg(test)]
mod tests {
    fn is_desc(suffix: &str, src: &str) -> anyhow::Result<bool> {
        let Some(annotation) = crate::build::exedit::element::Annotation::parse(src) else {
            return Ok(false);
        };

        crate::build::exedit::format::is_desc(suffix, &annotation)
    }

    #[test]
    fn rejects_unsupported_descriptors() {
        for (src, suffix) in [
            ("filter", ".tra2"),
            ("filter:extra", ".obj2"),
            ("timecontrol", ".anm2"),
            ("hidemenu", ".anm"),
            ("information:Effect", ".tra"),
        ] {
            let error = is_desc(suffix, src).unwrap_err();
            assert!(
                error.to_string().contains(&format!("is not supported in '{suffix}'")),
                "{src}: {error}"
            );
        }

        for suffix in [".anm2", ".tra2", ".anm", ".tra"] {
            for src in ["ordinary comment", "unknown:value", " filter", "filter out", "twopoint"] {
                assert!(!is_desc(suffix, src).unwrap(), "{suffix}: {src}");
            }
        }
    }
}
