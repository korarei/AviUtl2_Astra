use crate::config::{BuildType, Config, TaskCall};
use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

mod deploy;
mod runtime;

pub(crate) const RUNTIME_DIR: &str = ".astra/runtime";

#[cfg(windows)]
mod process;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RuntimeManifest {
    #[serde(skip)]
    path: PathBuf,
    #[serde(default)]
    config: Option<u128>,
    #[serde(default)]
    build: Option<u128>,
    #[serde(default)]
    pub(crate) aviutl2: AviUtl2Manifest,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AviUtl2Manifest {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    package: Option<u128>,
    #[serde(default)]
    pub(crate) links: BTreeMap<String, String>,
}

impl RuntimeManifest {
    pub(crate) fn read(dir: &Path) -> anyhow::Result<Self> {
        let path = dir.join("manifest.json");
        let mut manifest = match std::fs::metadata(&path) {
            Ok(meta) if !meta.is_file() => {
                bail!("runtime manifest is not a regular file: '{}'", path.display());
            }
            Ok(_) => match serde_json::from_str::<Self>(
                &std::fs::read_to_string(&path).with_context(|| format!("failed to read '{}'", path.display()))?,
            ) {
                Ok(manifest) => manifest,
                Err(err) => {
                    tracing::warn!(
                        "invalid runtime manifest '{}': {err}; it will be rebuilt",
                        path.display()
                    );
                    Self::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(err.into()),
        };

        let mut invalid = false;
        manifest.aviutl2.links.retain(|dst, _| {
            let mut normal = false;
            let valid = !dst.contains('\\')
                && Path::new(dst).components().all(|component| match component {
                    Component::Normal(_) => {
                        normal = true;
                        true
                    }
                    Component::CurDir => true,
                    _ => false,
                })
                && normal;

            if !valid {
                tracing::warn!("invalid link path '{dst}' in runtime manifest; it will be redeployed");
                invalid = true;
            }

            valid
        });

        if invalid {
            manifest.config = None;
            manifest.build = None;
            manifest.aviutl2.package = None;
        }

        manifest.path = path;
        Ok(manifest)
    }

    fn write(&self) -> anyhow::Result<()> {
        crate::fs::write_file(&self.path, serde_json::to_string_pretty(self)?.as_bytes())
    }
}

#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Args {
    /// Name of the task to execute.
    ///
    /// If specified, runs the designated task from astra.toml instead of launching `AviUtl ExEdit2`.
    #[arg(value_name = "TASK")]
    task: Option<String>,

    /// Launch `AviUtl ExEdit2` with the specified version.
    ///
    /// Can be specified without a value to use the latest version.
    #[arg(
        long,
        value_name = "VERSION",
        num_args = 0..=1,
        default_missing_value = "latest",
        conflicts_with = "task"
    )]
    aviutl2: Option<String>,

    /// Use release build artifacts when running `AviUtl ExEdit2`.
    #[arg(long, conflicts_with = "debug", conflicts_with = "task")]
    release: bool,

    /// Use debug build artifacts when running `AviUtl ExEdit2` (default).
    #[arg(long, conflicts_with = "release", conflicts_with = "task")]
    debug: bool,

    /// パッケージのURLキャッシュを再ダウンロードする。
    #[arg(long = "refresh", conflicts_with = "task")]
    is_refresh: bool,
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

pub(super) fn run(config: &Config, args: &Args) -> anyhow::Result<()> {
    if let Some(id) = &args.task {
        crate::task::run(&TaskCall::Simple(id.clone()), config, true)?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        bail!("running AviUtl ExEdit2 is only supported on Windows");
    }

    #[cfg(windows)]
    run_aviutl2(config, args)
}

#[cfg(windows)]
use crate::config::{Package, Release, ReleaseDependency};

#[cfg(windows)]
#[allow(clippy::too_many_lines)]
fn run_aviutl2(config: &Config, args: &Args) -> anyhow::Result<()> {
    use fd_lock::RwLock;
    use process::MonitorExit;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[allow(non_camel_case_types)]
    #[allow(clippy::upper_case_acronyms)]
    type BOOL = i32;
    #[allow(non_camel_case_types)]
    #[allow(clippy::upper_case_acronyms)]
    type DWORD = u32;
    #[allow(non_camel_case_types)]
    type PHANDLER_ROUTINE = Option<unsafe extern "system" fn(DWORD) -> BOOL>;

    const TRUE: BOOL = 1;
    const FALSE: BOOL = 0;
    const CTRL_C_EVENT: DWORD = 0;
    const CTRL_BREAK_EVENT: DWORD = 1;

    static IS_RUNNING: AtomicBool = AtomicBool::new(true);

    unsafe extern "system" fn handle_console_ctrl(ctrl_type: DWORD) -> BOOL {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT => {
                IS_RUNNING.store(false, Ordering::SeqCst);
                TRUE
            }
            _ => FALSE,
        }
    }

    unsafe extern "system" {
        fn SetConsoleCtrlHandler(HandlerRoutine: PHANDLER_ROUTINE, Add: BOOL) -> BOOL;
    }

    struct CtrlHandler;
    impl CtrlHandler {
        fn install() -> std::io::Result<Self> {
            let result = unsafe { SetConsoleCtrlHandler(PHANDLER_ROUTINE::Some(handle_console_ctrl), TRUE) };
            if result == FALSE {
                return Err(std::io::Error::last_os_error());
            }

            Ok(Self)
        }
    }

    impl Drop for CtrlHandler {
        fn drop(&mut self) {
            let result = unsafe { SetConsoleCtrlHandler(PHANDLER_ROUTINE::Some(handle_console_ctrl), FALSE) };
            if result == FALSE {
                tracing::warn!(
                    "failed to unregister console control handler: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
    }

    IS_RUNNING.store(true, Ordering::SeqCst);

    let _ctrl_guard = CtrlHandler::install()?;

    let astra = config.astra();
    let build_dir = Path::new(astra.build_dir());
    let runtime_dir = Path::new(RUNTIME_DIR);
    let aviutl2_dir = runtime_dir.join("aviutl2");
    let aviutl2_exe = aviutl2_dir.join("aviutl2.exe");
    let data_dir = aviutl2_dir.join("data");

    crate::fs::create_managed_dir(build_dir)?;
    crate::fs::create_managed_dir(Path::new(astra.dist_dir()))?;
    crate::fs::create_managed_dir(Path::new(".astra"))?;
    std::fs::create_dir_all(runtime_dir)
        .with_context(|| format!("failed to create directory '{}'", runtime_dir.display()))?;

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

    let release = select_release(config)?;

    let mut manifest = RuntimeManifest::read(runtime_dir)?;

    process::shutdown_aviutl2(&aviutl2_exe)?;

    let is_setup = aviutl2_exe.is_file() && manifest.aviutl2.version != 0;
    if let Some(ver) = args.aviutl2.as_deref() {
        let ver_num = crate::aviutl2_version::resolve(ver)?;
        if !is_setup || manifest.aviutl2.version != ver_num {
            tracing::info!("Installing AviUtl ExEdit2 ({ver})");

            runtime::setup(ver_num, config)?;
            manifest.aviutl2.version = ver_num;
        }
    } else if !is_setup {
        let ver = crate::aviutl2_version::resolve("latest")?;

        tracing::info!("Installing AviUtl ExEdit2 (latest)");

        runtime::setup(ver, config)?;
        manifest.aviutl2.version = ver;
    }

    let mut config = config.clone();
    let mut config_hash = crate::fs::hash_file(config.path()).ok();
    let build_type = args.build_type();

    sync_package(&config, build_type, &release, &data_dir, &mut manifest, args.is_refresh)?;

    loop {
        if !IS_RUNNING.load(Ordering::SeqCst) {
            break;
        }

        tracing::info!("Running AviUtl ExEdit2 ({})", aviutl2_exe.display());

        match process::run_process(&aviutl2_exe, &IS_RUNNING)? {
            MonitorExit::ProcessExited(code) => {
                if code != 0 {
                    tracing::warn!("aviutl exedit2 exited with code: {code:#010x}");
                }

                tracing::info!("AviUtl ExEdit2 exited");
                break;
            }
            MonitorExit::Interrupted => {
                break;
            }
            MonitorExit::ReloadRequested => loop {
                if !IS_RUNNING.load(Ordering::SeqCst) {
                    return Ok(());
                }

                if let Ok(hash) = crate::fs::hash_file(config.path())
                    && Some(hash) != config_hash
                {
                    match config.reload() {
                        Ok(new_config) => {
                            config = new_config;
                            config_hash = Some(hash);
                            tracing::info!("Configuration reloaded");
                        }
                        Err(e) => {
                            tracing::error!("failed to reload configuration: {e:#}");
                            tracing::info!("Fix the configuration and press 'Ctrl+R' or 'r' to retry reload");
                            if process::wait_for_reload(&IS_RUNNING) == MonitorExit::ReloadRequested {
                                continue;
                            }
                            return Ok(());
                        }
                    }
                }

                let release = match select_release(&config) {
                    Ok(r) => r,
                    Err(e) => {
                        tracing::error!("{e:#}");
                        tracing::info!("Fix the configuration and press 'Ctrl+R' or 'r' to retry reload");
                        if process::wait_for_reload(&IS_RUNNING) == MonitorExit::ReloadRequested {
                            continue;
                        }
                        return Ok(());
                    }
                };

                match sync_package(&config, build_type, &release, &data_dir, &mut manifest, false) {
                    Ok(()) => break,
                    Err(e) => {
                        tracing::error!("{e:#}");
                        tracing::info!("Fix the build error and press 'Ctrl+R' or 'r' to retry reload");
                        if process::wait_for_reload(&IS_RUNNING) == MonitorExit::ReloadRequested {
                            continue;
                        }
                        return Ok(());
                    }
                }
            },
        }
    }

    Ok(())
}

#[cfg(windows)]
fn select_release(config: &Config) -> anyhow::Result<Release> {
    let id = config
        .astra()
        .run()
        .release_id()
        .ok_or_else(|| anyhow::anyhow!("astra.run.release is not configured"))?;
    let release = config.release(id)?;
    if !release.package().is_some_and(Package::is_au2pkg) {
        bail!("release '{id}' does not use an au2pkg package: 'astra run' requires an au2pkg package");
    }

    Ok(release)
}

#[cfg(windows)]
fn sync_package(
    config: &Config,
    build_type: BuildType,
    release: &Release,
    data_dir: &Path,
    manifest: &mut RuntimeManifest,
    is_refresh: bool,
) -> anyhow::Result<()> {
    let package = release
        .package()
        .ok_or_else(|| anyhow::anyhow!("release '{}' has no package", release.id()))?;
    let config_hash =
        xxhash_rust::const_xxh3::xxh3_128(&serde_json::to_vec(&(config, release, build_type.as_lowercase()))?);

    let mut outputs = BTreeMap::new();

    for dep in release.depends() {
        match dep {
            ReleaseDependency::Simple(_) => unreachable!("release dependency must be expanded"),
            ReleaseDependency::Build(call) => {
                let id = call.id();
                if outputs.contains_key(id) {
                    continue;
                }

                let build = config.build(id, build_type)?;
                if build.is_enabled(build_type) {
                    tracing::info!("Building dependency '{}' for release '{}'", id, release.id());
                    outputs.insert(
                        id.to_owned(),
                        crate::build::run(&build, config, config.astra(), build_type)?,
                    );
                } else {
                    tracing::warn!(
                        "build '{}' is disabled for {} build type",
                        id,
                        build_type.as_lowercase()
                    );
                }
            }
            ReleaseDependency::Task(call) => {
                tracing::debug!("calling dependency '{}'", call.id());
                crate::task::run(call, config, false)?;
            }
        }
    }

    let build_hash = {
        let mut hash = xxhash_rust::xxh3::Xxh3::new();

        for (id, output) in &outputs {
            hash.update(&(id.len() as u64).to_le_bytes());
            hash.update(id.as_bytes());
            hash.update(&output.hash.to_le_bytes());
        }

        Some(hash.digest128())
    };

    let mut cache = crate::package::PackageCache::new(is_refresh)?;
    let mut package_hash = if is_refresh {
        None
    } else {
        cache.hash(package.contents())?
    };

    let should_deploy = is_refresh
        || manifest.config != Some(config_hash)
        || manifest.build != build_hash
        || package_hash.is_none()
        || manifest.aviutl2.package != package_hash
        || !manifest.aviutl2.links.iter().all(|(dst, target)| {
            let target = Path::new(target);
            let target = if target.is_absolute() {
                target.to_path_buf()
            } else {
                cache.root().join(target)
            };

            let path = data_dir.join(dst);
            path.exists() && deploy::is_same_link(&path, &target)
        });

    let mut err = if should_deploy {
        match deploy::deploy(data_dir, package, &outputs, manifest, &mut cache) {
            Ok(hash) => {
                package_hash = hash;
                None
            }
            Err(err) => Some(err.context("package deployment failed; rerun 'astra run' to deploy again")),
        }
    } else {
        None
    };

    for call in release.finally() {
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

    if should_deploy {
        manifest.config = Some(config_hash);
        manifest.build = build_hash;
        manifest.aviutl2.package = package_hash;
        if let Err(err) = manifest.write() {
            manifest.config = None;
            return Err(err);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::RuntimeManifest;
    use std::collections::BTreeMap;

    #[test]
    fn reads_and_writes_manifest() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let mut manifest = RuntimeManifest::read(dir.path())?;
        assert_eq!(manifest.aviutl2.version, 0);
        assert!(manifest.aviutl2.links.is_empty());

        manifest.aviutl2.version = 2_005_400;
        manifest.config = Some(1);
        manifest.build = Some(2);
        manifest.aviutl2.links = BTreeMap::from([(String::from("Script/main.lua"), String::from("blob"))]);
        manifest.write()?;
        assert_eq!(RuntimeManifest::read(dir.path())?, manifest);

        Ok(())
    }
}
