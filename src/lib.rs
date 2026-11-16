//! Git Archiver: keeps local copies of public git repositories so their
//! history survives takedowns, deletions and rewrites upstream.

mod config;
mod web;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// How to start the app.
pub struct Options {
    /// Holds `config.toml` (later also `state.json` and `repos/`).
    pub data_dir: PathBuf,
    /// Overrides the config's `listen` address. Tests use this to bind a
    /// random port; the binary never sets it.
    pub listen: Option<SocketAddr>,
}

/// A started app, serving HTTP until shut down.
pub struct RunningApp {
    addr: SocketAddr,
    shutdown: oneshot::Sender<()>,
    server: JoinHandle<std::io::Result<()>>,
}

impl RunningApp {
    /// The address the app actually listens on.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Stops serving and waits for the server to finish.
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        let _ = self.server.await;
    }

    /// Serves until the server stops on its own (it normally doesn't).
    pub async fn wait(self) -> anyhow::Result<()> {
        let _keep_running = self.shutdown;
        self.server.await.context("server task failed")??;
        Ok(())
    }
}

/// Prepares the data directory, loads the config and starts serving.
pub async fn start(options: Options) -> anyhow::Result<RunningApp> {
    let config = config::load_or_init(&options.data_dir)?;
    let listen = options.listen.unwrap_or(config.listen);
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("couldn't listen on {listen}"))?;
    let addr = listener.local_addr()?;

    let router = web::router(Arc::new(web::AppState {
        config,
        config_path: config::path(&options.data_dir),
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
        server,
    })
}
