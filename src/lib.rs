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
mod text;
pub mod vars;

use anyhow::{Context, bail};
use cli::{Cli, Commands};
use config::Config;
use indexmap::IndexMap;
use std::path::{Path, PathBuf};

pub fn run(args: &Cli) -> anyhow::Result<()> {
    let Some(command) = &args.command else {
        bail!("no command specified");
    };

    match command {
        Commands::Init(cmd_args) => {
            if args.config_path.is_some() {
                tracing::warn!("--config-path is ignored for 'init'");
            }

            if !args.define.is_empty() {
                tracing::warn!("--define is ignored for 'init'");
            }

            if args.project_version.is_some() {
                tracing::warn!("--project-version is ignored for 'init'");
            }

            init::command(cmd_args)
        }
        Commands::Schema(cmd_args) => {
            if args.config_path.is_some() {
                tracing::warn!("--config-path is ignored for 'schema'");
            }

            if !args.define.is_empty() {
                tracing::warn!("--define is ignored for 'schema'");
            }

            if args.project_version.is_some() {
                tracing::warn!("--project-version is ignored for 'schema'");
            }

            schema::command(cmd_args)
        }
        Commands::SetVersion(cmd_args) => {
            if !args.define.is_empty() {
                tracing::warn!("--define is ignored for 'set-version'");
            }

            if args.project_version.is_some() {
                tracing::warn!("--project-version is ignored for 'set-version'");
            }

            let config_file = resolve_config_path(args)?;
            std::env::set_current_dir(config_file.parent().unwrap_or(Path::new(".")))?;

            set_version::command(&config_file, cmd_args)
        }
        Commands::Build(build_args) => build::command(&load_config(args)?, build_args),
        Commands::Cache(cache_args) => cache::command(&load_config(args)?, cache_args),
        Commands::Clean => clean::command(&load_config(args)?),
        Commands::Run(run_args) => run::run(&load_config(args)?, run_args),
        Commands::Release(release_args) => release::command(&load_config(args)?, release_args),
    }
}

fn resolve_config_path(args: &Cli) -> anyhow::Result<PathBuf> {
    if let Some(path) = &args.config_path {
        std::path::absolute(path).with_context(|| format!("failed to resolve absolute path '{}'", path.display()))
    } else {
        Config::find_path()
    }
}

fn load_config(args: &Cli) -> anyhow::Result<Config> {
    let config_file = resolve_config_path(args)?;
    std::env::set_current_dir(config_file.parent().unwrap_or(Path::new(".")))?;

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

    Config::load(&config_file, defines, args.project_version.as_deref())
}
