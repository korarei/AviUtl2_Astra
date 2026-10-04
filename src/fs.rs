use anyhow::Context;
use std::path::{Path, PathBuf};

pub(crate) fn resolve_glob(pattern: &str) -> anyhow::Result<(PathBuf, wax::Glob<'_>)> {
    let bytes = pattern.as_bytes();
    let (drive, expr) = if bytes.first().is_some_and(u8::is_ascii_alphabetic) && bytes.get(1) == Some(&b':') {
        pattern.split_at(2)
    } else {
        ("", pattern)
    };

    let (prefix, glob) = wax::Glob::new(expr)?.partition();
    let prefix = if drive.is_empty() {
        Path::new(".").join(prefix)
    } else {
        let mut path = std::ffi::OsString::from(drive);
        path.push(prefix.as_os_str());
        PathBuf::from(path)
    };

    std::fs::symlink_metadata(&prefix).with_context(|| format!("failed to read '{}'", prefix.display()))?;

    Ok((prefix, glob.unwrap_or_else(wax::Glob::empty)))
}

pub(crate) fn to_key(path: &Path) -> anyhow::Result<PathBuf> {
    let path = clean_path::clean(
        std::path::absolute(path).with_context(|| format!("failed to resolve absolute path '{}'", path.display()))?,
    );

    #[cfg(windows)]
    #[allow(clippy::upper_case_acronyms, non_camel_case_types, non_snake_case)]
    {
        use std::ffi::OsString;
        use std::os::windows::ffi::{OsStrExt, OsStringExt};

        type BOOLEAN = u8;
        type NTSTATUS = i32;
        type USHORT = u16;
        type WCHAR = u16;
        type PWSTR = *mut WCHAR;
        type PUNICODE_STRING = *mut UNICODE_STRING;
        type PCUNICODE_STRING = *const UNICODE_STRING;

        const FALSE: BOOLEAN = 0;

        #[repr(C)]
        struct UNICODE_STRING {
            Length: USHORT,
            MaximumLength: USHORT,
            Buffer: PWSTR,
        }

        unsafe extern "system" {
            fn RtlUpcaseUnicodeString(
                DestinationString: PUNICODE_STRING,
                SourceString: PCUNICODE_STRING,
                AllocateDestinationString: BOOLEAN,
            ) -> NTSTATUS;
        }

        let mut src: Vec<WCHAR> = path
            .as_os_str()
            .encode_wide()
            .map(|ch| if ch == u16::from(b'/') { u16::from(b'\\') } else { ch })
            .collect();

        let len = USHORT::try_from(src.len())?
            .checked_mul(2)
            .context("path is too long to convert to a comparison key")?;

        let mut buf = vec![0_u16; src.len()];
        let mut dst = UNICODE_STRING {
            Length: 0,
            MaximumLength: len,
            Buffer: buf.as_mut_ptr(),
        };

        let status = unsafe {
            RtlUpcaseUnicodeString(
                &raw mut dst,
                &UNICODE_STRING {
                    Length: len,
                    MaximumLength: len,
                    Buffer: src.as_mut_ptr(),
                },
                FALSE,
            )
        };

        if status < 0 {
            anyhow::bail!(
                "failed to convert path '{}' to a comparison key: NTSTATUS {status:#010x}",
                path.display()
            );
        }

        Ok(PathBuf::from(OsString::from_wide(&buf[..usize::from(dst.Length) / 2])))
    }

    #[cfg(not(windows))]
    Ok(path)
}

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
    use wax::walk::Entry;

    #[test]
    fn walks_absolute_globs_and_fixed_paths() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        assert!(dir.path().is_absolute());
        let one = dir.path().join("one.dll");
        let nested = dir.path().join("nested");
        let two = nested.join("two.dll");
        fs::create_dir(&nested)?;
        fs::write(&one, b"one")?;
        fs::write(&two, b"two")?;
        fs::write(dir.path().join("other.txt"), b"other")?;

        let base = dir.path().to_string_lossy().replace('\\', "/");
        #[cfg(windows)]
        let base = format!("{}{}", &base[..2], wax::escape(&base[2..]));
        #[cfg(not(windows))]
        let base = wax::escape(&base);

        for (suffix, mut expected) in [
            ("*.dll", vec![one.clone()]),
            ("**/*.dll", vec![one.clone(), two]),
            ("one.dll", vec![one]),
            ("nested", vec![nested]),
            ("*.missing", Vec::new()),
        ] {
            let pattern = format!("{base}/{suffix}");
            let (prefix, glob) = resolve_glob(&pattern)?;
            let mut paths = glob
                .walk(prefix)
                .map(|entry| entry.map(|entry| entry.path().to_path_buf()))
                .collect::<Result<Vec<_>, _>>()?;
            paths.sort_unstable();
            expected.sort_unstable();
            assert_eq!(paths, expected, "{pattern}");
        }

        Ok(())
    }

    #[test]
    fn rejects_invalid_glob_syntax() {
        assert!(resolve_glob("[").is_err());
    }

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
