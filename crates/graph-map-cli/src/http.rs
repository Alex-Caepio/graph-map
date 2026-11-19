use crate::common::{Engine, build_sharing, render_explanation};
use axum::{
    Json, Router,
    body::Body,
    extract::{Path as AxumPath, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use graph_map_core::{
    GraphError,
    graph_changes::Target,
    graphs::{Followed, Shown, places},
    util,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

const INDEX_HTML: &str = include_str!("../web/index.html");
const APP_CSS: &str = include_str!("../web/app.css");
const BOOT_JS: &str = include_str!("../web/boot.js");
const LOGO_SVG: &str = include_str!("../web/icons/logo.svg");
const UI_JS: &str = include_str!("../web/assets/graph_map_ui.js");
const UI_WASM: &[u8] = include_bytes!("../web/assets/graph_map_ui_bg.wasm");

const CSP: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self'; img-src 'self' data:; font-src 'self'; connect-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

pub const GRAPH_MAP_QUERY_PARAMS: &[&str] = &[
    "scope",
    "focus",
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
    "scenario",
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
        .route("/api/projects", post(post_projects))
        .route("/api/projects/{project_id}/graph", get(get_graph_status).post(post_graph_act))
        .route("/api/projects/{project_id}/graph/map", get(get_graph_map))
        .route("/api/projects/{project_id}/graph/find", get(get_graph_find))
        .route("/api/projects/{project_id}/graph/pulls", get(get_graph_pulls))
        .route("/api/projects/{project_id}/graph/commits", get(get_graph_commits))
        .route("/api/projects/{project_id}/graph/branches", get(get_graph_branches))
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
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("no-store"),
    );
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

async fn get_graph_status(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let visit = match query.get("check").map(String::as_str) {
        None | Some("0") => false,
        Some("1") => true,
        Some(_) => return Err(ApiError(GraphError::invalid("Invalid graph check."))),
    };
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || engine.graphs.status(&project, visit))
        .await
        .map_err(|_| GraphError::unavailable("Status task aborted."))??;
    Ok(Json(val))
}

async fn post_graph_act(
    State(state): State<AppState>,
    headers: HeaderMap,
    AxumPath(project): AxumPath<String>,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, ApiError> {
    verify_csrf(&headers, &state)?;
    if !util::exact(&body, &["action"], &["target"]) {
        return Err(ApiError(GraphError::invalid("Invalid code graph action.")));
    }
    let engine = state.engine.clone();
    match (body["action"].as_str().unwrap_or(""), body.get("target")) {
        ("changes", Some(target)) => {
            let target = Target::parse(target.as_str().unwrap_or(""))?;
            let val = tokio::task::spawn_blocking(move || engine.graphs.choose(&project, &target))
                .await
                .map_err(|_| GraphError::unavailable("The change could not be read."))??;
            Ok(Json(val))
        }
        (action, None) if action != "changes" => {
            let action = action.to_owned();
            let val = tokio::task::spawn_blocking(move || engine.graphs.act(&project, &action))
                .await
                .map_err(|_| GraphError::unavailable("Graph action aborted."))??;
            Ok(Json(val))
        }
        _ => Err(ApiError(GraphError::invalid("Invalid code graph action."))),
    }
}

async fn get_graph_pulls(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let (state_q, wanted, _scope, _skip) = parse_history_query(&query)?;
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || {
        if wanted.is_empty() && ["", "open"].contains(&state_q.as_str()) {
            engine.graphs.pulls(&project)
        } else {
            engine.graphs.found(&project, &state_q, &wanted)
        }
    })
    .await
    .map_err(|_| GraphError::unavailable("Pulls query aborted."))??;
    Ok(Json(val))
}

async fn get_graph_commits(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let (_state_q, wanted, scope, skip) = parse_history_query(&query)?;
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || engine.graphs.commits(&project, &scope, &wanted, skip))
        .await
        .map_err(|_| GraphError::unavailable("Commits query aborted."))??;
    Ok(Json(val))
}

async fn get_graph_branches(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
) -> std::result::Result<Json<Value>, ApiError> {
    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || engine.graphs.branches(&project))
        .await
        .map_err(|_| GraphError::unavailable("Branches query aborted."))??;
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

async fn get_graph_find(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let allowed: BTreeSet<&str> = ["query", "target", "notests"].into_iter().collect();
    if query.keys().any(|k| !allowed.contains(k.as_str())) {
        return Err(ApiError(GraphError::invalid("Unexpected query parameter.")));
    }
    let wanted = query.get("query").map(String::as_str).unwrap_or("");
    if wanted.len() > 400 || wanted.chars().any(char::is_control) {
        return Err(ApiError(GraphError::invalid("Invalid search.")));
    }
    let notests = match query.get("notests").map(String::as_str) {
        None | Some("0") => false,
        Some("1") => true,
        Some(_) => return Err(ApiError(GraphError::invalid("Invalid graph filter."))),
    };
    let target = match query.get("target").map(String::as_str) {
        None | Some("") => None,
        Some(text) => Some(Target::parse(text)?),
    };
    let engine = state.engine.clone();
    let wanted = wanted.to_owned();
    let val = tokio::task::spawn_blocking(move || {
        let record = engine.store.project(&project)?;
        let root = PathBuf::from(record["path"].as_str().unwrap_or(""));
        engine.run_find(&project, &root, target.as_ref(), &wanted, notests, None)
    })
    .await
    .map_err(|_| GraphError::unavailable("The code graph could not be searched."))??;
    Ok(Json(val))
}

async fn get_graph_map(
    State(state): State<AppState>,
    AxumPath(project): AxumPath<String>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, ApiError> {
    let allowed: BTreeSet<&str> = GRAPH_MAP_QUERY_PARAMS.iter().copied().collect();
    if query.keys().any(|k| !allowed.contains(k.as_str())) {
        return Err(ApiError(GraphError::invalid("Unexpected query parameter.")));
    }
    let text = |name: &str| -> std::result::Result<String, GraphError> {
        let val = query.get(name).map(String::as_str).unwrap_or("");
        (val.len() <= 1024)
            .then(|| val.to_owned())
            .ok_or_else(|| GraphError::invalid("Invalid code graph place."))
    };
    let flag = |name: &str| -> std::result::Result<bool, GraphError> {
        match query.get(name).map(String::as_str) {
            None | Some("0") => Ok(false),
            Some("1") => Ok(true),
            Some(_) => Err(GraphError::invalid("Invalid graph filter.")),
        }
    };

    let (scope, focus, filter) = (text("scope")?, text("focus")?, text("filter")?);
    let (certain, notests) = (flag("certain")?, flag("notests")?);
    let patterns = flag("patterns")?;
    let history = flag("history")?;
    let modules = flag("modules")?;
    let no_incoming = flag("no_incoming")?;
    let origin = text("scenario")?;
    let launch = text("launch")?;
    let compare_launch = text("compare_launch")?;
    let target = match query.get("target").map(String::as_str) {
        None | Some("") => None,
        Some(raw) => Some(Target::parse(raw)?),
    };
    let followed = Followed::parse(
        &text("follow")?,
        query.get("out").map(String::as_str).unwrap_or(""),
        query.get("into").map(String::as_str).unwrap_or(""),
    )?;
    let spread = places(
        query.get("spread").map(String::as_str).unwrap_or(""),
        "Too many folders are open here; close some first.",
        "Invalid folders to open here.",
    )?;
    let (from, to) = (text("from")?, text("to")?);
    if from.is_empty() != to.is_empty() {
        return Err(ApiError(GraphError::invalid("Invalid code graph link.")));
    }
    let ends = places(
        query.get("path").map(String::as_str).unwrap_or(""),
        "Invalid code graph path.",
        "Invalid code graph path.",
    )?;
    if !ends.is_empty() && ends.len() != 2 {
        return Err(ApiError(GraphError::invalid("Invalid code graph path.")));
    }
    let sharing = places(
        query.get("shared").map(String::as_str).unwrap_or(""),
        "Invalid code graph places.",
        "Invalid code graph places.",
    )?;
    if !sharing.is_empty() && (sharing.len() != 2 || !ends.is_empty()) {
        return Err(ApiError(GraphError::invalid("Invalid code graph places.")));
    }
    let cycle = text("cycle")?;
    let base = text("base")?;
    if !base.is_empty()
        && (!matches!(base.len(), 40 | 64) || !base.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(ApiError(GraphError::invalid("Invalid pull request base.")));
    }
    if !cycle.is_empty()
        && (cycle.chars().any(char::is_control)
            || !from.is_empty()
            || !ends.is_empty()
            || !sharing.is_empty()
            || followed.is_some())
    {
        return Err(ApiError(GraphError::invalid("Invalid code graph cycle.")));
    }
    let pattern = text("pattern")?;
    if !pattern.is_empty()
        && (pattern.chars().any(char::is_control)
            || !from.is_empty()
            || !ends.is_empty()
            || !sharing.is_empty()
            || !cycle.is_empty()
            || followed.is_some())
    {
        return Err(ApiError(GraphError::invalid("Invalid code graph pattern.")));
    }
    if !origin.is_empty()
        && (modules
            || !base.is_empty()
            || !from.is_empty()
            || !ends.is_empty()
            || !sharing.is_empty()
            || !cycle.is_empty()
            || !pattern.is_empty()
            || followed.is_some())
    {
        return Err(ApiError(GraphError::invalid("Invalid scenario view.")));
    }
    let users = match query.get("side").map(String::as_str) {
        None | Some("") => None,
        Some("both_use") if !sharing.is_empty() => Some(false),
        Some("uses_both") if !sharing.is_empty() => Some(true),
        Some(_) => return Err(ApiError(GraphError::invalid("Invalid code graph places."))),
    };
    let skip = match query.get("skip").map(String::as_str) {
        None | Some("") => 0,
        Some(text) if !sharing.is_empty() || !origin.is_empty() => text
            .parse::<usize>()
            .ok()
            .filter(|skip| *skip <= 1_000_000)
            .ok_or_else(|| GraphError::invalid("Invalid code graph places."))?,
        Some(_) => return Err(ApiError(GraphError::invalid("Invalid code graph places."))),
    };

    let engine = state.engine.clone();
    let val = tokio::task::spawn_blocking(move || {
        let record = engine.store.project(&project)?;
        let root = PathBuf::from(record["path"].as_str().unwrap_or(""));
        let shown = Shown {
            scope: &scope,
            focus: (!focus.is_empty()).then_some(focus.as_str()),
            certain,
            filter: &filter,
            followed: followed.as_ref(),
            notests,
            spread: &spread,
            link: (!from.is_empty()).then_some((from.as_str(), to.as_str())),
            path: match ends.as_slice() {
                [f, t] => Some((f.as_str(), t.as_str())),
                _ => None,
            },
            cycle: (!cycle.is_empty()).then_some(cycle.as_str()),
            base: (!base.is_empty()).then_some(base.as_str()),
            shared: match sharing.as_slice() {
                [f, t] => Some(build_sharing(f, t, users, skip)),
                _ => None,
            },
            patterns,
            history,
            modules,
            no_incoming,
            launch: (!launch.is_empty()).then_some(launch.as_str()),
            compare_launch: (!compare_launch.is_empty()).then_some(compare_launch.as_str()),
            scenario: (!origin.is_empty()).then_some((origin.as_str(), skip)),
            pattern: (!pattern.is_empty()).then_some(pattern.as_str()),
        };
        engine.run_map(&project, &root, target.as_ref(), &shown, None)
    })
    .await
    .map_err(|_| GraphError::unavailable("The code graph map could not be read."))??;
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

fn parse_history_query(
    query: &BTreeMap<String, String>,
) -> std::result::Result<(String, String, String, usize), GraphError> {
    let typed = |name: &str| {
        let value = query.get(name).map(String::as_str).unwrap_or("");
        (value.len() <= 200 && !value.chars().any(char::is_control))
            .then(|| value.to_owned())
            .ok_or_else(|| GraphError::invalid("Invalid search."))
    };
    let (state, wanted, scope) = (typed("state")?, typed("query")?, typed("scope")?);
    if !["", "open", "closed", "merged", "all"].contains(&state.as_str())
        || scope.starts_with('-')
        || scope.chars().any(char::is_whitespace)
    {
        return Err(GraphError::invalid("Invalid search."));
    }
    let skip = match query.get("skip").map(String::as_str) {
        None | Some("") => 0,
        Some(text) => text
            .parse::<usize>()
            .ok()
            .filter(|skip| *skip <= 1_000_000)
            .ok_or_else(|| GraphError::invalid("Invalid search."))?,
    };
    Ok((state, wanted, scope, skip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_map_query_allowlist_covers_every_handler_param() {
        for part in [
            "scope",
            "focus",
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
            "scenario",
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
