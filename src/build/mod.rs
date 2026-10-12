pub mod hlsl;
pub mod ini;
pub mod lua;
mod preprocess;
pub mod script;
pub mod shader;
mod source;
mod target;
mod validate;

use crate::config::{Astra, Build, BuildCall, BuildType, Config};
use anyhow::{Context, bail};
use std::path::{Path, PathBuf};
use wax::walk::Entry;

pub(crate) struct BuildOutput {
    pub(crate) artifacts: Vec<PathBuf>,
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) hash: u128,
}

#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Args {
    #[arg(
        value_name = "ID",
        help = "ID of the specific build target to execute",
        long_help = "ID of the specific build target to execute.\n\nIf omitted, all enabled build targets will be executed."
    )]
    id: Option<String>,

    #[arg(
        id = "release",
        long = "release",
        conflicts_with = "debug",
        help = "Build in release mode"
    )]
    is_release: bool,

    #[arg(
        id = "debug",
        long = "debug",
        conflicts_with = "release",
        help = "Build in debug mode (default)"
    )]
    is_debug: bool,
}

impl Args {
    #[must_use]
    fn build_type(&self) -> BuildType {
        if self.is_release {
            BuildType::Release
        } else {
            BuildType::Debug
        }
    }
}

pub(super) fn command(config: &Config, args: &Args) -> anyhow::Result<()> {
    let build_type = args.build_type();
    tracing::info!("Running build ({build_type})");

    let builds = config.builds(build_type);

    if builds.len() == 0 {
        bail!("no build configurations found");
    }

    let astra = config.astra();
    let build_dir = Path::new(astra.build_dir());
    crate::fs::create_managed_dir(build_dir)?;

    let mut lock = crate::fs::Lock::new(build_dir)?;
    let _guard = lock.write()?;

    if let Some(id) = &args.id {
        let build = config.build(&BuildCall::new(id), build_type)?;

        if !build.is_enabled(build_type) {
            bail!(
                "build '{}' is disabled for {} build type",
                id,
                build_type.as_lowercase()
            );
        }

        let _ = run(&build, config, astra, build_type)?;
    } else {
        for build in builds {
            let build = build?;

            if !build.is_enabled(build_type) {
                continue;
            }

            let _ = run(&build, config, astra, build_type)?;
        }
    }

    Ok(())
}

pub(crate) fn run(build: &Build, config: &Config, astra: &Astra, build_type: BuildType) -> anyhow::Result<BuildOutput> {
    tracing::info!("Building '{}'", build.id());

    let build_dir = Path::new(astra.build_dir()).join(build.id());
    std::fs::create_dir_all(&build_dir)
        .with_context(|| format!("failed to create directory '{}'", build_dir.display()))?;

    for call in build.depends() {
        tracing::debug!("calling dependency '{}'", call.id());

        crate::task::run(call, config, false)?;
    }

    let mut result = target::build(&build_dir, build, build_type);

    for call in build.finally() {
        tracing::debug!("calling finally task '{}'", call.id());

        if let Err(err) = crate::task::run(call, config, false) {
            if result.is_err() {
                tracing::error!("finally task '{}' failed: {err:#}", call.id());
            } else {
                result = Err(err.context(format!("finally task '{}' failed", call.id())));
            }
        }
    }

    let mut artifacts = result?;

    for pattern in build.artifacts() {
        let (prefix, glob) = crate::fs::resolve_glob(pattern)?;
        for entry in glob.walk(prefix) {
            artifacts.push(std::path::absolute(entry?.path())?);
        }
    }

    artifacts.sort_unstable();
    artifacts.dedup();

    Ok(BuildOutput {
        hash: hash_artifacts(&artifacts, build_type)?,
        artifacts,
    })
}

fn hash_artifacts(artifacts: &[PathBuf], build_type: BuildType) -> anyhow::Result<u128> {
    let mut hash = xxhash_rust::xxh3::Xxh3::new();
    hash.update(build_type.as_lowercase().as_bytes());

    for path in artifacts {
        if path.is_file() {
            hash_file(path, &mut hash)?;
        } else if path.is_dir() {
            hash_directory(path, &mut hash)?;
        } else {
            bail!("artifact '{}' is not a regular file or directory", path.display());
        }
    }

    Ok(hash.digest128())
}

fn hash_directory(dir: &Path, hash: &mut xxhash_rust::xxh3::Xxh3) -> anyhow::Result<()> {
    let path = dir.to_string_lossy();
    hash.update(&(path.len() as u64).to_le_bytes());
    hash.update(path.as_bytes());

    let mut entries = std::fs::read_dir(dir)
        .with_context(|| format!("failed to read artifact directory '{}'", dir.display()))?
        .map(|entry| {
            let entry = entry?;
            Ok((entry.file_name(), entry.file_type()?))
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    entries.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));

    for (name, kind) in entries {
        let path = dir.join(name);

        if kind.is_dir() {
            hash_directory(&path, hash)?;
        } else if kind.is_file() {
            hash_file(&path, hash)?;
        } else {
            bail!("artifact '{}' contains a non-regular file", path.display());
        }
    }

    Ok(())
}

fn hash_file(path: &Path, hash: &mut xxhash_rust::xxh3::Xxh3) -> anyhow::Result<()> {
    let path_str = path.to_string_lossy();
    hash.update(&(path_str.len() as u64).to_le_bytes());
    hash.update(path_str.as_bytes());
    hash.update(&crate::fs::hash_file(path)?.to_le_bytes());

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
