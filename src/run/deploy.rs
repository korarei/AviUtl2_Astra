use crate::build::BuildOutput;
use crate::config::Package;
use crate::package::PackageCache;
use anyhow::{Context, bail};
use std::collections::BTreeMap;
use std::path::Path;

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
                let dst = file.dst.to_string_lossy().into_owned();
                #[cfg(windows)]
                let dst = dst.replace('/', "\\");
                links.insert(dst, file.src);
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
        .map(|(dst, target)| {
            let encode = |path: &Path| {
                #[cfg(windows)]
                return path.to_string_lossy().replace('/', "\\");
                #[cfg(not(windows))]
                path.to_string_lossy().into_owned()
            };

            (
                dst.clone(),
                target
                    .strip_prefix(cache.root())
                    .map_or_else(|_| encode(target), encode),
            )
        })
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
        if !is_same_link(&path, target)? {
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

pub(super) fn is_same_link(path: &Path, target: &Path) -> anyhow::Result<bool> {
    let Ok(link) = std::fs::read_link(path) else {
        return Ok(false);
    };

    let path = if link.is_absolute() {
        link
    } else {
        let Some(dir) = path.parent() else {
            return Ok(false);
        };
        dir.join(link)
    };

    Ok(crate::fs::to_key(&path)? == crate::fs::to_key(target)?)
}
