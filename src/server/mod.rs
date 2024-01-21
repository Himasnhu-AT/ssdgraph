pub mod routes;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use tokio::sync::RwLock;

use crate::graph::Graph;

#[derive(Clone)]
pub struct AppState {
    pub graph: Arc<RwLock<Graph>>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .nest("/api", routes::api_router())
        .with_state(state)
}

pub async fn serve(graph: Graph, addr: SocketAddr) -> anyhow::Result<()> {
    let state = AppState {
        graph: Arc::new(RwLock::new(graph)),
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(?addr, "ssdgraph listening");
    axum::serve(listener, app).await?;
    Ok(())
}
