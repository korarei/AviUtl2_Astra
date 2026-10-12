use clap::Parser;
use std::io::Write;
use std::process::ExitCode;
use tracing_subscriber::EnvFilter;

fn main() -> ExitCode {
    let args = astra::cli::Cli::parse();

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(if args.is_verbose { "debug" } else { "info" }));

    if let Err(err) = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .event_format(astra::logger::Formatter)
        .try_init()
    {
        let _ = writeln!(std::io::stderr().lock(), "error: failed to initialize logging: {err}");
        return ExitCode::FAILURE;
    }

    if let Err(err) = astra::run(&args) {
        tracing::error!("{err:#}");
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
