use crate::config::Config;
use anyhow::Context;
use fd_lock::RwLock;
use std::path::Path;

pub(super) fn command(config: &Config) -> anyhow::Result<()> {
    tracing::info!("Cleaning build and dist directories");

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

    for dir in config.build_dirs() {
        crate::fs::remove(&dir)?;
    }

    for dir in config.release_dirs() {
        crate::fs::remove(&dir)?;
    }

    crate::fs::remove(Path::new(crate::package::CACHE_DIR))?;

    Ok(())
}
