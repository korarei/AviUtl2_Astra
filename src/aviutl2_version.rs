use anyhow::bail;

pub(crate) const FIRST_STABLE_NUM: u32 = 2_005_400;

pub(crate) fn resolve(ver: &str) -> anyhow::Result<u32> {
    let ver = ver.trim();

    if ver.is_empty() {
        bail!("empty AviUtl ExEdit2 version");
    }

    if ver.eq_ignore_ascii_case("latest") {
        return resolve_latest();
    }

    let ver = ver.strip_prefix(['v', 'V']).unwrap_or(ver);

    parse(ver).ok_or_else(|| anyhow::anyhow!("invalid AviUtl ExEdit2 version '{ver}'"))
}

fn resolve_latest() -> anyhow::Result<u32> {
    const AVIUTL2_PACKAGE_ID: &str = "Kenkun.AviUtlExEdit2";

    let package = crate::catalog::get(AVIUTL2_PACKAGE_ID)?;

    let latest = package
        .get("latest-version")
        .or_else(|| package.get("latest_version"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("latest version for package '{AVIUTL2_PACKAGE_ID}' not found in catalog"))?;

    let latest = latest.trim();
    let latest = latest.strip_prefix(['v', 'V']).unwrap_or(latest);

    parse(latest).ok_or_else(|| anyhow::anyhow!("failed to parse latest version '{latest}' from catalog"))
}

fn parse(ver: &str) -> Option<u32> {
    const LAST_BETA_NUM: u32 = 2_005_301;

    let ver = ver.trim();

    if let Ok(n) = ver.parse::<u32>() {
        return Some(n);
    }

    let parse_patch_build = |s: &str| -> Option<u32> {
        let (patch, build) = if let Some(last) = s.chars().last()
            && last.is_ascii_alphabetic()
        {
            (
                &s[..s.len() - 1],
                u32::from(last.to_ascii_lowercase()) - u32::from(b'a') + 1,
            )
        } else {
            (s, 0)
        };
        patch.parse::<u32>().ok()?.checked_mul(100)?.checked_add(build)
    };

    if let Some(prefix) = ver.get(..4)
        && prefix.eq_ignore_ascii_case("beta")
    {
        let rest = ver.get(4..)?;
        let ver = 2_000_000u32.checked_add(parse_patch_build(rest)?)?;
        return (ver <= LAST_BETA_NUM).then_some(ver);
    }

    let mut parts = ver.split('.').map(str::trim);
    if let (Some(major), Some(minor), Some(patch), None) = (parts.next(), parts.next(), parts.next(), parts.next()) {
        let ver = major
            .parse::<u32>()
            .ok()?
            .checked_mul(1_000_000)?
            .checked_add(minor.parse::<u32>().ok()?.checked_mul(10_000)?)?
            .checked_add(parse_patch_build(patch)?)?;
        return (ver >= FIRST_STABLE_NUM).then_some(ver);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aviutl2_version_forms() {
        for (source, expected) in [
            ("2010701", 2_010_701),
            (" 2010701 ", 2_010_701),
            ("2.1.8", 2_010_800),
            ("2.1.7a", 2_010_701),
            ("2.1.7b", 2_010_702),
            ("beta53", 2_005_300),
            ("BETA53", 2_005_300),
            ("beta53a", 2_005_301),
            ("v2.1.8", 2_010_800),
            ("V2.1.8", 2_010_800),
            ("2.0.54", 2_005_400),
        ] {
            assert_eq!(resolve(source).unwrap(), expected, "{source}");
        }

        for source in [
            "",
            "abc",
            "2.1",
            "beta54",
            "beta53b",
            "2.0.53",
            "2.0.53a",
            "あ",
            "あい",
            "あbeta53",
        ] {
            assert!(parse(source).is_none(), "{source}");
            assert!(resolve(source).is_err(), "{source}");
        }
    }
}
