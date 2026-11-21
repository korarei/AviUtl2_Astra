use crate::config::Config;
use std::path::Path;

pub(super) fn command(config: &Config) -> anyhow::Result<()> {
    tracing::info!("Cleaning build and dist directories");

    let astra = config.astra();
    let build_dir = Path::new(astra.build_dir());
    let dist_dir = Path::new(astra.dist_dir());

    crate::fs::create_managed_dir(build_dir)?;
    crate::fs::create_managed_dir(dist_dir)?;
    crate::fs::create_managed_dir(Path::new(".astra"))?;

    let _build_lock = crate::fs::lock(build_dir, "build directory")?;
    let _dist_lock = crate::fs::lock(dist_dir, "dist directory")?;
    let _astra_lock = crate::fs::lock(Path::new(".astra"), ".astra directory")?;

    for dir in config.build_dirs() {
        crate::fs::remove(&dir)?;
    }

    for dir in config.release_dirs() {
        crate::fs::remove(&dir)?;
    }

    crate::fs::remove(Path::new(crate::package::CACHE_DIR))?;

    Ok(())
}
