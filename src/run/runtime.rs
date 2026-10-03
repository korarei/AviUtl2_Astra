use crate::aviutl2_version::FIRST_STABLE_NUM;
use crate::{config::Config, http};
use anyhow::{Context, bail};
use std::io::{Seek, SeekFrom};
use std::path::{Path, PathBuf};

pub(super) fn setup(ver: u32, config: &Config) -> anyhow::Result<()> {
    if let Some(req) = config.project().requires_aviutl2()
        && ver < req
    {
        tracing::warn!("specified AviUtl ExEdit2 version ({ver}) is lower than required version in config ({req})");
    }

    let runtime_dir = Path::new(super::RUNTIME_DIR);
    crate::fs::create_managed_dir(Path::new(".astra"))?;
    std::fs::create_dir_all(runtime_dir)
        .with_context(|| format!("failed to create directory '{}'", runtime_dir.display()))?;

    let mut manifest = super::RuntimeManifest::read(runtime_dir)?;
    manifest.aviutl2.version = ver;

    let staging = fetch_staging(ver, runtime_dir)?;
    let temp_aviutl2_dir = staging.path();

    let aviutl2_dir = runtime_dir.join("aviutl2");
    let data_dir = aviutl2_dir.join("data");

    let has_data = match std::fs::metadata(&data_dir) {
        Ok(meta) if meta.is_dir() => true,
        Ok(_) => bail!("data path is not a directory: '{}'", data_dir.display()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
        Err(err) => return Err(err).with_context(|| format!("failed to inspect '{}'", data_dir.display())),
    };

    let temp_data_dir = if has_data {
        let dir = temp_aviutl2_dir.join("data");
        crate::fs::remove(&dir)?;
        dir
    } else {
        init_default_config(temp_aviutl2_dir)?
    };

    let backup = if aviutl2_dir.try_exists()? {
        Some(
            tempfile::tempdir_in(runtime_dir)
                .with_context(|| format!("failed to create temporary directory in '{}'", runtime_dir.display()))?,
        )
    } else {
        None
    };
    let backup_dir = backup.as_ref().map(|dir| dir.path().join("aviutl2"));

    let mut rollbacks: Vec<(PathBuf, PathBuf)> = Vec::new();
    if let Err(err) = (|| -> anyhow::Result<()> {
        if let Some(dir) = &backup_dir {
            std::fs::rename(&aviutl2_dir, dir)
                .with_context(|| format!("failed to move '{}' to '{}'", aviutl2_dir.display(), dir.display()))?;
            rollbacks.push((dir.clone(), aviutl2_dir.clone()));
        }

        if has_data {
            let backup_data = backup_dir
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("previous environment disappeared: '{}'", aviutl2_dir.display()))?
                .join("data");
            std::fs::rename(&backup_data, &temp_data_dir).with_context(|| {
                format!(
                    "failed to move '{}' to '{}'",
                    backup_data.display(),
                    temp_data_dir.display()
                )
            })?;
            rollbacks.push((temp_data_dir.clone(), backup_data));
        }

        std::fs::rename(staging.path(), &aviutl2_dir).with_context(|| {
            format!(
                "failed to move '{}' to '{}'",
                staging.path().display(),
                aviutl2_dir.display()
            )
        })?;

        rollbacks.push((aviutl2_dir.clone(), staging.path().to_path_buf()));

        manifest.write()
    })() {
        let restore_err = rollbacks
            .into_iter()
            .rev()
            .try_for_each(|(from, to)| {
                std::fs::rename(&from, &to)
                    .with_context(|| format!("failed to restore '{}' to '{}'", from.display(), to.display()))
            })
            .err();
        if let Some(restore) = restore_err {
            return Err(err.context(format!(
                "failed to restore environment: {restore:#}; files retained at '{}', '{}' and '{}'",
                staging.keep().display(),
                backup
                    .map(tempfile::TempDir::keep)
                    .as_deref()
                    .unwrap_or(runtime_dir)
                    .display(),
                aviutl2_dir.display()
            )));
        }
        return Err(err);
    }

    if let Some(backup) = backup {
        backup.close().with_context(|| {
            format!(
                "failed to remove previous environment '{}'",
                backup_dir.as_deref().unwrap_or(runtime_dir).display()
            )
        })?;
    }

    Ok(())
}

fn fetch_staging(ver: u32, runtime_dir: &Path) -> anyhow::Result<tempfile::TempDir> {
    const BASE_URL: &str = "https://spring-fragrance.mints.ne.jp/aviutl/";
    const MAX_ARCHIVE_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;

    let url = format!("{BASE_URL}{}", format_zip_filename(ver)?);

    let mut archive = http::to_temp_file(
        runtime_dir,
        http::client()?.get(&url).send()?.error_for_status()?,
        &url,
        MAX_ARCHIVE_DOWNLOAD_BYTES,
    )?;

    archive.as_file_mut().seek(SeekFrom::Start(0))?;

    let mut reader = zip::ZipArchive::new(archive.as_file_mut())
        .with_context(|| format!("failed to read zip archive from '{url}'"))?;

    let staging = tempfile::tempdir_in(runtime_dir)
        .with_context(|| format!("failed to create temporary directory in '{}'", runtime_dir.display()))?;

    reader.extract(staging.path()).with_context(|| {
        format!(
            "failed to extract zip archive from '{url}' to '{}'",
            staging.path().display()
        )
    })?;

    if !staging.path().join("aviutl2.exe").is_file() {
        bail!("zip archive from '{url}' does not contain aviutl2.exe");
    }

    Ok(staging)
}

fn init_default_config(temp_aviutl2_dir: &Path) -> anyhow::Result<PathBuf> {
    const CONFIG_PREFIX: &str = include_str!("aviutl2.ini");
    const LAYOUT: &str = include_str!("layout.ini");

    let temp_data_dir = temp_aviutl2_dir.join("data");
    let default_dir = temp_data_dir.join("Default");
    std::fs::create_dir_all(&default_dir)
        .with_context(|| format!("failed to create directory '{}'", default_dir.display()))?;
    crate::fs::write_file(
        &temp_data_dir.join("aviutl2.ini"),
        format!("{CONFIG_PREFIX}{LAYOUT}").as_bytes(),
    )?;
    crate::fs::write_file(&default_dir.join("Debug.layout"), LAYOUT.as_bytes())?;

    Ok(temp_data_dir)
}

fn format_zip_filename(ver: u32) -> anyhow::Result<String> {
    let (major, rem) = (ver / 1_000_000, ver % 1_000_000);
    let (minor, rem) = (rem / 10_000, rem % 10_000);
    let (patch, build) = (rem / 100, rem % 100);

    let build = if build > 0 {
        if let Ok(offset) = u8::try_from(build - 1)
            && offset < 26
        {
            char::from(b'a' + offset).to_string()
        } else {
            bail!("invalid AviUtl ExEdit2 build number in version '{ver}'");
        }
    } else {
        String::new()
    };

    if ver < FIRST_STABLE_NUM {
        Ok(format!("aviutl2beta{patch}{build}.zip"))
    } else {
        Ok(format!("aviutl2_v{major}.{minor}.{patch}{build}.zip"))
    }
}

#[cfg(test)]
mod tests {
    use super::format_zip_filename;

    #[test]
    fn formats_beta_and_stable_versions() -> anyhow::Result<()> {
        for (ver, expected) in [
            (2_005_300, "aviutl2beta53.zip"),
            (2_005_301, "aviutl2beta53a.zip"),
            (2_005_400, "aviutl2_v2.0.54.zip"),
            (2_005_401, "aviutl2_v2.0.54a.zip"),
        ] {
            assert_eq!(format_zip_filename(ver)?, expected);
        }

        assert!(format_zip_filename(2_005_427).is_err());
        Ok(())
    }
}
