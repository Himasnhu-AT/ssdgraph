pub mod routes;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::response::Redirect;
use axum::routing::get;
use axum::Router;
use tokio::sync::RwLock;
use tower_http::services::ServeDir;

use crate::graph::Graph;

#[derive(Clone)]
pub struct AppState {
    pub graph: Arc<RwLock<Graph>>,
}

pub fn router(state: AppState, dashboard_dir: PathBuf) -> Router {
    Router::new()
        .route("/", get(|| async { Redirect::permanent("/dashboard/") }))
        .nest("/api", routes::api_router())
        .nest_service("/dashboard", ServeDir::new(dashboard_dir))
        .with_state(state)
}

pub async fn serve(graph: Graph, addr: SocketAddr, dashboard_dir: &Path) -> anyhow::Result<()> {
    let state = AppState {
        graph: Arc::new(RwLock::new(graph)),
    };
    let app = router(state, dashboard_dir.to_path_buf());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(?addr, "ssdgraph listening");
    axum::serve(listener, app).await?;
    Ok(())
}
