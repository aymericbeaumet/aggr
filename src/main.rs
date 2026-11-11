use aggr_lib::{cli, commands};

use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = cli::Cli::parse_for_run();
    let level = match cli.verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(level))
        .filter_module("html5ever::serialize", log::LevelFilter::Error)
        // Rasterizing a publisher's SVG reports every CSS rule and attribute it has to skip
        // (animations, `:root`, `width="auto"`): the publisher's markup, not something to act on.
        .filter_module("simplecss", log::LevelFilter::Error)
        .filter_module("usvg", log::LevelFilter::Error)
        .filter_module("resvg", log::LevelFilter::Error)
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
