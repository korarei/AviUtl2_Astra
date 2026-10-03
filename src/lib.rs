mod aviutl2_version;
pub mod build;
mod cache;
mod catalog;
mod clean;
pub mod cli;
pub mod config;
mod fs;
mod http;
mod init;
pub mod logger;
mod package;
mod release;
mod run;
mod schema;
mod set_version;
mod task;
pub mod vars;

use anyhow::bail;
use cli::{Cli, Commands};
use config::Config;
use indexmap::IndexMap;
use std::path::Path;

pub fn run(args: &Cli) -> anyhow::Result<()> {
    let Some(command) = &args.command else {
        bail!("no command specified");
    };

    if let Commands::Init(args) = command {
        return init::command(args);
    }

    if let Commands::Schema(args) = command {
        return schema::command(args);
    }

    let config_file = Config::find_path()?;
    std::env::set_current_dir(config_file.parent().unwrap_or(Path::new(".")))?;

    if let Commands::SetVersion(args) = command {
        return set_version::command(&config_file, args);
    }

    let mut defines = IndexMap::with_capacity(args.define.len().div_ceil(2));
    for chunk in args.define.chunks(2) {
        let key = chunk[0].trim();

        if key.is_empty() {
            bail!("--define contains an empty variable name");
        }

        if defines.contains_key(key) {
            bail!("duplicate --define variable '{key}'");
        }

        let val = chunk.get(1).map_or("", String::as_str);
        defines.insert(key.to_string(), val.to_string());
    }

    let config = Config::load(&config_file, defines, args.project_version.as_deref())?;

    match command {
        Commands::Build(args) => build::command(&config, args)?,
        Commands::Cache(args) => cache::command(&config, args)?,
        Commands::Clean => clean::command(&config)?,
        Commands::Init(_) | Commands::Schema(_) | Commands::SetVersion(_) => unreachable!(),
        Commands::Run(args) => run::run(&config, args)?,
        Commands::Release(args) => release::command(&config, args)?,
    }

    Ok(())
}
