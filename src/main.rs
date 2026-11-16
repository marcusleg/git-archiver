use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};

/// Keeps local copies of public git repositories so their history survives
/// takedowns, deletions and rewrites upstream.
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Directory holding config.toml, state and archives. Created if missing.
    #[arg(long, env = "GIT_ARCHIVER_DATA_DIR", value_name = "DIR")]
    data_dir: PathBuf,
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<()> {
    let app = git_archiver::start(git_archiver::Options {
        data_dir: cli.data_dir,
        listen: None,
    })
    .await?;

    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    tokio::select! {
        _ = terminate.recv() => {}
        _ = interrupt.recv() => {}
    }
    tracing::info!("shutting down");
    app.shutdown().await;
    Ok(())
}
