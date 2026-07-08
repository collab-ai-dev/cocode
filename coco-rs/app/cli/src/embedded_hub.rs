use anyhow::Result;

use crate::Cli;

pub async fn start_if_requested(cli: &mut Cli) -> Result<Option<EmbeddedHubGuard>> {
    if !cli.serve_hub {
        return Ok(None);
    }
    start(cli).await.map(Some)
}

#[cfg(not(feature = "serve-hub"))]
async fn start(_cli: &mut Cli) -> Result<EmbeddedHubGuard> {
    anyhow::bail!(
        "This `coco` build was not compiled with the `serve-hub` feature. \
         Rebuild with `cargo build -p coco-cli --features serve-hub`. \
         Alternatively, run a separate `coco-hub-server serve` and pass \
         `--event-hub-url ws://127.0.0.1:8731/v1/connect`."
    );
}

#[cfg(feature = "serve-hub")]
async fn start(cli: &mut Cli) -> Result<EmbeddedHubGuard> {
    use std::net::SocketAddr;

    use coco_hub_server::SqliteHubServerOptions;
    use coco_hub_server::serve_sqlite_listener_until;
    use tokio::sync::oneshot;

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.hub_port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    let data_dir = default_data_dir();
    let options = SqliteHubServerOptions::new(data_dir);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let join = tokio::spawn(async move {
        if let Err(error) = serve_sqlite_listener_until(listener, options, async move {
            let _ = shutdown_rx.await;
        })
        .await
        {
            tracing::error!(%error, "embedded Event Hub server exited with error");
        }
    });

    let url = format!("ws://127.0.0.1:{}/v1/connect", local_addr.port());
    cli.event_hub_url = Some(url.clone());
    tracing::info!(%url, "embedded Event Hub server started");
    Ok(EmbeddedHubGuard {
        shutdown_tx: Some(shutdown_tx),
        join: Some(join),
    })
}

#[cfg(feature = "serve-hub")]
fn default_data_dir() -> std::path::PathBuf {
    coco_config::global_config::config_home().join("hub")
}

pub struct EmbeddedHubGuard {
    #[cfg(feature = "serve-hub")]
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    #[cfg(feature = "serve-hub")]
    join: Option<tokio::task::JoinHandle<()>>,
}

impl Drop for EmbeddedHubGuard {
    fn drop(&mut self) {
        #[cfg(feature = "serve-hub")]
        {
            if let Some(shutdown_tx) = self.shutdown_tx.take() {
                let _ = shutdown_tx.send(());
            }
            if let Some(join) = self.join.take() {
                join.abort();
            }
        }
    }
}

#[cfg(test)]
#[path = "embedded_hub.test.rs"]
mod tests;
