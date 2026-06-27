use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;
use coco_hub_server::AppState;
use coco_hub_server::LocalSessionJsonStore;

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
    /// Memory base containing projects/<slug>/<session>.jsonl.
    #[arg(long)]
    memory_base: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.command {
        Command::Serve(args) => serve(args).await,
    }
}

async fn serve(args: ServeArgs) -> Result<(), Box<dyn std::error::Error>> {
    let memory_base = args
        .memory_base
        .unwrap_or_else(coco_config::global_config::config_home);
    let addr: SocketAddr = format!("{}:{}", args.bind, args.port).parse()?;
    let store = LocalSessionJsonStore::new(memory_base);
    let app = coco_hub_server::router(AppState::new(store));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "serving local session hub");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
