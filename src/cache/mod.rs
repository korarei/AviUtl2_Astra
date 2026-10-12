use crate::config::Config;
use std::path::Path;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum Command {
    /// Delete all cached package data.
    ///
    /// Removes the entire package cache directory including blobs and extracted trees.
    Clean,

    /// Collect and remove unused package cache entries.
    ///
    /// Deletes cached packages that are not referenced by the current runtime.
    Gc,

    /// Refresh package URL cache by re-downloading packages.
    ///
    /// Re-downloads all currently cached package URLs and updates cache entries.
    Refresh,
}

pub(super) fn command(_config: &Config, args: &Args) -> anyhow::Result<()> {
    crate::fs::create_managed_dir(Path::new(".astra"))?;

    let mut lock = crate::fs::Lock::new(Path::new(".astra"))?;
    let _guard = lock.write()?;

    let mut cache = crate::package::PackageCache::new(false)?;

    match &args.command {
        Command::Clean => {
            if !cache.root().is_dir() {
                tracing::info!("No package cache found");
                return Ok(());
            }

            cache.clean()?;
            tracing::info!("Removed package cache '{}'", cache.root().display());
            Ok(())
        }
        Command::Gc => {
            if !cache.root().is_dir() {
                tracing::info!("No unused cache entries found");
                return Ok(());
            }

            let manifest = crate::run::RuntimeManifest::read(Path::new(crate::run::RUNTIME_DIR))?;
            let removed = cache.collect(manifest.aviutl2.links.values())?;

            if removed == 0 {
                tracing::info!("No unused cache entries found");
            } else if removed == 1 {
                tracing::info!("Removed 1 unused cache entry");
            } else {
                tracing::info!("Removed {removed} unused cache entries");
            }

            Ok(())
        }
        Command::Refresh => {
            if !cache.root().is_dir() {
                tracing::info!("No package cache found");
                return Ok(());
            }

            let refreshed = cache.refresh()?;

            if refreshed == 0 {
                tracing::info!("No cached URLs found");
            } else if refreshed == 1 {
                tracing::info!("Refreshed 1 package cache entry");
            } else {
                tracing::info!("Refreshed {refreshed} package cache entries");
            }

            Ok(())
        }
    }
}
