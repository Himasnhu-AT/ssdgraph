//! REST API. The dashboard talks to it; `curl` does too.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use super::AppState;
use crate::graph::property::{Properties, Value};
use crate::graph::{EdgeId, NodeId};

pub fn api_router() -> Router<AppState> {
    Router::new()
        .route("/nodes", get(list_nodes).post(create_node))
        .route("/edges", get(list_edges).post(create_edge))
        .route("/graph", get(get_graph))
        .route("/search", get(search))
}

#[derive(Serialize)]
struct NodeJson {
    id: NodeId,
    labels: Vec<String>,
    props: Properties,
}

#[derive(Serialize)]
struct EdgeJson {
    id: EdgeId,
    src: NodeId,
    dst: NodeId,
    #[serde(rename = "type")]
    type_name: String,
    props: Properties,
}

#[derive(Serialize)]
struct GraphJson {
    nodes: Vec<NodeJson>,
    edges: Vec<EdgeJson>,
}

#[derive(Deserialize)]
struct CreateNodeBody {
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    props: Properties,
}

#[derive(Deserialize)]
struct CreateEdgeBody {
    src: NodeId,
    dst: NodeId,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(default)]
    props: Properties,
}

#[derive(Deserialize)]
struct GraphQuery {
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

fn err500<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn list_nodes(
    State(state): State<AppState>,
) -> Result<Json<Vec<NodeJson>>, (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let ids = g.all_node_ids();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(n) = g.get_node(id).map_err(err500)? {
            out.push(NodeJson {
                id: n.id,
                labels: n.labels,
                props: n.props,
            });
        }
    }
    Ok(Json(out))
}

async fn list_edges(
    State(state): State<AppState>,
) -> Result<Json<Vec<EdgeJson>>, (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let ids = g.all_edge_ids();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(e) = g.get_edge(id).map_err(err500)? {
            out.push(EdgeJson {
                id: e.id,
                src: e.src,
                dst: e.dst,
                type_name: e.type_name,
                props: e.props,
            });
        }
    }
    Ok(Json(out))
}

async fn get_graph(
    State(state): State<AppState>,
    Query(q): Query<GraphQuery>,
) -> Result<Json<GraphJson>, (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let mut node_ids = g.all_node_ids();
    if let Some(lim) = q.limit {
        node_ids.truncate(lim);
    }
    let mut nodes = Vec::with_capacity(node_ids.len());
    let allowed: std::collections::HashSet<NodeId> = node_ids.iter().copied().collect();
    for id in &node_ids {
        if let Some(n) = g.get_node(*id).map_err(err500)? {
            nodes.push(NodeJson {
                id: n.id,
                labels: n.labels,
                props: n.props,
            });
        }
    }
    let mut edges = Vec::new();
    for eid in g.all_edge_ids() {
        if let Some(e) = g.get_edge(eid).map_err(err500)? {
            if allowed.contains(&e.src) && allowed.contains(&e.dst) {
                edges.push(EdgeJson {
                    id: e.id,
                    src: e.src,
                    dst: e.dst,
                    type_name: e.type_name,
                    props: e.props,
                });
            }
        }
    }
    Ok(Json(GraphJson { nodes, edges }))
}

async fn search(
    State(state): State<AppState>,
    Query(q): Query<SearchQuery>,
) -> Result<Json<Vec<NodeJson>>, (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let candidates = match &q.label {
        Some(label) => g.nodes_with_label(label),
        None => g.all_node_ids(),
    };
    let needle = q.q.unwrap_or_default().to_lowercase();
    let limit = q.limit.unwrap_or(usize::MAX);
    let mut out = Vec::new();
    for id in candidates {
        let Some(n) = g.get_node(id).map_err(err500)? else {
            continue;
        };
        let matches = if needle.is_empty() {
            true
        } else {
            n.labels.iter().any(|l| l.to_lowercase().contains(&needle))
                || n.props.iter().any(|(k, v)| {
                    k.to_lowercase().contains(&needle)
                        || matches!(v, Value::Str(s) if s.to_lowercase().contains(&needle))
                })
        };
        if matches {
            out.push(NodeJson {
                id: n.id,
                labels: n.labels,
                props: n.props,
            });
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(Json(out))
}

async fn create_node(
    State(state): State<AppState>,
    Json(body): Json<CreateNodeBody>,
) -> Result<(StatusCode, Json<HashMap<&'static str, NodeId>>), (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let id = g.create_node(body.labels, body.props).map_err(err500)?;
    g.sync().map_err(err500)?;
    let mut resp = HashMap::new();
    resp.insert("id", id);
    Ok((StatusCode::CREATED, Json(resp)))
}

async fn create_edge(
    State(state): State<AppState>,
    Json(body): Json<CreateEdgeBody>,
) -> Result<(StatusCode, Json<HashMap<&'static str, EdgeId>>), (StatusCode, String)> {
    let mut g = state.graph.write().await;
    let id = g
        .create_edge(body.src, body.dst, body.type_name, body.props)
        .map_err(err500)?;
    g.sync().map_err(err500)?;
    let mut resp = HashMap::new();
    resp.insert("id", id);
    Ok((StatusCode::CREATED, Json(resp)))
}

#[allow(dead_code)]
fn _impl_into_response<T: IntoResponse>(_: T) {}
