//! Git Archiver: keeps local copies of public git repositories so their
//! history survives takedowns, deletions and rewrites upstream.

mod archive;
pub mod clock;
mod config;
mod git_http;
mod identity;
mod metrics;
mod overview;
mod scheduler;
mod state;
mod tracking;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::archive::Archives;
use crate::clock::Clock;
use crate::overview::Overview;
use crate::scheduler::Scheduler;
use crate::tracking::Tracking;

/// How to start the app.
pub struct Options {
    /// Holds `config.toml`, `state.json` and the archives under `repos/`.
    pub data_dir: PathBuf,
    /// Overrides the config's `listen` address. Tests use this to bind a
    /// random port; the binary never sets it.
    pub listen: Option<SocketAddr>,
    /// Where the app reads the time. Tests replace the system clock.
    pub clock: Arc<dyn Clock>,
}

/// A started app, serving HTTP and fetching on schedule until shut down.
pub struct RunningApp {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    /// Ends the open event streams, which the server would wait for.
    closing: CancellationToken,
    server: JoinHandle<std::io::Result<()>>,
    scheduler: JoinHandle<()>,
}

impl RunningApp {
    /// The address the app actually listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops fetching and serving, and waits for both to finish. A fetch in
    /// progress is killed; the next start picks up from the saved state.
    pub async fn shutdown(self) {
        self.scheduler.abort();
        let _ = self.scheduler.await;
        self.closing.cancel();
        let _ = self.shutdown.send(());
        let _ = self.server.await;
    }

    /// Resolves only if the server stops on its own, which it normally
    /// doesn't. Don't call `shutdown` after this has resolved.
    pub async fn stopped(&mut self) -> anyhow::Result<()> {
        (&mut self.server).await.context("server task failed")??;
        anyhow::bail!("server stopped unexpectedly")
    }
}

/// Prepares the data directory, loads the config and state, and starts
/// serving and fetching.
pub async fn start(options: Options) -> anyhow::Result<RunningApp> {
    let data_dir = options.data_dir;
    let config = config::load_or_init(&data_dir)?;
    let state_path = state::file_path(&data_dir);
    let state = state::load(&state_path)?;
    let listen = options.listen.unwrap_or(config.listen);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("couldn't listen on {listen}"))?;
    let addr = listener.local_addr()?;

    let overview = Overview::new(overview::tracked(&config.repos), state, state_path);
    let archives = Arc::new(Archives::new(&data_dir));
    let tracking = Arc::new(Tracking::new(
        config::file_path(&data_dir),
        options.clock.clone(),
        overview.clone(),
        archives.clone(),
    ));
    let scheduler = tokio::spawn(
        Scheduler {
            clock: options.clock.clone(),
            archives: archives.clone(),
            interval: config.interval,
            started: options.clock.now(),
            overview: overview.clone(),
            tracking: tracking.clone(),
            tokens: config.tokens,
        }
        .run(),
    );

    let closing = CancellationToken::new();
    let router = web::router(Arc::new(web::AppState {
        config_path: config::file_path(&data_dir),
        clock: options.clock,
        archives,
        overview,
        tracking,
        closing: closing.clone(),
    }));
    let (shutdown, shutdown_rx) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
    });
    tracing::info!(%addr, "listening");

    Ok(RunningApp {
        addr,
        shutdown,
        closing,
        server,
        scheduler,
    })
}
