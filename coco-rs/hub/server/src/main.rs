use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use coco_hub_server::AppState;
use coco_hub_server::HubServerResult;
use coco_hub_server::LocalSessionJsonStore;
use coco_hub_server::SqliteHubServerOptions;
use coco_hub_server::serve_sqlite_until;
use coco_hub_server::store::RetentionPolicy;

#[derive(Debug, Parser)]
#[command(name = "coco-hub-server")]
#[command(about = "Serve a local read-only Event Hub view over session JSONL files")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, clap::Subcommand)]
enum Command {
    Serve(ServeArgs),
}

#[derive(Debug, Parser)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1")]
    bind: String,
    #[arg(long, default_value_t = 8731)]
    port: u16,
    /// Hub data directory containing events.sqlite.
    #[arg(long)]
    data_dir: Option<PathBuf>,
    /// Read-only local transcript memory base containing projects/<slug>/<session>.jsonl.
    #[arg(long, conflicts_with = "data_dir")]
    memory_base: Option<PathBuf>,
    #[arg(long, default_value_t = 3)]
    hub_retention_days: i64,
    #[arg(long, default_value_t = 3_221_225_472)]
    hub_retention_max_bytes: i64,
    #[arg(long, default_value_t = 900)]
    hub_retention_sweep_interval_secs: u64,
}

#[tokio::main]
async fn main() -> HubServerResult<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> HubServerResult<()> {
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    match args.memory_base {
        Some(memory_base) => {
            tracing::info!(memory_base = %memory_base.display(), "serving read-only local session hub");
            let app =
                coco_hub_server::router(AppState::new(LocalSessionJsonStore::new(memory_base)));
            let listener = tokio::net::TcpListener::bind(addr).await?;
            tracing::info!(%addr, "serving local session hub");
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await?;
        }
        None => {
            let data_dir = args.data_dir.unwrap_or_else(|| PathBuf::from("data"));
            tracing::info!(data_dir = %data_dir.display(), "serving ingest-capable sqlite event hub");
            serve_sqlite_until(
                addr,
                SqliteHubServerOptions {
                    data_dir,
                    retention_policy: RetentionPolicy {
                        retention_days: args.hub_retention_days,
                        retention_max_bytes: args.hub_retention_max_bytes,
                    },
                    retention_sweep_interval_secs: args.hub_retention_sweep_interval_secs,
                },
                shutdown_signal(),
            )
            .await?;
        }
    }
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
#[path = "main.test.rs"]
mod tests;
