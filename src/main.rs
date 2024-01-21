use std::net::SocketAddr;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use ssdgraph::graph::Graph;
use ssdgraph::server;

#[derive(Parser)]
#[command(version, about = "ssdgraph — disk-resident property graph")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start the HTTP server and dashboard.
    Serve {
        #[arg(long, default_value = "./data")]
        data: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7878")]
        addr: SocketAddr,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "ssdgraph=info,tower_http=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Serve { data, addr } => {
            let graph = Graph::open(&data)?;
            tracing::info!(?data, "graph opened");
            server::serve(graph, addr).await?;
        }
    }
    Ok(())
}
