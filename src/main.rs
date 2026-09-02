mod cache;
mod cli;
mod commands;
mod config;
mod content;
mod digest;
mod git;
mod github;
mod http;
mod model;
mod site;
mod sources;
mod store;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .filter_module("html5ever::serialize", log::LevelFilter::Error)
        .format_timestamp(None)
        .init();

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            eprintln!("aggr: starting runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(commands::run(cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("aggr: {err:#}");
            ExitCode::FAILURE
        }
    }
}
