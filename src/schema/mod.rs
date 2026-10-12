use anyhow::{Context, bail};
use schemars::generate::SchemaSettings;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Output file path for the JSON schema.
    ///
    /// If omitted, the schema will be printed to standard output.
    #[arg(long, value_name = "PATH")]
    output: Option<PathBuf>,
}

pub(super) fn command(args: &Args) -> anyhow::Result<()> {
    let text = format!(
        "{}\n",
        serde_json::to_string_pretty(
            &SchemaSettings::default()
                .with(|settings| settings.inline_subschemas = true)
                .into_generator()
                .into_root_schema_for::<crate::config::Config>(),
        )?
    );

    let Some(path) = &args.output else {
        std::io::stdout()
            .lock()
            .write_all(text.as_bytes())
            .context("failed to write schema to standard output")?;
        return Ok(());
    };

    if path.exists() {
        bail!("schema output '{}' already exists", path.display());
    }

    tracing::info!("Writing schema '{}'", path.display());
    std::fs::write(path, text).with_context(|| format!("failed to write '{}'", path.display()))?;

    Ok(())
}
