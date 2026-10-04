use crate::build::BuildOutput;
use crate::config::{
    PackageContent, PackageSource, PackageSourceBuild, PackageSourceFile, PackageSourcePath, PackageSourceUrl,
};
use crate::{fs, http};
use anyhow::{Context, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use wax::walk::Entry;

pub(crate) const CACHE_DIR: &str = ".astra/cache";

#[derive(Serialize, Deserialize)]
struct UrlFile {
    #[serde(default)]
    url: String,
    path: PathBuf,
    name: Option<String>,
}

impl UrlFile {
    fn read(file: &Path) -> anyhow::Result<Option<Self>> {
        match std::fs::read_to_string(file) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("failed to parse '{}'", file.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("failed to read '{}'", file.display())),
        }
    }

    fn write(&self, file: &Path) -> anyhow::Result<()> {
        fs::write_file(file, &serde_json::to_vec(self)?)
    }
}

pub(crate) struct PackageFile {
    pub(crate) dst: PathBuf,
    pub(crate) src: PathBuf,
}

pub(crate) struct PackageCache {
    is_refresh: bool,
    root: PathBuf,
    blobs: PathBuf,
    trees: PathBuf,
    client: Option<&'static Client>,
    urls: BTreeMap<String, UrlFile>,
    archives: BTreeMap<PathBuf, PathBuf>,
    paths: BTreeSet<PathBuf>,
}

impl PackageCache {
    pub(crate) fn new(is_refresh: bool) -> anyhow::Result<Self> {
        fs::create_managed_dir(Path::new(".astra"))?;
        let root = std::path::absolute(CACHE_DIR)?;

        Ok(Self {
            is_refresh,
            blobs: root.join("blobs"),
            trees: root.join("trees"),
            root,
            client: None,
            urls: BTreeMap::new(),
            archives: BTreeMap::new(),
            paths: BTreeSet::new(),
        })
    }

    #[cfg(test)]
    pub(crate) fn create_temp() -> anyhow::Result<(tempfile::TempDir, Self)> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join(CACHE_DIR);

        Ok((
            dir,
            Self {
                is_refresh: false,
                blobs: root.join("blobs"),
                trees: root.join("trees"),
                root,
                client: None,
                urls: BTreeMap::new(),
                archives: BTreeMap::new(),
                paths: BTreeSet::new(),
            },
        ))
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn hash<'a>(
        &self,
        contents: impl IntoIterator<Item = anyhow::Result<&'a PackageContent>>,
    ) -> anyhow::Result<Option<u128>> {
        let mut hash = xxhash_rust::xxh3::Xxh3::new();
        for content in contents {
            for src in content?.sources() {
                match src {
                    PackageSource::Url(src) => {
                        let Some(file) = UrlFile::read(&self.root.join("urls").join(format!(
                            "{:032x}.json",
                            xxhash_rust::const_xxh3::xxh3_128(src.url().as_bytes())
                        )))?
                        else {
                            return Ok(None);
                        };
                        if file.path.parent() != Some(self.blobs.as_path()) || !file.path.is_file() {
                            return Ok(None);
                        }
                        hash.update(&serde_json::to_vec(&file)?);
                    }
                    PackageSource::Path(src) => {
                        let mut paths = Vec::new();
                        let (prefix, glob) = fs::resolve_glob(src.path())?;
                        for entry in glob.walk(prefix) {
                            let entry = entry?;
                            if !entry.path().is_dir() {
                                paths.push(std::path::absolute(entry.path())?);
                            }
                        }
                        paths.sort_unstable();
                        hash.update(&serde_json::to_vec(&paths)?);
                    }
                    _ => {}
                }
            }
        }
        Ok(Some(hash.digest128()))
    }

    pub(crate) fn clean(&self) -> anyhow::Result<()> {
        fs::remove(&self.root)
    }

    pub(crate) fn collect<'a>(&self, used: impl Iterator<Item = &'a String>) -> anyhow::Result<usize> {
        let mut keep = HashSet::new();

        for target in used {
            let path = Path::new(target);
            let path = if path.is_absolute() {
                path.to_owned()
            } else {
                self.root.join(path)
            };

            let path = std::path::absolute(&path)
                .with_context(|| format!("failed to resolve absolute path '{}'", path.display()))?;
            for dir in [&self.blobs, &self.trees] {
                if let Ok(path) = path.strip_prefix(dir)
                    && let Some(Component::Normal(name)) = path.components().next()
                {
                    let _ = keep.insert(dir.join(name));
                    if dir == &self.trees {
                        let _ = keep.insert(self.blobs.join(name));
                    }
                }
            }
        }

        let mut obsolete = Vec::new();
        let dir = self.root.join("urls");
        if dir.is_dir() {
            for entry in
                std::fs::read_dir(&dir).with_context(|| format!("failed to read directory '{}'", dir.display()))?
            {
                let path = entry?.path();
                let Some(file) = UrlFile::read(&path)? else {
                    continue;
                };
                if file.path.parent() != Some(self.blobs.as_path())
                    || !keep.contains(&file.path)
                    || !file.path.is_file()
                {
                    obsolete.push(path);
                }
            }
        }

        let mut removed = 0;
        for dir in [&self.blobs, &self.trees] {
            if !dir.is_dir() {
                continue;
            }

            for entry in std::fs::read_dir(dir)? {
                let path = entry?.path();
                if keep.contains(&path) {
                    continue;
                }

                fs::remove(&path)?;
                removed += 1;
            }
        }

        for path in obsolete {
            fs::remove(&path)?;
            removed += 1;
        }

        Ok(removed)
    }

    pub(crate) fn refresh(&mut self) -> anyhow::Result<usize> {
        let dir = self.root.join("urls");
        if !dir.is_dir() {
            return Ok(0);
        }

        let mut urls = BTreeSet::new();
        for entry in std::fs::read_dir(&dir).with_context(|| format!("failed to read directory '{}'", dir.display()))? {
            let path = entry?.path();
            if let Some(file) = UrlFile::read(&path)?
                && !file.url.is_empty()
            {
                urls.insert(file.url);
            }
        }

        self.urls.clear();

        let mut count = 0;
        for url in urls {
            self.download(&url, true)?;
            count += 1;
        }

        Ok(count)
    }

    pub(crate) fn reserve(&mut self, dst: &Path) -> anyhow::Result<()> {
        use std::ops::Bound::{Included, Unbounded};

        let key = fs::to_key(dst)?;
        if key.ancestors().any(|path| self.paths.contains(path))
            || self
                .paths
                .range::<Path, _>((Included(key.as_path()), Unbounded))
                .next()
                .is_some_and(|path| path.starts_with(&key))
        {
            bail!("package destination is specified more than once: '{}'", dst.display());
        }

        let _ = self.paths.insert(key);
        Ok(())
    }

    pub(crate) fn resolve(
        &mut self,
        dst: &Path,
        src: &PackageSource,
        outputs: &BTreeMap<String, BuildOutput>,
    ) -> anyhow::Result<Vec<PackageFile>> {
        let dst = clean_path::clean(dst);
        match src {
            PackageSource::Build(src) => self.collect_artifacts(&dst, src, outputs),
            PackageSource::Path(src) => self.collect_path(&dst, src),
            PackageSource::Url(src) => self.collect_url(&dst, src),
            PackageSource::File(src) => self.store_file(&dst, src),
            PackageSource::Simple(_) => unreachable!("package source must be expanded"),
        }
    }

    fn collect_artifacts(
        &mut self,
        dst: &Path,
        src: &PackageSourceBuild,
        outputs: &BTreeMap<String, BuildOutput>,
    ) -> anyhow::Result<Vec<PackageFile>> {
        let output = outputs
            .get(src.id())
            .ok_or_else(|| anyhow::anyhow!("artifacts for build '{}' were not produced", src.id()))?;
        let mut files = Vec::with_capacity(output.artifacts.len());

        for artifact in &output.artifacts {
            let name = artifact
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("build artifact has no file name: '{}'", artifact.display()))?;
            files.push(self.add(dst.join(name), artifact)?);
        }

        Ok(files)
    }

    fn collect_path(&mut self, dst: &Path, src: &PackageSourcePath) -> anyhow::Result<Vec<PackageFile>> {
        let mut files = Vec::new();

        let (prefix, glob) = fs::resolve_glob(src.path())?;
        for entry in glob.walk(prefix) {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                continue;
            }

            let name = path
                .file_name()
                .ok_or_else(|| anyhow::anyhow!("source path has no file name: '{}'", path.display()))?;
            files.push(self.add(dst.join(name), path)?);
        }

        if files.is_empty() {
            tracing::warn!("no files matched for path '{}'", src.path());
        }

        Ok(files)
    }

    fn collect_url(&mut self, dst: &Path, src: &PackageSourceUrl) -> anyhow::Result<Vec<PackageFile>> {
        let (prefix, glob) = src
            .pick()
            .map(wax::Glob::new)
            .transpose()?
            .map_or_else(|| (PathBuf::new(), Some(wax::Glob::tree())), wax::Glob::partition);

        self.download(src.url(), self.is_refresh)?;

        if src.extract().unwrap_or_default() {
            let root = self.extract(src.url())?;
            let path = root.join(&prefix);
            let base = if glob.is_none() && !prefix.as_os_str().is_empty() {
                path.parent().unwrap_or(&root)
            } else {
                &path
            };
            let mut files = Vec::new();

            for entry in glob.unwrap_or_else(wax::Glob::tree).walk(&path) {
                let entry = entry?;
                let file = entry.path();
                if entry.file_type().is_dir() {
                    continue;
                }

                if !entry.file_type().is_file() {
                    bail!("extracted path is not a regular file: '{}'", file.display());
                }

                files.push(self.add(dst.join(file.strip_prefix(base)?), file)?);
            }

            if let Some(pick) = src.pick()
                && files.is_empty()
            {
                bail!("zip archive '{}' contains no files matching '{}'", src.url(), pick);
            }

            Ok(files)
        } else {
            let file = self
                .urls
                .get(src.url())
                .ok_or_else(|| anyhow::anyhow!("the URL was not downloaded: '{}'", src.url()))?;
            let name = file
                .name
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("the URL has no file name: '{}'", src.url()))?;

            let file = PackageFile {
                dst: dst.join(name),
                src: std::path::absolute(&file.path)?,
            };
            self.reserve(&file.dst)?;
            Ok(vec![file])
        }
    }

    fn store_file(&mut self, dst: &Path, src: &PackageSourceFile) -> anyhow::Result<Vec<PackageFile>> {
        let dst = dst.join(src.filename());
        self.reserve(&dst)?;

        let content = match src.newline() {
            Some(newline) => Cow::Owned(src.content().replace('\n', newline)),
            None => src.content(),
        };

        let encoding = src.encoding();
        let (bytes, _, has_unmappable) = encoding.encode(&content);
        if has_unmappable {
            bail!(
                "failed to encode file '{}' as {}: contains unmappable characters",
                src.filename(),
                encoding.name()
            );
        }

        let path = self
            .blobs
            .join(format!("{:032x}", xxhash_rust::const_xxh3::xxh3_128(bytes.as_ref())));
        if !path.is_file() {
            fs::write_file(&path, &bytes)?;
        }

        let path = std::path::absolute(path)?;

        Ok(vec![PackageFile { dst, src: path }])
    }

    fn download(&mut self, url: &str, is_refresh: bool) -> anyhow::Result<()> {
        const MAX_URL_BYTES: u64 = 1024 * 1024 * 1024;

        if self.urls.contains_key(url) {
            return Ok(());
        }

        let manifest = self.root.join("urls").join(format!(
            "{:032x}.json",
            xxhash_rust::const_xxh3::xxh3_128(url.as_bytes())
        ));
        if !is_refresh
            && let Some(file) = UrlFile::read(&manifest)?
            && file.path.parent() == Some(self.blobs.as_path())
            && file.path.is_file()
        {
            let _ = self.urls.insert(url.to_owned(), file);
            return Ok(());
        }

        tracing::info!("Downloading '{url}'");

        if self.client.is_none() {
            self.client = Some(http::client()?);
        }

        let response = self
            .client
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("the HTTP client is not initialized"))?
            .get(url)
            .send()?
            .error_for_status()?;
        let filename = response
            .url()
            .path_segments()
            .and_then(|mut segments| segments.next_back())
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
        std::fs::create_dir_all(&self.blobs)
            .with_context(|| format!("failed to create directory '{}'", self.blobs.display()))?;
        let file = http::to_temp_file(&self.blobs, response, url, MAX_URL_BYTES)?;
        let path = self.blobs.join(format!("{:032x}", fs::hash_file(file.path())?));

        if is_refresh || !path.is_file() {
            let _ = file
                .persist(&path)
                .with_context(|| format!("failed to persist '{}'", path.display()))?;
        }

        let file = UrlFile {
            url: url.to_owned(),
            path,
            name: filename,
        };
        file.write(&manifest)?;
        let _ = self.urls.insert(url.to_owned(), file);
        Ok(())
    }

    fn extract(&mut self, url: &str) -> anyhow::Result<PathBuf> {
        let src = self
            .urls
            .get(url)
            .ok_or_else(|| anyhow::anyhow!("the URL was not downloaded: '{url}'"))?;
        if let Some(path) = self.archives.get(&src.path).cloned() {
            return Ok(path);
        }

        let name = src
            .path
            .file_name()
            .ok_or_else(|| anyhow::anyhow!("cached URL has no file name: '{url}'"))?;
        let path = self.trees.join(name);

        if !path.is_dir() {
            fs::remove(&path)?;
            std::fs::create_dir_all(&self.trees)
                .with_context(|| format!("failed to create directory '{}'", self.trees.display()))?;
            let temp = tempfile::tempdir_in(&self.trees)
                .with_context(|| format!("failed to create temporary directory in '{}'", self.trees.display()))?;
            let mut archive = zip::ZipArchive::new(
                std::fs::File::open(&src.path).with_context(|| format!("failed to read '{}'", src.path.display()))?,
            )
            .with_context(|| format!("failed to read zip archive '{}'", src.path.display()))?;

            {
                let mut entries = HashMap::new();
                for i in 0..archive.len() {
                    let file = archive
                        .by_index(i)
                        .with_context(|| format!("failed to read entry {i} in zip archive '{}'", src.path.display()))?;
                    let entry = file.enclosed_name().ok_or_else(|| {
                        anyhow::anyhow!(
                            "zip archive '{}' contains an unsafe path: '{}'",
                            src.path.display(),
                            file.name()
                        )
                    })?;

                    let mut parts = entry.components().peekable();
                    let mut path = PathBuf::new();

                    while let Some(part) = parts.next() {
                        path.push(part.as_os_str());

                        let is_file = parts.peek().is_none() && !file.is_dir();
                        if let Some(has_file) = entries.insert(fs::to_key(&path)?, is_file)
                            && (has_file || is_file)
                        {
                            bail!(
                                "zip archive '{}' contains a conflicting entry: '{}'",
                                src.path.display(),
                                file.name()
                            );
                        }
                    }
                }
            }

            archive.extract(temp.path()).with_context(|| {
                format!(
                    "failed to extract zip archive '{}' to '{}'",
                    src.path.display(),
                    temp.path().display()
                )
            })?;

            std::fs::rename(temp.path(), &path)
                .with_context(|| format!("failed to move '{}' to '{}'", temp.path().display(), path.display()))?;
        }

        let _ = self.archives.insert(src.path.clone(), path.clone());
        Ok(path)
    }

    fn add(&mut self, dst: PathBuf, src: &Path) -> anyhow::Result<PackageFile> {
        self.reserve(&dst)?;
        let src = std::path::absolute(src)?;
        Ok(PackageFile { dst, src })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn resolves_picks() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        fs::create_dir_all(dir.path().join("aaa"))?;
        fs::create_dir_all(dir.path().join("assets/sub"))?;
        fs::write(dir.path().join("aaa/foo.txt"), "foo")?;
        fs::write(dir.path().join("assets/sub/icon.png"), "icon")?;

        for (pick, expected) in [
            (Some("aaa/*"), vec!["Plugin/foo.txt"]),
            (Some("aaa/foo.txt"), vec!["Plugin/foo.txt"]),
            (Some("assets"), vec!["Plugin/assets/sub/icon.png"]),
            (Some("assets/"), vec!["Plugin/assets/sub/icon.png"]),
            (Some("assets/**"), vec!["Plugin/sub/icon.png"]),
            (None, vec!["Plugin/aaa/foo.txt", "Plugin/assets/sub/icon.png"]),
        ] {
            let src: PackageSourceUrl = serde_json::from_value(serde_json::json!({
                "url": "http://127.0.0.1:0/archive.zip",
                "extract": true,
                "pick": pick,
            }))?;
            let (_dir, mut cache) = PackageCache::create_temp()?;
            let _ = cache.urls.insert(
                src.url().to_owned(),
                UrlFile {
                    url: src.url().to_owned(),
                    path: PathBuf::from("archive.zip"),
                    name: None,
                },
            );
            let _ = cache
                .archives
                .insert(PathBuf::from("archive.zip"), dir.path().to_path_buf());
            let mut files = cache
                .collect_url(Path::new("Plugin"), &src)?
                .into_iter()
                .map(|file| file.dst)
                .collect::<Vec<_>>();
            files.sort();
            assert_eq!(files, expected.into_iter().map(PathBuf::from).collect::<Vec<_>>());
        }

        let (_dir, mut cache) = PackageCache::create_temp()?;
        let src = serde_json::from_value(serde_json::json!({
            "url": "http://127.0.0.1:0/archive.zip",
            "extract": true,
            "pick": "[",
        }))?;
        assert_eq!(
            cache
                .collect_url(Path::new("Plugin"), &src)
                .err()
                .map(|err| err.to_string()),
            Some(wax::Glob::new("[").unwrap_err().to_string())
        );
        Ok(())
    }

    #[test]
    fn collects_and_cleans_cache() -> anyhow::Result<()> {
        let (_dir, cache) = PackageCache::create_temp()?;
        let blobs = cache.root().join("blobs");
        let trees = cache.root().join("trees");
        fs::create_dir_all(&blobs)?;
        fs::create_dir_all(&trees)?;
        fs::write(blobs.join("used"), "used")?;
        fs::write(blobs.join("unused"), "unused")?;
        fs::create_dir_all(trees.join("live/sub"))?;
        fs::write(trees.join("live/sub/file"), "live")?;
        fs::create_dir_all(trees.join("dead"))?;
        fs::write(trees.join("dead/file"), "dead")?;

        assert_eq!(
            cache.collect(
                BTreeMap::from([
                    ("Plugin/used".to_owned(), "blobs/used".to_owned()),
                    ("Script/file".to_owned(), "trees/live/sub/file".to_owned()),
                ])
                .values(),
            )?,
            2
        );
        assert!(blobs.join("used").is_file());
        assert!(!blobs.join("unused").exists());
        assert!(trees.join("live/sub/file").is_file());
        assert!(!trees.join("dead").exists());

        cache.clean()?;
        assert!(!cache.root().exists());
        Ok(())
    }

    #[test]
    fn caches_url_content_and_extraction() -> anyhow::Result<()> {
        let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        archive.start_file("sub/file.txt", zip::write::SimpleFileOptions::default())?;
        archive.write_all(b"cached")?;
        let bytes = archive.finish()?.into_inner();

        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let url = format!("http://127.0.0.1:{}/archive.zip", listener.local_addr()?.port());
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            stream.read(&mut request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            )
            .unwrap();
            stream.write_all(&bytes).unwrap();
        });

        let source: PackageSource = serde_json::from_value(serde_json::json!({
            "url": url.clone(),
            "extract": true
        }))?;
        let (_dir, mut cache) = PackageCache::create_temp()?;
        let first = cache.resolve(Path::new("Plugin"), &source, &BTreeMap::new())?;
        let second = cache.resolve(Path::new("Script"), &source, &BTreeMap::new())?;
        assert_eq!(first[0].src, second[0].src);
        assert_eq!(
            fs::read(&first[0].src).with_context(|| format!("failed to read '{}'", first[0].src.display()))?,
            b"cached"
        );

        let mut cache = PackageCache {
            client: None,
            urls: BTreeMap::new(),
            archives: BTreeMap::new(),
            paths: BTreeSet::new(),
            ..cache
        };
        let third = cache.resolve(Path::new("Plugin"), &source, &BTreeMap::new())?;
        assert_eq!(first[0].src, third[0].src);
        assert_eq!(fs::read_dir(cache.root().join("blobs"))?.count(), 1);
        assert_eq!(fs::read_dir(cache.root().join("trees"))?.count(), 1);

        let source: PackageSource = serde_json::from_value(serde_json::json!({ "url": url }))?;
        let raw = cache.resolve(Path::new("Raw"), &source, &BTreeMap::new())?;
        assert_eq!(raw[0].dst, PathBuf::from("Raw/archive.zip"));

        server.join().unwrap();
        Ok(())
    }

    #[test]
    fn refreshes_cached_urls() -> anyhow::Result<()> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        let url = format!("http://127.0.0.1:{}/file.txt", listener.local_addr()?.port());
        let server = thread::spawn(move || {
            for content in [b"initial".as_slice(), b"updated".as_slice()] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0; 1024];
                stream.read(&mut request).unwrap();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    content.len()
                )
                .unwrap();
                stream.write_all(content).unwrap();
            }
        });

        let source: PackageSource = serde_json::from_value(serde_json::json!({ "url": url }))?;
        let (_dir, mut cache) = PackageCache::create_temp()?;

        assert_eq!(cache.refresh()?, 0);

        let files = cache.resolve(Path::new("Raw"), &source, &BTreeMap::new())?;
        assert_eq!(fs::read(&files[0].src)?, b"initial");

        let refreshed = cache.refresh()?;
        assert_eq!(refreshed, 1);

        cache.paths.clear();
        let files = cache.resolve(Path::new("Raw"), &source, &BTreeMap::new())?;
        assert_eq!(fs::read(&files[0].src)?, b"updated");

        server.join().unwrap();
        Ok(())
    }
}
