use clap::Parser;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let args = astra::cli::Cli::parse();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(if args.verbose { "debug" } else { "info" }));

    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .event_format(astra::logger::Formatter)
        .try_init();

    if let Err(err) = astra::run(&args) {
        tracing::error!("{err:#}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
