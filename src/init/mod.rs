use anyhow::{Context, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    /// Directory where the project will be initialized.
    #[arg(value_name = "PATH", default_value = ".")]
    path: PathBuf,

    /// Project name.
    ///
    /// If omitted, the name of the target directory will be used.
    #[arg(long, value_name = "NAME")]
    name: Option<String>,
}

pub(super) fn command(args: &Args) -> anyhow::Result<()> {
    let path = if args.path == Path::new(".") {
        std::env::current_dir()?
    } else {
        args.path.clone()
    };

    if !path.exists() {
        std::fs::create_dir_all(&path).with_context(|| format!("failed to create directory '{}'", path.display()))?;
    } else if !path.is_dir() {
        bail!("init path '{}' is not a directory", path.display());
    }

    let file = path.join("astra.toml");
    if file.exists() {
        bail!("astra.toml already exists in '{}'", path.display());
    }

    let name = match args.name.as_deref().map(str::trim) {
        Some("") => bail!("project name cannot be empty"),
        Some(name) => name.to_owned(),
        None => path
            .file_name()
            .and_then(|name| name.to_str())
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("could not infer project name; use --name"))?,
    };

    let mut project = toml::map::Map::new();
    project.insert("name".to_owned(), toml::Value::String(name));

    let mut config = toml::map::Map::new();
    config.insert("version".to_owned(), toml::Value::Integer(2));
    config.insert("project".to_owned(), toml::Value::Table(project));

    std::fs::write(&file, toml::to_string_pretty(&toml::Value::Table(config))?)
        .with_context(|| format!("failed to write '{}'", file.display()))?;

    for (name, text) in [
        (
            ".gitattributes",
            concat!(
                "* text=auto eol=lf\n\n",
                "*.obj text eol=crlf linguist-language=Lua working-tree-encoding=cp932\n",
                "*.anm text eol=crlf linguist-language=Lua working-tree-encoding=cp932\n",
                "*.scn text eol=crlf linguist-language=Lua working-tree-encoding=cp932\n",
                "*.cam text eol=crlf linguist-language=Lua working-tree-encoding=cp932\n",
                "*.tra text eol=crlf linguist-language=Lua working-tree-encoding=cp932\n\n",
                "*.obj2 text eol=crlf linguist-language=Lua working-tree-encoding=utf-8\n",
                "*.anm2 text eol=crlf linguist-language=Lua working-tree-encoding=utf-8\n",
                "*.scn2 text eol=crlf linguist-language=Lua working-tree-encoding=utf-8\n",
                "*.cam2 text eol=crlf linguist-language=Lua working-tree-encoding=utf-8\n",
                "*.tra2 text eol=crlf linguist-language=Lua working-tree-encoding=utf-8\n\n",
                "*.aul2 text eol=crlf linguist-language=ini working-tree-encoding=utf-8\n"
            ),
        ),
        (".gitignore", "/build/\n/dist/\n/.astra/\n"),
    ] {
        let file = path.join(name);
        if !file.exists() {
            std::fs::write(&file, text).with_context(|| format!("failed to write '{}'", file.display()))?;
        }
    }

    tracing::info!(concat!(
        "Initialized astra project\n\n",
        "Next steps:\n",
        "  add a task under [tasks.<id>]\n",
        "  add a build under [builds.<id>]\n",
        "  add a release under [releases.<id>]\n",
        "  set [astra.run].release to use astra run"
    ));

    Ok(())
}
