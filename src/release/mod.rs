mod stage;

use crate::build::BuildOutput;
use crate::config::{Astra, BuildType, Config, Release, ReleaseDependency};
use anyhow::{Context, bail};
use regex::{Captures, Regex};
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Clone, clap::Args)]
pub(crate) struct Args {
    /// ID of the specific release configuration to execute.
    ///
    /// If omitted, all configured releases will be executed.
    #[arg(value_name = "ID")]
    id: Option<String>,
}

struct ZipInput<'a> {
    path: &'a Path,
    src: PathBuf,
    size: u64,
}

pub(super) fn command(config: &Config, args: &Args) -> anyhow::Result<()> {
    tracing::info!("Running release");

    let releases = config.releases();

    if releases.len() == 0 {
        bail!("no release configurations found");
    }

    let astra = config.astra();

    let build_dir = Path::new(astra.build_dir());
    let dist_dir = Path::new(astra.dist_dir());
    crate::fs::create_managed_dir(build_dir)?;
    crate::fs::create_managed_dir(dist_dir)?;
    crate::fs::create_managed_dir(Path::new(".astra"))?;

    let _build_lock = crate::fs::lock(build_dir, "build directory")?;
    let _dist_lock = crate::fs::lock(dist_dir, "dist directory")?;
    let _astra_lock = crate::fs::lock(Path::new(".astra"), ".astra directory")?;

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
            ReleaseDependency::Build(call) => {
                let id = call.id();

                if !outputs.contains_key(id) {
                    let build = config.build(call, BuildType::Release)?;

                    if build.is_enabled(BuildType::Release) {
                        tracing::info!("Building dependency '{}' for release '{}'", id, release.id());
                        outputs.insert(
                            id.to_string(),
                            crate::build::run(&build, config, astra, BuildType::Release)?,
                        );
                    } else {
                        tracing::debug!("skipping build '{}': disabled for release build type", id);
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
            create_zip(&archive_file, temp_dir)?;
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
            if err.is_some() {
                tracing::error!("finally task '{}' failed: {e:#}", call.id());
            } else {
                err = Some(e.context(format!("finally task '{}' failed", call.id())));
            }
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

fn create_zip(dst: &Path, root: &Path) -> anyhow::Result<()> {
    let file = tempfile::NamedTempFile::new_in(
        dst.parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    )
    .with_context(|| format!("failed to create temporary file for '{}'", dst.display()))?;

    let workers = std::thread::available_parallelism()?.get().min(4);
    write_zip(zip::ZipWriter::new(file.as_file()), root, workers)
        .with_context(|| format!("failed to write zip archive '{}'", dst.display()))?;
    file.persist(dst)
        .with_context(|| format!("failed to persist '{}'", dst.display()))?;

    Ok(())
}

fn write_zip(mut zip: zip::ZipWriter<&std::fs::File>, root: &Path, workers: usize) -> anyhow::Result<()> {
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let entries = collect_zip_entries(root)?;

    let mut remaining = entries.as_slice();
    while !remaining.is_empty() {
        let (len, files) = select_batch(remaining, root, workers)?;
        let (batch, rest) = remaining.split_at(len);
        remaining = rest;

        let mut results = compress_batch(&files, options).into_iter();
        for (path, kind) in batch {
            if kind.is_dir() {
                zip.add_directory_from_path(path, options)
                    .with_context(|| format!("failed to add directory '{}' to zip archive", path.display()))?;
            } else if let Some(data) = results.next().transpose()?.flatten() {
                zip.add_prepared_file(data)
                    .with_context(|| format!("failed to add file '{}' to zip archive", path.display()))?;
            } else {
                zip.start_file_from_path(path, options)
                    .with_context(|| format!("failed to start file '{}' in zip archive", path.display()))?;
                let path = root.join(path);
                let mut src =
                    std::fs::File::open(&path).with_context(|| format!("failed to read '{}'", path.display()))?;
                std::io::copy(&mut src, &mut zip)
                    .with_context(|| format!("failed to copy '{}' into zip archive", path.display()))?;
            }
        }
    }

    zip.finish().context("failed to finish zip archive")?;
    Ok(())
}

fn collect_zip_entries(root: &Path) -> anyhow::Result<Vec<(PathBuf, std::fs::FileType)>> {
    let children = |dir: &Path| -> anyhow::Result<_> {
        let mut entries = std::fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.file_name()));
        Ok(entries)
    };

    let mut entries = Vec::new();
    let mut pending = children(root)?;
    while let Some(entry) = pending.pop() {
        let kind = entry.file_type()?;
        if !kind.is_dir() && !kind.is_file() {
            continue;
        }

        let path = entry.path();
        entries.push((path.strip_prefix(root)?.to_owned(), kind));
        if kind.is_dir() {
            pending.extend(children(&path)?);
        }
    }

    Ok(entries)
}

fn select_batch<'a>(
    entries: &'a [(PathBuf, std::fs::FileType)],
    root: &Path,
    workers: usize,
) -> anyhow::Result<(usize, Vec<ZipInput<'a>>)> {
    const MAX_FILE_SIZE: u64 = 32 * 1024 * 1024;
    const MAX_INPUT_SIZE: u64 = 64 * 1024 * 1024;

    let mut files = Vec::with_capacity(workers);
    let mut input_size = 0;

    for (pos, (path, kind)) in entries.iter().enumerate() {
        if kind.is_dir() {
            continue;
        }
        if files.len() == workers {
            return Ok((pos, files));
        }

        let src = root.join(path);
        let size = std::fs::metadata(&src)
            .with_context(|| format!("failed to get size of '{}'", src.display()))?
            .len();

        if size > MAX_FILE_SIZE {
            return Ok((if files.is_empty() { pos + 1 } else { pos }, files));
        }
        if input_size + size > MAX_INPUT_SIZE {
            return Ok((pos, files));
        }

        files.push(ZipInput { path, src, size });
        input_size += size;
    }

    Ok((entries.len(), files))
}

fn compress_batch(
    files: &[ZipInput<'_>],
    options: zip::write::SimpleFileOptions,
) -> Vec<anyhow::Result<Option<zip::write::PreparedZipFile>>> {
    if files.len() < 2 {
        return Vec::new();
    }

    std::thread::scope(|scope| {
        let tasks = files
            .iter()
            .map(|file| {
                std::thread::Builder::new()
                    .spawn_scoped(scope, move || compress_file(&file.src, file.path, options, file.size))
            })
            .collect::<Vec<_>>();

        tasks
            .into_iter()
            .map(|task| {
                task?
                    .join()
                    .map_err(|_| anyhow::anyhow!("zip compression worker panicked"))?
            })
            .collect()
    })
}

fn compress_file(
    src: &Path,
    file: &Path,
    options: zip::write::SimpleFileOptions,
    limit: u64,
) -> anyhow::Result<Option<zip::write::PreparedZipFile>> {
    use std::io::Read as _;

    let src = std::fs::File::open(src).with_context(|| format!("failed to read '{}'", src.display()))?;
    let mut builder = zip::write::ZipFileBuilder::new_from_path(file, options)?;
    let size = std::io::copy(&mut src.take(limit + 1), &mut builder)
        .with_context(|| format!("failed to compress '{}'", file.display()))?;

    if size > limit {
        return Ok(None);
    }

    Ok(Some(builder.finish()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read as _, Seek as _};

    #[test]
    fn extracts_release_notes() {
        for (src, expected) in [
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
            assert_eq!(extract_release_notes(src).as_deref(), Some(expected), "{src:?}");
        }

        assert!(extract_release_notes("## Changelog\n## [2] 2026\n- Same level\n").is_none());
        assert!(extract_release_notes("# Project\n## Installation\nUse it\n").is_none());
    }

    #[test]
    fn creates_release_notes() {
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
    }

    #[test]
    fn writes_zip_in_order_across_batches() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        std::fs::create_dir_all(staging.join("aaa")).unwrap();
        std::fs::create_dir_all(staging.join("bbb")).unwrap();
        std::fs::write(staging.join("aaa/1.txt"), "1").unwrap();
        std::fs::write(staging.join("bbb/2.txt"), "2").unwrap();
        std::fs::write(staging.join("root.txt"), "root").unwrap();

        let mut file = tempfile::tempfile().unwrap();
        write_zip(zip::ZipWriter::new(&file), &staging, 2).unwrap();
        file.rewind().unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..archive.len())
            .map(|i| {
                archive
                    .by_index(i)
                    .with_context(|| format!("failed to read ZIP entry {i}"))
                    .unwrap()
                    .name()
                    .unwrap()
                    .replace('\\', "/")
            })
            .collect();
        assert_eq!(names, vec!["aaa/", "aaa/1.txt", "bbb/", "bbb/2.txt", "root.txt"]);

        for (name, expected) in [("aaa/1.txt", "1"), ("bbb/2.txt", "2"), ("root.txt", "root")] {
            let mut content = String::new();
            let mut entry = archive.by_name(name).unwrap();
            assert_eq!(entry.compression(), zip::CompressionMethod::Deflated, "{name}");
            entry.read_to_string(&mut content).unwrap();
            assert_eq!(content, expected, "{name}");
        }
    }

    #[test]
    fn selects_zip_batches_within_limits() {
        const MIB: u64 = 1024 * 1024;

        for (sizes, batch_len, file_count) in [
            (&[32 * MIB, 32 * MIB, 1][..], 3, 2),
            (&[32 * MIB, 32 * MIB + 1, 1][..], 2, 1),
            (&[32 * MIB + 1, 1][..], 1, 0),
            (&[1, 1, 1, 1, 1][..], 5, 4),
        ] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            std::fs::create_dir(root.join("b")).unwrap();

            let paths = ["a.dll", "b/c.dll", "d.dll", "e.dll", "f.dll"];
            for (path, size) in paths.iter().zip(sizes) {
                std::fs::File::create(root.join(path)).unwrap().set_len(*size).unwrap();
            }

            let entries = collect_zip_entries(root).unwrap();
            let (len, files) = select_batch(&entries, root, 4).unwrap();
            assert_eq!(len, batch_len, "{sizes:?}");
            assert_eq!(
                files.iter().map(|file| file.path).collect::<Vec<_>>(),
                paths[..file_count].iter().map(Path::new).collect::<Vec<_>>(),
                "{sizes:?}",
            );
        }
    }

    #[test]
    fn skips_buffered_compression_over_read_limit() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("root.txt");
        std::fs::write(&src, "root").unwrap();
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        assert!(
            compress_file(&src, Path::new("root.txt"), options, 4)
                .unwrap()
                .is_some()
        );
        assert!(
            compress_file(&src, Path::new("root.txt"), options, 3)
                .unwrap()
                .is_none()
        );
    }
}
