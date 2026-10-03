mod stage;

use crate::build::BuildOutput;
use crate::config::{Astra, BuildType, Config, Release, ReleaseDependency};
use anyhow::{Context, bail};
use fd_lock::RwLock;
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Args {
    /// ID of the specific release configuration to execute.
    ///
    /// If omitted, all configured releases will be executed.
    #[arg(value_name = "ID")]
    id: Option<String>,
}

pub(super) fn command(config: &Config, args: &Args) -> anyhow::Result<()> {
    tracing::info!("Running release");

    let releases = config.releases();

    if releases.len() == 0 {
        bail!("no release configurations found");
    }

    let astra = config.astra();

    let build_dir = Path::new(astra.build_dir());
    crate::fs::create_managed_dir(build_dir)?;
    crate::fs::create_managed_dir(Path::new(astra.dist_dir()))?;

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

    let mut outputs: BTreeMap<String, BuildOutput> = BTreeMap::new();

    if let Some(id) = &args.id {
        let release = config.release(id)?;
        run(&release, config, astra, &mut outputs)?;
    } else {
        for release in releases {
            run(&release?, config, astra, &mut outputs)?;
        }
    }

    Ok(())
}

fn run(
    release: &Release,
    config: &Config,
    astra: &Astra,
    outputs: &mut BTreeMap<String, BuildOutput>,
) -> anyhow::Result<()> {
    tracing::info!("Releasing '{}'", release.id());

    let dist_dir = Path::new(astra.dist_dir()).join(release.id());

    crate::fs::remove(&dist_dir)?;
    std::fs::create_dir_all(&dist_dir)
        .with_context(|| format!("failed to create directory '{}'", dist_dir.display()))?;

    for dep in release.depends() {
        match dep {
            ReleaseDependency::Simple(_) => unreachable!("release dependency must be expanded"),
            ReleaseDependency::Build(b) => {
                let id = b.id();
                if !outputs.contains_key(id) {
                    let build = config.build(id, BuildType::Release)?;
                    if build.is_enabled(BuildType::Release) {
                        tracing::info!("Building dependency '{}' for release '{}'", id, release.id());
                        outputs.insert(
                            id.to_string(),
                            crate::build::run(&build, config, astra, BuildType::Release)?,
                        );
                    } else {
                        tracing::warn!("build '{}' is disabled for release build type", id);
                    }
                }
            }
            ReleaseDependency::Task(call) => {
                tracing::debug!("calling dependency '{}'", call.id());
                crate::task::run(call, config, false)?;
            }
        }
    }

    let mut err = (|| -> anyhow::Result<()> {
        if let Some(package) = release.package() {
            let file = package.filename();
            let is_au2pkg = package.is_au2pkg();
            let staging = stage::stage(
                Some(&dist_dir),
                &mut crate::package::PackageCache::new(true)?,
                package.contents(),
                outputs,
                is_au2pkg,
            )?;
            let temp_dir = staging.path();

            if is_au2pkg {
                std::fs::write(temp_dir.join("package.ini"), package.to_manifest())
                    .with_context(|| format!("failed to write '{}'", temp_dir.join("package.ini").display()))?;
                std::fs::write(temp_dir.join("package.txt"), package.to_description())
                    .with_context(|| format!("failed to write '{}'", temp_dir.join("package.txt").display()))?;
            }

            let archive_file = dist_dir.join(file);
            tracing::info!("Packaging '{}'", archive_file.display());
            make_zip_archive(&archive_file, temp_dir)?;
        }

        if let Some(notes) = release.notes() {
            create_release_notes(notes, &dist_dir)?;
        }

        Ok(())
    })()
    .err();

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

    Ok(())
}

fn create_release_notes(notes: &crate::config::ReleaseNotes, release_dir: &Path) -> anyhow::Result<()> {
    let changelog_file = Path::new(notes.changelog());

    if !changelog_file.is_file() {
        bail!("changelog file '{}' not found", changelog_file.display());
    }

    let bytes =
        std::fs::read(changelog_file).with_context(|| format!("failed to read '{}'", changelog_file.display()))?;
    let (decoded, _, has_malformed) = notes.encoding().decode(&bytes);
    if has_malformed {
        bail!(
            "failed to decode changelog '{}' as {}: invalid byte sequence",
            changelog_file.display(),
            notes.encoding().name()
        );
    }

    if let Some(content) = extract_release_notes(&decoded) {
        let output_file = release_dir.join(notes.filename());
        tracing::info!("Writing release notes to '{}'", output_file.display());
        crate::fs::write_file(&output_file, content.as_bytes())?;
    } else {
        tracing::warn!("no changelog section found in '{}'", changelog_file.display());
    }

    Ok(())
}

fn extract_release_notes(text: &str) -> Option<String> {
    static CHANGELOG_HEADER_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im)^[^\S\n]*(?P<hashes>#+)[^\S\n]*(?:change[^\S\n]*logs?\b|更新履歴|改版履歴)").unwrap()
    });

    static VERSION_HEADER_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^[^\S\n]*(?:#+[^\S\n]*(?:\*\*)?|[-*][^\S\n]*\*\*)\[?v?\d+").unwrap());

    static HEADING_PATTERN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(?P<indent>[^\S\n]*)(?P<hashes>#+)(?P<rest>[^\S\n].*|$)").unwrap());

    let src: Cow<'_, str> = if text.contains('\r') {
        Cow::Owned(text.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(text)
    };

    let caps = CHANGELOG_HEADER_PATTERN.captures(&src)?;
    let changelog_level = caps.name("hashes")?.as_str().len();

    let mut it = src[caps.get(0)?.end()..]
        .lines()
        .take_while(|line| {
            let level = line.trim_start().chars().take_while(|c| *c == '#').count();
            !(level > 0 && level <= changelog_level)
        })
        .skip_while(|line| !VERSION_HEADER_PATTERN.is_match(line));

    it.next()?;

    let lines: Vec<&str> = it.take_while(|line| !VERSION_HEADER_PATTERN.is_match(line)).collect();
    let dedented = textwrap::dedent(&lines.join("\n"));

    let root_level = HEADING_PATTERN
        .captures_iter(&dedented)
        .filter_map(|c| c.name("hashes").map(|m| m.as_str().len()))
        .min();

    let changes = if let Some(root_level) = root_level {
        HEADING_PATTERN
            .replace_all(&dedented, |caps: &Captures| {
                let indent = caps.name("indent").map_or("", |m| m.as_str());
                let hashes = caps.name("hashes").map_or("", |m| m.as_str());
                let rest = caps.name("rest").map_or("", |m| m.as_str());
                format!("{indent}{}{rest}", "#".repeat(hashes.len() + 3 - root_level))
            })
            .into_owned()
    } else {
        dedented
    };

    Some(format!("## What's Changed\n\n{}\n", changes.trim()))
}

fn make_zip_archive(dst_file: &Path, root_dir: &Path) -> anyhow::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(
        dst_file
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .with_context(|| format!("failed to create temporary file for '{}'", dst_file.display()))?;
    let mut zip = zip::ZipWriter::new(file.as_file_mut());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    add_dir_to_zip(&mut zip, options, root_dir, Path::new(""))
        .with_context(|| format!("failed to write zip archive '{}'", dst_file.display()))?;

    let _ = zip
        .finish()
        .with_context(|| format!("failed to finish zip archive '{}'", dst_file.display()))?;
    let _ = file
        .persist(dst_file)
        .with_context(|| format!("failed to persist '{}'", dst_file.display()))?;

    Ok(())
}

fn add_dir_to_zip(
    zip: &mut zip::ZipWriter<&mut std::fs::File>,
    options: zip::write::SimpleFileOptions,
    root_dir: &Path,
    curr_dir: &Path,
) -> anyhow::Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(root_dir.join(curr_dir))?.collect::<Result<_, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);

    for entry in entries {
        let file = curr_dir.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            zip.add_directory_from_path(&file, options)
                .with_context(|| format!("failed to add directory '{}' to zip archive", file.display()))?;
            add_dir_to_zip(zip, options, root_dir, &file)?;
        } else if file_type.is_file() {
            zip.start_file_from_path(&file, options)
                .with_context(|| format!("failed to start file '{}' in zip archive", file.display()))?;
            let file = root_dir.join(&file);
            let mut src = std::fs::File::open(&file).with_context(|| format!("failed to read '{}'", file.display()))?;
            std::io::copy(&mut src, zip)
                .with_context(|| format!("failed to copy '{}' into zip archive", file.display()))?;
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_release_notes() {
        for (source, expected) in [
            (
                "## Changelog\n### v1.0.0\n- Added\n- Fixed\n\n### v0.9.0\n- Old\n",
                "## What's Changed\n\n- Added\n- Fixed\n",
            ),
            (
                "#changelog\n- **1.0.0**\n  - Item\n- **0.9.0**\n  - Old\n",
                "## What's Changed\n\n- Item\n",
            ),
            (
                "# Changelog\n## [2] 2026-09-06\n- Major\n## [1] 2025-01-01\n- Old\n",
                "## What's Changed\n\n- Major\n",
            ),
            (
                "## Change Log\n### [2.0.0] - 2026-09-06\n#### Added\n- Great\n#### Fixed\n- Bug\n",
                "## What's Changed\n\n### Added\n- Great\n### Fixed\n- Bug\n",
            ),
            (
                "## Changelog\n### 1.0.0\n- Single\n\n## License\nMIT\n",
                "## What's Changed\n\n- Single\n",
            ),
            (
                "## Changelog\r\n### v1.0.0\r\n- CRLF\r\n",
                "## What's Changed\n\n- CRLF\n",
            ),
            ("## Changelog\r### v1.0.0\r- CR\r", "## What's Changed\n\n- CR\n"),
        ] {
            assert_eq!(extract_release_notes(source).as_deref(), Some(expected), "{source:?}");
        }
        assert!(extract_release_notes("## Changelog\n## [2] 2026\n- Same level\n").is_none());
        assert!(extract_release_notes("# Project\n## Installation\nUse it\n").is_none());
    }

    #[test]
    fn creates_release_notes_and_archives() {
        let temp = tempfile::tempdir().unwrap();
        let changelog = temp.path().join("CHANGELOG.md");
        let text = "## Changelog\n### v1.0.0\n- Added\n";
        let (bytes, _, _) = encoding_rs::SHIFT_JIS.encode(text);
        std::fs::write(&changelog, bytes).unwrap();

        let json = format!(
            r#"{{"changelog":"{}","filename":"NOTES.md","encoding":"shift_jis"}}"#,
            changelog.display().to_string().replace('\\', "\\\\")
        );
        let notes: crate::config::ReleaseNotes = serde_json::from_str(&json).unwrap();
        create_release_notes(&notes, temp.path()).unwrap();
        assert!(temp.path().join("NOTES.md").is_file());
        assert!(
            std::fs::read_to_string(temp.path().join("NOTES.md"))
                .with_context(|| format!("failed to read '{}'", temp.path().join("NOTES.md").display()))
                .unwrap()
                .contains("- Added")
        );

        let staging = temp.path().join("staging");
        std::fs::create_dir_all(staging.join("aaa")).unwrap();
        std::fs::create_dir_all(staging.join("bbb")).unwrap();
        std::fs::write(staging.join("aaa/1.txt"), "1").unwrap();
        std::fs::write(staging.join("bbb/2.txt"), "2").unwrap();
        std::fs::write(staging.join("root.txt"), "root").unwrap();

        let zip_file = temp.path().join("test.zip");
        make_zip_archive(&zip_file, &staging).unwrap();
        let file = std::fs::File::open(&zip_file)
            .with_context(|| format!("failed to read '{}'", zip_file.display()))
            .unwrap();
        let mut archive = zip::ZipArchive::new(file)
            .with_context(|| format!("failed to read zip archive '{}'", zip_file.display()))
            .unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| {
                archive
                    .by_index(i)
                    .with_context(|| format!("failed to read entry {i} in zip archive '{}'", zip_file.display()))
                    .unwrap()
                    .name()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["aaa/", "aaa/1.txt", "bbb/", "bbb/2.txt", "root.txt"]);
    }
}
