use crate::build::BuildOutput;
use crate::config::PackageContent;
use crate::package::PackageCache;
use anyhow::{Context, bail};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) fn stage<'a>(
    base_dir: Option<&Path>,
    cache: &mut PackageCache,
    contents: impl IntoIterator<Item = anyhow::Result<&'a PackageContent>>,
    outputs: &BTreeMap<String, BuildOutput>,
    is_au2pkg: bool,
) -> anyhow::Result<tempfile::TempDir> {
    let staging = match base_dir {
        Some(dir) => tempfile::tempdir_in(dir)
            .with_context(|| format!("failed to create temporary directory in '{}'", dir.display()))?,
        None => tempfile::tempdir().with_context(|| {
            format!(
                "failed to create temporary directory in '{}'",
                std::env::temp_dir().display()
            )
        })?,
    };

    if is_au2pkg {
        cache.reserve(Path::new("package.ini"))?;
        cache.reserve(Path::new("package.txt"))?;
    }

    for content in contents {
        let content = content?;
        for src in content.sources() {
            for file in cache.resolve(Path::new(content.dir()), src, outputs)? {
                copy(&staging.path().join(file.dst), &file.src)?;
            }
        }
    }

    Ok(staging)
}

fn copy(dst: &Path, src: &Path) -> anyhow::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.is_symlink() {
        bail!("release package cannot contain a symbolic link: '{}'", src.display());
    }

    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
    }

    if meta.is_dir() {
        std::fs::create_dir_all(dst).with_context(|| format!("failed to create directory '{}'", dst.display()))?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            copy(&dst.join(entry.file_name()), &entry.path())?;
        }
    } else if meta.is_file() {
        std::fs::copy(src, dst)
            .with_context(|| format!("failed to copy '{}' to '{}'", src.display(), dst.display()))?;
    } else {
        bail!("release source is not a regular file or directory: '{}'", src.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn stages_sources_and_rejects_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let stage = |contents, outputs: &BTreeMap<String, BuildOutput>, is_au2pkg| {
            let (_dir, mut cache) = PackageCache::create_temp()?;
            super::stage(None, &mut cache, contents, outputs, is_au2pkg)
        };
        let content: PackageContent = serde_json::from_value(serde_json::json!({
            "directory": "Plugin",
            "sources": [{ "filename": "test.txt", "content": "hello" }],
        }))
        .unwrap();
        let staging = stage([Ok(&content)], &BTreeMap::new(), false).unwrap();
        assert_eq!(
            fs::read_to_string(staging.path().join("Plugin/test.txt"))
                .with_context(|| format!("failed to read '{}'", staging.path().join("Plugin/test.txt").display()))
                .unwrap(),
            "hello"
        );

        let encoded: PackageContent = serde_json::from_value(serde_json::json!({
            "directory": "Plugin",
            "sources": [{
                "filename": "nested/file.txt",
                "content": "a\r\nb\rc\n",
                "encoding": "utf-8",
                "newline": "lf"
            }],
        }))
        .unwrap();
        let staging = stage([Ok(&encoded)], &BTreeMap::new(), false).unwrap();
        assert_eq!(
            fs::read_to_string(staging.path().join("Plugin/nested/file.txt"))
                .with_context(|| {
                    format!(
                        "failed to read '{}'",
                        staging.path().join("Plugin/nested/file.txt").display()
                    )
                })
                .unwrap(),
            "a\nb\nc\n"
        );

        let artifact = temp.path().join("main.dll");
        fs::write(&artifact, "artifact").unwrap();
        let artifact_content: PackageContent = serde_json::from_value(serde_json::json!({
            "directory": "Plugin",
            "sources": [{ "build": "main" }],
        }))
        .unwrap();
        let staging = stage(
            [Ok(&artifact_content)],
            &BTreeMap::from([(
                "main".to_owned(),
                BuildOutput {
                    artifacts: vec![artifact],
                    hash: 0,
                },
            )]),
            false,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(staging.path().join("Plugin/main.dll"))
                .with_context(|| format!("failed to read '{}'", staging.path().join("Plugin/main.dll").display()))
                .unwrap(),
            "artifact"
        );

        let duplicate: PackageContent = serde_json::from_value(serde_json::json!({
            "directory": "Plugin",
            "sources": [
                { "filename": "same.txt", "content": "first" },
                { "filename": "same.txt", "content": "second" },
            ],
        }))
        .unwrap();
        let error = stage([Ok(&duplicate)], &BTreeMap::new(), false).unwrap_err();
        assert!(error.to_string().contains("specified more than once"));

        let reserved: PackageContent = serde_json::from_value(serde_json::json!({
            "directory": ".",
            "sources": [{ "filename": "package.ini", "content": "custom ini" }],
        }))
        .unwrap();
        let error = stage([Ok(&reserved)], &BTreeMap::new(), true).unwrap_err();
        assert!(error.to_string().contains("specified more than once"));
    }
}
