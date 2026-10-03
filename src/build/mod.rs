pub mod hlsl;
pub mod ini;
pub mod lua;
mod preprocess;
pub mod script;
pub mod shader;
mod target;

use crate::config::{Astra, Build, BuildType, Config};
use anyhow::{Context, bail};
use fd_lock::RwLock;
use std::path::{Path, PathBuf};
use wax::walk::Entry;

pub(crate) struct BuildOutput {
    pub(crate) artifacts: Vec<PathBuf>,
    pub(crate) hash: u128,
}

#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Args {
    /// ID of the specific build target to execute.
    ///
    /// If omitted, all enabled build targets will be executed.
    #[arg(value_name = "ID")]
    id: Option<String>,

    /// Build in release mode.
    #[arg(long, conflicts_with = "debug")]
    release: bool,

    /// Build in debug mode (default).
    #[arg(long, conflicts_with = "release")]
    debug: bool,
}

impl Args {
    #[must_use]
    fn build_type(&self) -> BuildType {
        if self.release {
            BuildType::Release
        } else {
            BuildType::Debug
        }
    }
}

pub(super) fn command(config: &Config, args: &Args) -> anyhow::Result<()> {
    tracing::info!("Running build ({})", args.build_type());

    let builds = config.builds(args.build_type());

    if builds.len() == 0 {
        bail!("no build configurations found");
    }

    let astra = config.astra();

    let build_dir = Path::new(astra.build_dir());

    crate::fs::create_managed_dir(build_dir)?;

    let mut lock = RwLock::new(
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(build_dir.join(".astra-lock"))
            .with_context(|| format!("failed to open '{}'", build_dir.join(".astra-lock").display()))?,
    );
    let _guard = if let Ok(guard) = lock.try_write() {
        guard
    } else {
        tracing::info!("Blocking waiting for file lock on build directory (press Ctrl+C to cancel)");
        lock.write()
            .with_context(|| format!("failed to lock '{}'", build_dir.join(".astra-lock").display()))?
    };

    if let Some(id) = &args.id {
        let build = config.build(id, args.build_type())?;

        if !build.is_enabled(args.build_type()) {
            bail!(
                "build '{}' is disabled for {} build type",
                id,
                args.build_type().as_lowercase()
            );
        }

        let _ = run(&build, config, astra, args.build_type())?;
    } else {
        for build in builds {
            let build = build?;

            if !build.is_enabled(args.build_type()) {
                continue;
            }

            let _ = run(&build, config, astra, args.build_type())?;
        }
    }

    Ok(())
}

pub(crate) fn run(build: &Build, config: &Config, astra: &Astra, build_type: BuildType) -> anyhow::Result<BuildOutput> {
    tracing::info!("Building '{}'", build.id());

    let root = Path::new(astra.build_dir());
    let build_dir = root.join(build.id());
    std::fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create directory '{}'", build_dir.display()))?;

    for call in build.depends() {
        tracing::debug!("calling dependency '{}'", call.id());

        crate::task::run(call, config, false)?;
    }

    let (mut artifacts, mut err) = match target::build(&build_dir, build, build_type) {
        Ok(targets) => (targets, None),
        Err(e) => (Vec::new(), Some(e)),
    };

    for call in build.finally() {
        tracing::debug!("calling finally task '{}'", call.id());

        if let Err(e) = crate::task::run(call, config, false) {
            err = Some(match err {
                Some(prev) => prev.context(format!("finally task '{}' failed: {e}", call.id())),
                None => e,
            });
        }
    }

    if let Some(e) = err {
        return Err(e);
    }

    for pattern in build.artifacts() {
        for entry in wax::Glob::new(pattern)?.walk(".") {
            artifacts.push(std::path::absolute(entry?.path())?);
        }
    }

    artifacts.sort_unstable();

    Ok(BuildOutput {
        hash: {
            let mut hash = xxhash_rust::xxh3::Xxh3::new();
            hash.update(build_type.as_lowercase().as_bytes());

            for path in &artifacts {
                if path.is_file() {
                    let file = path.to_string_lossy();
                    hash.update(&(file.len() as u64).to_le_bytes());
                    hash.update(file.as_bytes());
                    hash.update(&crate::fs::hash_file(path)?.to_le_bytes());
                } else if path.is_dir() {
                    hash_directory(path, &mut hash)?;
                } else {
                    bail!("artifact '{}' is not a regular file or directory", path.display());
                }
            }

            hash.digest128()
        },
        artifacts,
    })
}

fn hash_directory(dir: &Path, hash: &mut xxhash_rust::xxh3::Xxh3) -> anyhow::Result<()> {
    let path = dir.to_string_lossy();
    hash.update(&(path.len() as u64).to_le_bytes());
    hash.update(path.as_bytes());

    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        entries.push((entry.file_name(), entry.file_type()?));
    }

    entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    for (name, kind) in entries {
        let path = dir.join(name);

        if kind.is_dir() {
            hash_directory(&path, hash)?;
        } else if kind.is_file() {
            let file = path.to_string_lossy();
            hash.update(&(file.len() as u64).to_le_bytes());
            hash.update(file.as_bytes());
            hash.update(&crate::fs::hash_file(&path)?.to_le_bytes());
        } else {
            bail!("artifact '{}' contains a non-regular file", path.display());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::hash_directory;
    use std::fs;

    #[test]
    fn hashes_artifacts() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("main.dll");
        fs::write(&file, b"one")?;

        let first = crate::fs::hash_file(&file)?;
        assert_eq!(first, xxhash_rust::const_xxh3::xxh3_128(b"one"));

        fs::write(&file, b"two")?;
        assert_ne!(first, crate::fs::hash_file(&file)?);

        let tree = dir.path().join("assets");
        fs::create_dir(&tree)?;
        fs::create_dir(tree.join("sub"))?;
        fs::write(tree.join("sub").join("file.lua"), b"return 1")?;

        let mut hash = xxhash_rust::xxh3::Xxh3::new();
        hash_directory(&tree, &mut hash)?;
        let first = hash.digest128();

        fs::write(tree.join("sub").join("file.lua"), b"return 2")?;
        let mut hash = xxhash_rust::xxh3::Xxh3::new();
        hash_directory(&tree, &mut hash)?;
        assert_ne!(first, hash.digest128());

        Ok(())
    }
}
