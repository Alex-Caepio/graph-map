use crate::common::{Engine, render_explanation};
use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::stream;
use graph_map_core::{
    GraphError,
    graph_changes::Target,
    subscribe_graph_events,
    util,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};
use tokio::sync::broadcast::error::RecvError;

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const BOOT_JS: &str = include_str!("../web/boot.js");
const LOGO_SVG: &str = include_str!("../web/icons/logo.svg");
const UI_JS: &str = include_str!("../web/assets/graph_map_ui.js");
const UI_WASM: &[u8] = include_bytes!("../web/assets/graph_map_ui_bg.wasm");

const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub const GRAPH_MAP_QUERY_PARAMS: &[&str] = &[
    "scope",
    "focus",
    "focus_session",
    "certain",
    "filter",
    "target",
    "follow",
    "out",
    "into",
    "notests",
    "spread",
    "from",
    "to",
    "path",
    "longer",
    "failing",
    "shared",
    "side",
    "skip",
    "cycle",
    "base",
    "patterns",
    "pattern",
    "history",
    "modules",
    "no_incoming",
    "launch",
    "compare_launch",
    "agent",
    "scenario",
    "whole",
    "offset",
    "limit",
    "depth",
    "lod",
    "min_weight",
];

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<Engine>,
    pub csrf: String,
    pub selected: Option<String>,
}

pub fn router(engine: Arc<Engine>, selected: Option<String>) -> Router {
    let csrf = util::unique_hex();
    let state = AppState {
        engine,
        csrf,
        selected,
    };
    Router::new()
        .route("/", get(get_index))
        .route("/index.html", get(get_index))
        .route("/app.css", get(get_css))
        .route("/boot.js", get(get_boot_js))
        .route("/icons/logo.svg", get(get_logo_svg))
        .route("/assets/graph_map_ui.js", get(get_ui_js))
        .route("/assets/graph_map_ui_bg.wasm", get(get_ui_wasm))
        .route("/api/health", get(get_health))
        .route("/api/bootstrap", get(get_bootstrap))
        .route("/api/events/stream", get(get_events_stream))
        .route("/api/projects", post(post_projects))
        .route("/api/projects/{project_id}/graph", get(get_graph_status).post(post_graph_act))
        .route("/api/projects/{project_id}/graph/map", get(get_graph_map))
        .route("/api/projects/{project_id}/graph/find", get(get_graph_find))
        .route("/api/projects/{project_id}/graph/changes", get(get_graph_changes))
        .route("/api/projects/{project_id}/graph/file-impact", get(get_graph_file_impact))
        .route("/api/projects/{project_id}/graph/pulls", get(get_graph_pulls))
        .route("/api/projects/{project_id}/graph/commits", get(get_graph_commits))
        .route("/api/projects/{project_id}/graph/branches", get(get_graph_branches))
        .route("/api/projects/{project_id}/graph/memory-links", get(get_graph_memory_links))
        .route("/api/projects/{project_id}/graph/memory-links/batch", post(post_graph_memory_links_batch))
        .route("/api/projects/{project_id}/graph/runs", get(get_graph_runs))
        .route("/api/projects/{project_id}/graph/run-event", get(get_graph_run_event))
        .route("/api/projects/{project_id}/graph/explain", post(post_graph_explain))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

pub async fn serve(
    engine: Arc<Engine>,
    host: &str,
    port: u16,
    selected: Option<String>,
) -> graph_map_core::Result<()> {
    let addr = format!("{host}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        GraphError::unavailable(format!("Failed to bind HTTP server to {addr}: {e}"))
    })?;
    let actual = listener
        .local_addr()
        .map_err(|e| GraphError::unavailable(format!("Failed to inspect socket addr: {e}")))?;
    eprintln!("graph-map UI listening at http://{actual}");
    let app = router(engine, selected);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| GraphError::unavailable(format!("HTTP server error: {e}")))
}

async fn security_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert_with(|| HeaderValue::from_static("no-store"));
    res
}

fn verify_csrf(headers: &HeaderMap, state: &AppState) -> std::result::Result<(), ApiError> {
    let provided = headers
        .get("x-harness-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != state.csrf {
        return Err(ApiError(GraphError::forbidden(
            "Missing or invalid connection token.",
        )));
    }
    Ok(())
}

struct ApiError(GraphError);

impl From<GraphError> for ApiError {
    fn from(err: GraphError) -> Self {
        Self(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(json!({ "error": self.0.message }))).into_response()
    }
}

fn static_asset(content_type: &'static str, body: Vec<u8>) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

async fn get_index() -> Response {
    static_asset("text/html; charset=utf-8", INDEX_HTML.as_bytes().to_vec())
}

async fn get_css() -> Response {
    static_asset("text/css; charset=utf-8", APP_CSS.as_bytes().to_vec())
}

async fn get_boot_js() -> Response {
    static_asset(
        "application/javascript; charset=utf-8",
        BOOT_JS.as_bytes().to_vec(),
    )
}

async fn get_logo_svg() -> Response {
    static_asset("image/svg+xml", LOGO_SVG.as_bytes().to_vec())
}

async fn get_ui_js() -> Response {
    static_asset(
        "application/javascript; charset=utf-8",
        UI_JS.as_bytes().to_vec(),
    )
}

async fn get_ui_wasm() -> Response {
    static_asset("application/wasm", UI_WASM.to_vec())
}

async fn get_health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "graph-map",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn get_bootstrap(State(state): State<AppState>) -> std::result::Result<Json<Value>, ApiError> {
    let mut projects = state.engine.store.projects()?;
    if projects.is_empty()
        && let Ok((id, _)) = state.engine.resolve_project(None, None)
    {
        projects = state.engine.store.projects()?;
        let _ = id;
    }
    let selected = state
        .selected
        .clone()
        .filter(|id| projects.iter().any(|p| p["id"] == *id))
        .or_else(|| {
            projects
                .first()
                .and_then(|p| p["id"].as_str())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "csrf": state.csrf,
        "selected": selected,
        "projects": projects,
    })))
}

async fn post_projects(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    verify_csrf(&headers, &state)?;
    let path = body["path"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GraphError::invalid("Missing project `path`."))?;
    let (id, _) = state.engine.resolve_project(Some(path), None)?;
    Ok(Json(state.engine.store.project(&id)?))
}

fn check_allowed_query(
    query: &BTreeMap<String, String>,
    allowed: &[&str],
) -> std::result::Result<(), ApiError> {
    let allowed_set: BTreeSet<&str> = allowed.iter().copied().collect();
    if query.keys().any(|k| !allowed_set.contains(k.as_str())) {
        return Err(ApiError(GraphError::invalid("Unexpected query parameter.")));
    }
    Ok(())
}

async fn execute_domain(
    state: &AppState,
    operation: &str,
    project: &str,
    query: &BTreeMap<String, String>,
    body: Option<Value>,
    include_fallback: bool,
) -> std::result::Result<Json<Value>, ApiError> {
    let mut request = json!({
        "params": { "project_id": project },
        "query": query,
    });
    if let Some(body) = body {
        request["body"] = body;
    }
    if include_fallback {
        let target = query
            .get("target")
            .map(String::as_str)
            .filter(|s| !s.is_empty())
            .map(Target::parse)
            .transpose()?;
        let record = state.engine.store.project(project)?;
        let root = PathBuf::from(record["path"].as_str().unwrap_or(""));
        if let Some(fallback) =
            state
                .engine
                .fallback_graph_path(project, &root, target.as_ref(), None)?
        {
            request["_graph_override"] = json!(fallback.to_string_lossy());
        }
    }
    Ok(Json(
        state.engine.graphs.domain_execute(operation, request).await?,
    ))
}

async fn get_events_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Response, ApiError> {
    check_allowed_query(&query, &["project_id", "last_event_id", "session_id"])?;
    let raw_cursor = headers
        .get("last-event-id")
        .and_then(|v| v.to_str().ok())
        .or_else(|| query.get("last_event_id").map(String::as_str))
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let start_seq = match raw_cursor {
        None => 0u64,
        Some(raw) => raw
            .parse::<u64>()
            .map_err(|_| GraphError::invalid("Invalid Last-Event-ID cursor."))?,
    };
    let filter_pid = query
        .get("project_id")
        .map(String::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let expected_state = state.engine.store.state.clone();
    let rx = subscribe_graph_events();

    let event_stream = stream::unfold(
        (rx, start_seq, expected_state, filter_pid),
        |(mut rx, mut seq, expected_state, filter_pid)| async move {
            loop {
                match tokio::time::timeout(Duration::from_secs(15), rx.recv()).await {
                    Ok(Ok(ev)) => {
                        if ev.state_dir != expected_state {
                            continue;
                        }
                        if filter_pid.as_deref().is_some_and(|pid| pid != ev.project_id) {
                            continue;
                        }
                        seq = seq.saturating_add(1);
                        let payload = serde_json::to_string(&ev).unwrap_or_else(|_| "{}".into());
                        let frame = format!("id: {seq}\nevent: graph_status\ndata: {payload}\n\n");
                        return Some((
                            Ok::<Bytes, Infallible>(Bytes::from(frame)),
                            (rx, seq, expected_state, filter_pid),
                        ));
                    }
                    Ok(Err(RecvError::Lagged(skipped))) => {
                        seq = seq.saturating_add(1);
                        let payload = json!({
                            "reason": "graph_bridge_lagged",
                            "skipped": skipped,
                        });
                        let frame = format!("id: {seq}\nevent: resync\ndata: {payload}\n\n");
                        return Some((
                            Ok::<Bytes, Infallible>(Bytes::from(frame)),
                            (rx, seq, expected_state, filter_pid),
                        ));
                    }
                    Ok(Err(RecvError::Closed)) => return None,
                    Err(_) => {
                        return Some((
                            Ok::<Bytes, Infallible>(Bytes::from_static(b": keepalive\n\n")),
                            (rx, seq, expected_state, filter_pid),
                        ));
                    }
                }
            }
        },
    );

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(event_stream))
        .map_err(|_| ApiError(GraphError::unavailable("Could not start SSE stream.")))
}

async fn get_graph_status(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &["check"])?;
    execute_domain(&state, "graph.status", &project, &query, None, false).await
}

async fn post_graph_act(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(project): AxumPath<String>,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    verify_csrf(&headers, &state)?;
    execute_domain(
        &state,
        "graph.act",
        &project,
        &BTreeMap::new(),
        Some(body),
        false,
    )
    .await
}

async fn get_graph_pulls(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &["state", "query"])?;
    execute_domain(&state, "graph.pulls", &project, &query, None, false).await
}

async fn get_graph_commits(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &["scope", "query", "skip"])?;
    execute_domain(&state, "graph.commits", &project, &query, None, false).await
}

async fn get_graph_branches(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &[])?;
    execute_domain(&state, "graph.branches", &project, &query, None, false).await
}

async fn get_graph_find(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &["query", "target", "notests"])?;
    execute_domain(&state, "graph.find", &project, &query, None, true).await
}

async fn get_graph_map(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, GRAPH_MAP_QUERY_PARAMS)?;
    execute_domain(&state, "graph.map", &project, &query, None, true).await
}

async fn get_graph_changes(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(
        &query,
        &["target", "from", "to", "focus_session", "agent", "notests"],
    )?;
    execute_domain(&state, "graph.changes", &project, &query, None, false).await
}

async fn get_graph_file_impact(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(
        &query,
        &["path", "target", "from", "to", "focus_session", "agent", "certain"],
    )?;
    execute_domain(&state, "graph.file_impact", &project, &query, None, false).await
}

async fn get_graph_memory_links(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    check_allowed_query(&query, &["path", "previous_path"])?;
    let engine = state.engine.clone();
    let query_json = json!(query);
    let val = tokio::task::spawn_blocking(move || engine.graphs.memory_links(&project, &query_json))
        .await
        .map_err(|_| GraphError::unavailable("Memory links task aborted."))??;
    Ok(Json(val))
}

async fn post_graph_memory_links_batch(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(project): AxumPath<String>,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    verify_csrf(&headers, &state)?;
    let engine = state.engine.clone();
    let val =
        tokio::task::spawn_blocking(move || engine.graphs.memory_links_batch(&project, &body))
            .await
            .map_err(|_| GraphError::unavailable("Memory links batch task aborted."))??;
    Ok(Json(val))
}

async fn get_graph_runs(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || engine.graphs.list_runs(&project))
        .await
        .map_err(|_| GraphError::unavailable("Runs query aborted."))??;
    Ok(Json(val))
}

async fn get_graph_run_event(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let session = query
        .get("session")
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| GraphError::invalid("Missing `session` parameter."))?
        .to_owned();
    let event_id = query
        .get("event")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .ok_or_else(|| GraphError::invalid("Invalid `event` parameter."))?;
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || engine.graphs.shell_event(&project, &session, event_id))
        .await
        .map_err(|_| GraphError::unavailable("Run event query aborted."))??;
    Ok(Json(val))
}

async fn post_graph_explain(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(project): AxumPath<String>,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    verify_csrf(&headers, &state)?;
    let kind = body["kind"].as_str().unwrap_or("node").to_owned();
    let target = match body["context"]["target"].as_str() {
        None | Some("") => None,
        Some(raw) => Some(Target::parse(raw)?),
    };
    let certain = body["context"]["certain"].as_bool().unwrap_or(false);
    let notests = body["context"]["notests"].as_bool().unwrap_or(false);

    let (from, to) = if kind == "link" {
        let f = body["from"]
            .as_str()
            .or_else(|| body["subject"].as_str())
            .unwrap_or("")
            .to_owned();
        let t = body["to"].as_str().unwrap_or("").to_owned();
        (f, Some(t))
    } else {
        let f = body["path"]
            .as_str()
            .or_else(|| body["node"].as_str())
            .or_else(|| body["subject"].as_str())
            .unwrap_or("")
            .to_owned();
        (f, None)
    };
    if from.is_empty() {
        return Err(ApiError(GraphError::invalid("Missing explanation subject.")));
    }

    let engine = state.engine.clone();
    let raw = tokio::task::spawn_blocking(move || {
        engine.graphs.explain(
            &project,
            target.as_ref(),
            &kind,
            &from,
            to.as_deref(),
            certain,
            notests,
        )
    })
    .await
    .map_err(|_| GraphError::unavailable("Explanation task aborted."))??;
    let answer = render_explanation(&raw);
    Ok(Json(json!({
        "answer": answer,
        "raw": raw,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_map_query_allowlist_covers_every_handler_param() {
        for part in [
            "scope",
            "focus",
            "focus_session",
            "certain",
            "filter",
            "target",
            "follow",
            "out",
            "into",
            "notests",
            "spread",
            "from",
            "to",
            "path",
            "longer",
            "failing",
            "shared",
            "side",
            "skip",
            "cycle",
            "base",
            "patterns",
            "pattern",
            "history",
            "modules",
            "no_incoming",
            "launch",
            "compare_launch",
            "agent",
            "scenario",
            "whole",
            "offset",
            "limit",
            "depth",
            "lod",
            "min_weight",
        ] {
            assert!(GRAPH_MAP_QUERY_PARAMS.contains(&part), "{part}");
        }
    }

    #[test]
    fn embedded_web_assets_are_non_empty() {
        assert!(INDEX_HTML.contains("id=\"graph-map\""));
        assert!(APP_CSS.contains(".graph-map"));
        assert!(BOOT_JS.contains("/assets/graph_map_ui.js"));
        assert!(LOGO_SVG.contains("<svg"));
        assert!(!UI_JS.is_empty());
        assert!(UI_WASM.starts_with(b"\0asm"));
    }
}
