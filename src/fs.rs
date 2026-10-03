use anyhow::Context;
use std::path::Path;

pub(crate) fn create_managed_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create directory '{}'", dir.display()))?;
    if !dir.join(".gitignore").is_file() {
        write_file(&dir.join(".gitignore"), b"*\n")?;
    }

    Ok(())
}

pub(crate) fn write_file(file: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;

    let dir = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create directory '{}'", dir.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temporary file in '{}'", dir.display()))?;
    temp.write_all(bytes)
        .with_context(|| format!("failed to write '{}'", file.display()))?;
    let _ = temp
        .persist(file)
        .with_context(|| format!("failed to persist '{}'", file.display()))?;
    Ok(())
}

pub(crate) fn read_file(file: &Path, encoding: &'static encoding_rs::Encoding) -> anyhow::Result<String> {
    let bytes = std::fs::read(file).with_context(|| format!("failed to read '{}'", file.display()))?;
    let (decoded, _, is_malformed) = encoding.decode(&bytes);
    let text = if !is_malformed {
        decoded.into_owned()
    } else if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(&bytes) {
        encoding.decode(&bytes[bom_len..]).0.into_owned()
    } else {
        let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
        detector.feed(&bytes, true);

        detector
            .guess(None, chardetng::Utf8Detection::Allow)
            .decode(&bytes)
            .0
            .into_owned()
    };

    Ok(if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    })
}

pub(crate) fn remove(path: &Path) -> anyhow::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };

    #[cfg(windows)]
    if std::os::windows::fs::FileTypeExt::is_symlink_dir(&meta.file_type()) {
        std::fs::remove_dir(path)?;
        return Ok(());
    }

    if meta.is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }

    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> anyhow::Result<u128> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    let mut hash = xxhash_rust::xxh3::Xxh3::new();
    let mut bytes = vec![0_u8; 64 * 1024];

    loop {
        let size = file
            .read(&mut bytes)
            .with_context(|| format!("failed to read '{}'", path.display()))?;
        if size == 0 {
            break;
        }

        hash.update(&bytes[..size]);
    }

    Ok(hash.digest128())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn removes_files_directories_and_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file");
        fs::write(&file, "file").unwrap();
        remove(&file).unwrap();
        remove(&file).unwrap();

        let dir = temp.path().join("dir");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("file"), "file").unwrap();
        remove(&dir).unwrap();

        let target = temp.path().join("target");
        fs::write(&target, "target").unwrap();
        let link = temp.path().join("link");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_file(&target, &link).is_ok();
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&target, &link).is_ok();
        if created {
            remove(&link).unwrap();
            assert!(fs::symlink_metadata(&link).is_err());
            assert!(target.exists());
        }

        let target_dir = temp.path().join("target_dir");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("file"), "file").unwrap();
        let link_dir = temp.path().join("link_dir");
        #[cfg(windows)]
        let created = std::os::windows::fs::symlink_dir(&target_dir, &link_dir).is_ok();
        #[cfg(not(windows))]
        let created = std::os::unix::fs::symlink(&target_dir, &link_dir).is_ok();
        if created {
            remove(&link_dir).unwrap();
            assert!(fs::symlink_metadata(&link_dir).is_err());
            assert!(target_dir.exists());
            assert!(target_dir.join("file").is_file());
        }
    }
}
