mod check;
mod client_test;
mod dev;
mod distribution;
mod package;
mod vendor;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail, ensure};

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn command(command: &mut Command) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("starting {:?}", command.get_program()))?;
    ensure!(
        status.success(),
        "{:?} failed ({status})",
        command.get_program()
    );
    Ok(())
}

async fn run() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let Some(action) = args.next() else {
        bail!(
            "usage: aggr-tools <dev|build|check|client-test|package|publish|homebrew|msrv|verify-release>"
        );
    };
    let args: Vec<String> = args.collect();
    match action.as_str() {
        "dev" => dev::run(args).await,
        "setup" => package::setup(),
        "vendor" => vendor::run(args).await,
        "check" | "client-check" => check::run(args).await,
        "client-test" => client_test::run(args).await,
        "build" => {
            let root = root();
            aggr_ui_model::export_types(root.join("web/.generated/types"))?;
            let output = args
                .first()
                .map_or_else(|| root.join("web/.generated/dist"), PathBuf::from);
            let result =
                tokio::task::spawn_blocking(move || aggr_ui_build::build(&root, &output)).await??;
            println!("{result:?}");
            Ok(())
        }
        "package" => package::run(args).await,
        "publish" => distribution::publish(args).await,
        "homebrew" => distribution::homebrew(args).await,
        "msrv" => package::msrv(args),
        "verify-release" => package::verify_release(args),
        _ => bail!("unknown development command {action}"),
    }
}

fn main() -> ExitCode {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("aggr-tools: {error}");
            return ExitCode::FAILURE;
        }
    };
    match runtime.block_on(run()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("aggr-tools: {error:#}");
            ExitCode::FAILURE
        }
    }
}
