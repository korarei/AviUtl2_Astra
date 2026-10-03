use clap::{Parser, Subcommand};

/// Build tool and task runner for `AviUtl ExEdit` extensions.
///
/// Astra is a dedicated build tool and task runner designed for developing
/// `AviUtl` and `ExEdit` plugins and scripts. It automates building, testing in
/// isolated `AviUtl` environments, packaging distributions, and running custom tasks.
#[derive(Debug, Parser)]
#[command(name = "astra", version)]
pub struct Cli {
    /// Enable verbose debug logging.
    #[arg(long, global = true)]
    pub verbose: bool,

    /// Override the project version defined in astra.toml.
    #[arg(short = 'v', long = "project-version", global = true, value_name = "VERSION")]
    pub(crate) project_version: Option<String>,

    /// Override or define a configuration variable in KEY VALUE pairs.
    ///
    /// Can be specified multiple times to define multiple variables.
    #[arg(short, long, global = true, num_args = 2, value_names = ["KEY", "VALUE"], action = clap::ArgAction::Append)]
    pub(crate) define: Vec<String>,

    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Commands {
    /// Build project targets defined in astra.toml.
    ///
    /// Compiles configured build targets according to the specified build type (debug/release).
    Build(crate::build::Args),
    /// Manage package cache.
    ///
    /// Cleans all cached packages, removes unused cache entries, or refreshes cached package URLs.
    Cache(crate::cache::Args),

    /// Clean build artifacts and distribution directories.
    ///
    /// Deletes output files in the build and dist directories while
    /// preserving internal lock files and git ignores.
    Clean,

    /// Initialize a new astra project.
    ///
    /// Creates a basic astra.toml configuration file in the specified directory.
    Init(crate::init::Args),

    /// Generate JSON Schema for astra.toml.
    ///
    /// Outputs the JSON Schema specification for astra.toml configuration files,
    /// useful for editor integration and validation.
    Schema(crate::schema::Args),

    /// Set project version in astra.toml.
    ///
    /// Updates the project version defined in the configuration file
    /// while preserving existing formatting and comments.
    SetVersion(crate::set_version::Args),

    /// Run a task or launch `AviUtl ExEdit2` with built binaries.
    ///
    /// Runs a predefined task from astra.toml or sets up an isolated environment
    /// and launches `AviUtl ExEdit2` with the compiled plugin or script.
    Run(crate::run::Args),

    /// Create release packages according to astra.toml.
    ///
    /// Builds and packages output files, dependencies, and documentation
    /// into distribution archives (e.g. zip).
    Release(crate::release::Args),
}
