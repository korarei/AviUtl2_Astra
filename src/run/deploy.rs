use crate::build::BuildOutput;
use crate::config::Package;
use crate::package::PackageCache;
use anyhow::{Context, bail};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn deploy(
    data_dir: &Path,
    package: &Package,
    outputs: &BTreeMap<String, BuildOutput>,
    manifest: &mut super::RuntimeManifest,
    cache: &mut PackageCache,
) -> anyhow::Result<Option<u128>> {
    let mut links = BTreeMap::new();

    for content in package.contents() {
        let content = content?;
        for src in content.sources() {
            for file in cache.resolve(Path::new(content.dir()), src, outputs)? {
                links.insert(file.dst.to_string_lossy().replace('\\', "/"), file.src);
            }
        }
    }

    for dst in manifest.aviutl2.links.keys().chain(links.keys()) {
        let path = data_dir.join(dst);
        match std::fs::symlink_metadata(&path) {
            Ok(meta) if !meta.is_symlink() => {
                bail!("refusing to replace existing file or directory '{}'", path.display());
            }
            Ok(_) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("failed to inspect '{}'", path.display())),
        }
    }

    let hash = cache.hash(package.contents())?;
    let targets: BTreeMap<_, _> = links
        .iter()
        .map(|(dst, target)| (dst.clone(), describe(cache.root(), target)))
        .collect();
    manifest.config = None;
    manifest.build = None;
    manifest.aviutl2.package = None;
    manifest
        .aviutl2
        .links
        .extend(targets.iter().map(|(dst, target)| (dst.clone(), target.clone())));
    manifest.write()?;

    for dst in manifest
        .aviutl2
        .links
        .keys()
        .filter(|dst| !links.contains_key(dst.as_str()))
    {
        let path = data_dir.join(dst);
        crate::fs::remove(&path).with_context(|| format!("failed to remove outdated link '{}'", path.display()))?;
    }

    for (dst, target) in &links {
        let path = data_dir.join(dst);
        if !is_same_link(&path, target) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create directory '{}'", parent.display()))?;
            }
            crate::fs::remove(&path)?;
            link(&path, target)?;
        }
    }

    manifest.aviutl2.links = targets;
    Ok(hash)
}

fn link(dst: &Path, src: &Path) -> anyhow::Result<()> {
    let src = std::path::absolute(src)?;

    #[cfg(windows)]
    {
        if let Err(err) = if src.is_dir() {
            std::os::windows::fs::symlink_dir(&src, dst)
        } else {
            std::os::windows::fs::symlink_file(&src, dst)
        } {
            if err.raw_os_error() == Some(1314) {
                bail!(
                    "failed to create symbolic link '{}' -> '{}': \
                     Windows Developer Mode must be enabled, or Astra must be run as administrator",
                    dst.display(),
                    src.display()
                );
            }

            return Err(err).with_context(|| {
                format!(
                    "failed to create symbolic link '{}' -> '{}'",
                    dst.display(),
                    src.display()
                )
            });
        }
    }

    #[cfg(not(windows))]
    std::os::unix::fs::symlink(&src, dst).with_context(|| {
        format!(
            "failed to create symbolic link '{}' -> '{}'",
            dst.display(),
            src.display()
        )
    })?;

    Ok(())
}

fn read_link_target(path: &Path) -> Option<PathBuf> {
    let target = std::fs::read_link(path).ok()?;
    Some(if target.is_absolute() {
        target
    } else {
        path.parent()?.join(target)
    })
}

pub(super) fn is_same_link(path: &Path, target: &Path) -> bool {
    read_link_target(path).is_some_and(|path| {
        let Ok(path) = std::path::absolute(path) else {
            return false;
        };

        let Ok(target) = std::path::absolute(target) else {
            return false;
        };

        #[cfg(windows)]
        {
            path.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy())
        }

        #[cfg(not(windows))]
        {
            path == target
        }
    })
}

fn describe(cache_dir: &Path, target: &Path) -> String {
    target.strip_prefix(cache_dir).map_or_else(
        |_| target.to_string_lossy().replace('\\', "/"),
        |path| path.to_string_lossy().replace('\\', "/"),
    )
}
