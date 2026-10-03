use anyhow::{Context, bail};
use reqwest::blocking::{Client, Response};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

pub(crate) fn client() -> anyhow::Result<&'static Client> {
    const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
    const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

    static CLIENT: OnceLock<Client> = OnceLock::new();

    if let Some(client) = CLIENT.get() {
        return Ok(client);
    }

    let client = Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()?;

    Ok(CLIENT.get_or_init(|| client))
}

pub(crate) fn read(response: Response, url: &str, max_bytes: u64) -> anyhow::Result<Vec<u8>> {
    if response.content_length().is_some_and(|length| length > max_bytes) {
        bail!("response from '{url}' exceeds the {max_bytes}-byte limit");
    }

    let mut bytes = Vec::new();
    response.take(max_bytes.saturating_add(1)).read_to_end(&mut bytes)?;

    if u64::try_from(bytes.len())? > max_bytes {
        bail!("response from '{url}' exceeds the {max_bytes}-byte limit");
    }

    Ok(bytes)
}

pub(crate) fn to_temp_file(
    dir: &Path,
    response: Response,
    url: &str,
    max_bytes: u64,
) -> anyhow::Result<tempfile::NamedTempFile> {
    if response.content_length().is_some_and(|length| length > max_bytes) {
        bail!("response from '{url}' exceeds the {max_bytes}-byte limit");
    }

    let mut file = tempfile::NamedTempFile::new_in(dir)
        .with_context(|| format!("failed to create temporary file in '{}'", dir.display()))?;

    if std::io::copy(&mut response.take(max_bytes.saturating_add(1)), file.as_file_mut())
        .with_context(|| format!("failed to copy response from '{url}' to '{}'", file.path().display()))?
        > max_bytes
    {
        bail!("response from '{url}' exceeds the {max_bytes}-byte limit");
    }

    Ok(file)
}
