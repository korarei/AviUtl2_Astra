use crate::build::{ini, script};
use crate::config::{Build, BuildType};
use anyhow::{Context, bail};
use std::borrow::Cow;
use std::path::{Path, PathBuf};

pub(super) fn build(build_dir: &Path, build: &Build, build_type: BuildType) -> anyhow::Result<Vec<PathBuf>> {
    let targets = build.targets();

    if targets.len() == 0 {
        return Ok(Vec::new());
    }

    let is_ini = build
        .suffix()
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".aul2"));
    let is_bundled = targets.len() > 1 && !is_ini;
    let include_dirs_base: Vec<PathBuf> = build.include_dirs().iter().map(PathBuf::from).collect();

    let mut combined = script::Output::default();

    for target in targets {
        let target = target?;
        let file = Path::new(target.path());

        if !file.is_file() {
            bail!("target file '{}' not found", file.display());
        }

        let bytes = std::fs::read(file).with_context(|| format!("failed to read '{}'", file.display()))?;
        let (decoded, _, has_malformed) = target.encoding().decode(&bytes);
        if has_malformed {
            bail!(
                "failed to decode target file '{}' as {}: invalid byte sequence",
                file.display(),
                target.encoding().name()
            );
        }

        let src: Cow<'_, str> = if decoded.contains('\r') {
            Cow::Owned(decoded.replace("\r\n", "\n").replace('\r', "\n"))
        } else {
            decoded
        };

        let include_dirs: Vec<PathBuf> = if target.include_dirs().is_empty() {
            include_dirs_base.clone()
        } else {
            target
                .include_dirs()
                .iter()
                .map(PathBuf::from)
                .chain(include_dirs_base.iter().cloned())
                .collect()
        };

        if is_ini {
            combined
                .script
                .push_str(&ini::build(&src, &target, &include_dirs, target.vars())?);
        } else {
            combined.push_str(&script::build(
                &src,
                &target,
                &include_dirs,
                target.vars(),
                is_bundled,
                build.suffix().unwrap_or_default(),
            )?);
        }
    }

    if build.newline() == "\r\n" {
        combined.replace("\n", "\r\n");
    }

    let bytes = if is_ini {
        Cow::Owned(combined.script.into_bytes())
    } else {
        let (bytes, _, has_unmappable) = build.encoding().encode(&combined.script);
        if has_unmappable {
            bail!(
                "failed to encode build output in {}: contains unmappable characters",
                build.encoding().name()
            );
        }
        bytes
    };

    let filename = format!(
        "{}{}{}",
        if is_bundled { "@" } else { "" },
        build.name(),
        build.suffix().unwrap_or_default()
    );

    let output_dir = build_dir.join(build_type.as_lowercase());

    let output_file = output_dir.join(&filename);

    tracing::info!("Writing build output to '{}'", output_file.display());
    crate::fs::write_file(&output_file, &bytes)?;

    if !is_ini && build.suffix().is_some_and(|suffix| suffix.ends_with('2')) {
        let output_file = output_dir.join(format!("Default.{}.aul2", build.name()));
        tracing::info!("Writing localization template to '{}'", output_file.display());
        crate::fs::write_file(&output_file, combined.l10n.as_bytes())?;
    }

    Ok(vec![std::path::absolute(&output_file)?])
}

#[cfg(test)]
mod tests {
    use super::build;
    use crate::config::{Build, BuildType};
    use crate::vars::Scope;
    use anyhow::Context;
    use std::fs;

    #[test]
    fn writes_single_and_bundled_targets() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let one = dir.path().join("one.lua");
        fs::write(&one, "return 1\n")?;
        let one = one.to_string_lossy().replace('\\', "/");

        let mut spec: Build = serde_json::from_value(serde_json::json!({
            "name": "demo",
            "suffix": "anm2",
            "target": { "path": one.clone() }
        }))?;
        spec.expanded(&Scope::default())?;

        let files = build(dir.path(), &spec, BuildType::Debug)?;
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::read_to_string(&files[0]).with_context(|| format!("failed to read '{}'", files[0].display()))?,
            "return 1\r\n"
        );
        assert_eq!(
            fs::read_to_string(files[0].parent().unwrap().join("Default.demo.aul2")).with_context(|| {
                format!(
                    "failed to read '{}'",
                    files[0].parent().unwrap().join("Default.demo.aul2").display()
                )
            })?,
            "[demo]\r\ndemo=\r\n\r\n[Tips.demo]\r\neffect.name=\r\n\r\n"
        );

        let two = dir.path().join("two.lua");
        fs::write(&two, "return 2\n")?;
        let two = two.to_string_lossy().replace('\\', "/");
        let mut spec: Build = serde_json::from_value(serde_json::json!({
            "name": "demo",
            "suffix": "anm2",
            "target": [
                { "name": "one", "path": one.clone() },
                { "name": "two", "path": two }
            ]
        }))?;
        spec.expanded(&Scope::default())?;

        let files = build(dir.path(), &spec, BuildType::Release)?;
        assert_eq!(files.len(), 1);
        assert_eq!(
            fs::read_to_string(&files[0]).with_context(|| format!("failed to read '{}'", files[0].display()))?,
            "@one\r\nreturn 1\r\n@two\r\nreturn 2\r\n"
        );
        assert_eq!(
            fs::read_to_string(files[0].parent().unwrap().join("Default.demo.aul2")).with_context(|| {
                format!(
                    "failed to read '{}'",
                    files[0].parent().unwrap().join("Default.demo.aul2").display()
                )
            })?,
            "[one@demo]\r\none@demo=\r\n\r\n[Tips.one@demo]\r\neffect.name=\r\n\r\n\
             [two@demo]\r\ntwo@demo=\r\n\r\n[Tips.two@demo]\r\neffect.name=\r\n\r\n"
        );

        let mut spec: Build = serde_json::from_value(serde_json::json!({
            "name": "legacy",
            "suffix": "anm",
            "target": { "path": one }
        }))?;
        spec.expanded(&Scope::default())?;
        let files = build(dir.path(), &spec, BuildType::Release)?;
        assert_eq!(files.len(), 1);
        assert!(!files[0].parent().unwrap().join("Default.legacy.aul2").exists());

        Ok(())
    }
}
