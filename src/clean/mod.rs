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

    let mut build_lock = crate::fs::Lock::new(build_dir)?;
    let _build_guard = build_lock.write()?;
    let mut dist_lock = crate::fs::Lock::new(dist_dir)?;
    let _dist_guard = dist_lock.write()?;
    let mut astra_lock = crate::fs::Lock::new(Path::new(".astra"))?;
    let _astra_guard = astra_lock.write()?;

    for dir in config.build_dirs() {
        crate::fs::remove(&dir)?;
    }

    for dir in config.release_dirs() {
        crate::fs::remove(&dir)?;
    }

    crate::fs::remove(Path::new(crate::package::CACHE_DIR))?;

    Ok(())
}
