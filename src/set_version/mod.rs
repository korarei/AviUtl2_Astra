use anyhow::{Context, bail};
use std::path::Path;
use toml_edit::{DocumentMut, Item, value};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Project version to set in astra.toml.
    #[arg(value_name = "VERSION")]
    version: String,
}

pub(super) fn command(file: &Path, args: &Args) -> anyhow::Result<()> {
    let ver = args.version.trim();
    if ver.is_empty() {
        bail!("project version cannot be empty");
    }

    let mut doc = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read '{}'", file.display()))?
        .parse::<DocumentMut>()
        .with_context(|| format!("failed to parse '{}'", file.display()))?;

    let project = doc
        .get_mut("project")
        .and_then(Item::as_table_mut)
        .ok_or_else(|| anyhow::anyhow!("project table not found in '{}'", file.display()))?;

    let decor = project
        .get("version")
        .and_then(Item::as_value)
        .map(|value| value.decor().clone());

    let mut new_val = value(ver);
    if let Some(decor) = decor
        && let Some(val) = new_val.as_value_mut()
    {
        val.decor_mut().clone_from(&decor);
    }

    project["version"] = new_val;

    crate::fs::write_file(file, doc.to_string().as_bytes())?;

    tracing::info!("Set project version to '{ver}'");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sets_version_without_reformatting() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let file = dir.path().join("astra.toml");
        std::fs::write(
            &file,
            "# keep\nversion = 2\n\n[project]\nname = \"demo\"\nversion = \"1.0.0\" # keep\n",
        )?;

        command(
            &file,
            &Args {
                version: "release-2026".to_owned(),
            },
        )?;

        let text = std::fs::read_to_string(&file).with_context(|| format!("failed to read '{}'", file.display()))?;
        assert!(text.contains("# keep"));
        assert!(text.contains("version = \"release-2026\" # keep"));
        Ok(())
    }
}
