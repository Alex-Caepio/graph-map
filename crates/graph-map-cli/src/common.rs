use graph_map_core::{
    GraphError, Graphs, MapQueryParams, Result, StateStore,
    graph_changes::Target,
    graph_map::Sharing,
    graphs::{Followed, Shown},
    util,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

pub struct Engine {
    pub store: Arc<StateStore>,
    pub graphs: Arc<Graphs>,
}

impl Engine {
    pub fn open_default() -> Result<Self> {
        let store = Arc::new(StateStore::open_default()?);
        let graphs = Graphs::new(store.clone());
        Ok(Self { store, graphs })
    }

    pub fn open_at(state_dir: &Path) -> Result<Self> {
        let store = Arc::new(StateStore::open(state_dir)?);
        let graphs = Graphs::new(store.clone());
        Ok(Self { store, graphs })
    }

    pub fn resolve_project(
        &self,
        explicit_project: Option<&str>,
        session_default: Option<&Path>,
    ) -> Result<(String, PathBuf)> {
        self.resolve_project_inner(explicit_project, session_default, false)
    }

    pub fn resolve_mcp_project(
        &self,
        explicit_project: Option<&str>,
        session_default: Option<&Path>,
    ) -> Result<(String, PathBuf)> {
        self.resolve_project_inner(explicit_project, session_default, true)
    }

    fn resolve_project_inner(
        &self,
        explicit_project: Option<&str>,
        session_default: Option<&Path>,
        guard_plugin_cwd: bool,
    ) -> Result<(String, PathBuf)> {
        let trimmed = explicit_project.map(str::trim).filter(|s| !s.is_empty());
        let cwd = std::env::current_dir().ok();
        let cwd_is_plugin = cwd.as_deref().is_some_and(looks_like_plugin_cache);
        if guard_plugin_cwd {
            if let Some(raw) = trimmed {
                if cwd_is_plugin && Path::new(raw).is_relative() {
                    return Err(GraphError::invalid(
                        "Pass `project` (absolute repository directory path); the server process cwd is inside the plugin cache.",
                    ));
                }
            } else {
                let effective_default = session_default.or(cwd.as_deref());
                if effective_default.is_some_and(looks_like_plugin_cache) {
                    return Err(GraphError::invalid(
                        "Pass `project` (repository directory path); the server process cwd is inside the plugin cache.",
                    ));
                }
            }
        }
        let candidate = match trimmed {
            Some(path) => PathBuf::from(path),
            None => match session_default {
                Some(path) => path.to_path_buf(),
                None => cwd.ok_or_else(|| {
                    GraphError::invalid("Could not determine current working directory.")
                })?,
            },
        };
        if guard_plugin_cwd && looks_like_plugin_cache(&candidate) {
            return Err(GraphError::invalid(
                "Pass `project` (repository directory path); target resolves inside a plugin directory.",
            ));
        }
        if !candidate.is_dir() {
            return Err(GraphError::missing(format!(
                "Project directory does not exist: {}",
                candidate.display()
            )));
        }
        let root = util::canonical_dir(&candidate)?;
        let id = self.graphs.ensure_project(&root)?;
        Ok((id, root))
    }

    pub fn fallback_graph_path(
        &self,
        project: &str,
        root: &Path,
        target: Option<&Target>,
        explicit_graph: Option<&Path>,
    ) -> Result<Option<PathBuf>> {
        if let Some(path) = explicit_graph {
            return Ok(Some(self.graphs.resolve_graph_file(project, root, Some(path))?));
        }
        let supports_fallback = matches!(
            target,
            None | Some(Target::Work | Target::Uncommitted | Target::Run(_))
        );
        if supports_fallback {
            let home = self.store.state.join("graphs").join(project);
            let live = home.join("live").join("graph.json");
            let ir_db = home.join("graph_ir.sqlite3");
            if !live.is_file() && !ir_db.is_file() {
                for candidate in [
                    root.join("graphify-out").join("graph.json"),
                    root.join(".graph-map").join("graph.json"),
                ] {
                    if candidate.is_file() {
                        return Ok(Some(candidate));
                    }
                }
            }
        }
        Ok(None)
    }

    pub fn ensure_graph_or_import(&self, project: &str, root: &Path) -> Result<Option<Value>> {
        let home = self.store.state.join("graphs").join(project);
        let live = home.join("live").join("graph.json");
        let ir_db = home.join("graph_ir.sqlite3");
        if live.is_file() || ir_db.is_file() {
            return Ok(None);
        }
        for candidate in [
            root.join("graphify-out").join("graph.json"),
            root.join(".graph-map").join("graph.json"),
        ] {
            if candidate.is_file() {
                return Ok(Some(self.graphs.import_graph(project, &candidate)?));
            }
        }
        Ok(None)
    }

    pub fn run_map(
        &self,
        project: &str,
        root: &Path,
        target: Option<&Target>,
        shown: &Shown<'_>,
        explicit_graph: Option<&Path>,
    ) -> Result<Value> {
        let graph_override = self.fallback_graph_path(project, root, target, explicit_graph)?;
        if let Some(t) = target
            && !matches!(t, Target::Work | Target::Uncommitted | Target::Run(_))
        {
            let _ = self.graphs.choose(project, t);
        }
        self.graphs
            .map_with_graph(project, target, shown, graph_override.as_deref())
    }

    pub fn run_find(
        &self,
        project: &str,
        root: &Path,
        target: Option<&Target>,
        query: &str,
        notests: bool,
        explicit_graph: Option<&Path>,
    ) -> Result<Value> {
        let graph_override = self.fallback_graph_path(project, root, target, explicit_graph)?;
        self.graphs
            .find_with_graph(project, target, query, notests, graph_override.as_deref())
    }
}

fn has_plugin_markers(dir: &Path) -> bool {
    let norm = dir.to_string_lossy().replace('\\', "/");
    let has_bin = dir.join("bin").join("graph-map").is_file()
        || dir.join("bin").join("graph-map.exe").is_file();
    norm.contains("/.codex/plugins/cache/")
        || norm.ends_with("/.codex/plugins/cache")
        || norm.contains("/.claude/plugins/cache/")
        || norm.ends_with("/.claude/plugins/cache")
        || norm.contains("/.gemini/config/plugins/")
        || norm.ends_with("/.gemini/config/plugins")
        || dir.join(".codex-plugin").join("plugin.json").is_file()
        || dir.join(".claude-plugin").join("plugin.json").is_file()
        || dir.join(".antigravity-plugin").join("plugin.json").is_file()
        || dir.join(".gemini-plugin").join("plugin.json").is_file()
        || dir.join(".claude-plugin").join("marketplace.json").is_file()
        || dir.join(".agents").join("plugins").join("marketplace.json").is_file()
        || (dir.join("plugin.json").is_file() && (dir.join("mcp_config.json").is_file() || has_bin))
        || ((dir.join(".mcp.json").is_file() || dir.join("mcp_config.json").is_file()) && has_bin)
}

fn looks_like_plugin_cache(path: &Path) -> bool {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for check in [path, resolved.as_path()] {
        for anc in check.ancestors().take(4) {
            if has_plugin_markers(anc) {
                return true;
            }
        }
    }
    false
}

pub fn parse_target(text: Option<&str>) -> Result<Option<Target>> {
    match text.map(str::trim) {
        None | Some("") | Some("project") | Some("all") => Ok(None),
        Some(raw) => {
            let normalized = if let Some(rest) = raw.strip_prefix("pull:") {
                format!("pr:{rest}")
            } else if let Some(rest) = raw.strip_prefix("branch:") {
                format!("ref:{rest}")
            } else if !raw.contains(':') && raw.contains("..") {
                format!("range:{raw}")
            } else {
                raw.to_owned()
            };
            Target::parse(&normalized).map(Some)
        }
    }
}

pub fn parse_target_or_range(
    target: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
) -> Result<Option<Target>> {
    let t = target.map(str::trim).filter(|s| !s.is_empty());
    let f = from.map(str::trim).filter(|s| !s.is_empty());
    let to_ref = to.map(str::trim).filter(|s| !s.is_empty());
    if t.is_some() && (f.is_some() || to_ref.is_some()) {
        return Err(GraphError::invalid(
            "Specify either `target` or `from`/`to` range parameters, not both.",
        ));
    }
    if f.is_some() || to_ref.is_some() {
        let (Some(from_ref), Some(to_ref)) = (f, to_ref) else {
            return Err(GraphError::invalid(
                "Both `from` and `to` are required for a range.",
            ));
        };
        return Target::parse(&format!("range:{from_ref}..{to_ref}")).map(Some);
    }
    parse_target(t)
}

pub fn parse_map_params(
    offset: Option<&str>,
    limit: Option<&str>,
    depth: Option<&str>,
    lod: Option<&str>,
    min_weight: Option<&str>,
) -> Result<MapQueryParams> {
    MapQueryParams::parse_with(offset, limit, depth, lod, min_weight)
}

pub fn build_or_import(
    engine: &Engine,
    project: &str,
    root: &Path,
    rebuild: bool,
    wait: bool,
) -> Result<Value> {
    let current = engine.graphs.status(project, false)?;
    let state = current["state"].as_str().unwrap_or("off");
    if state == "ready" && !rebuild {
        return Ok(current);
    }
    if state == "building" && !rebuild {
        return if wait {
            wait_settled(&engine.graphs, project, Duration::from_secs(120))
        } else {
            Ok(current)
        };
    }
    let action = if current["enabled"] == true && (rebuild || state == "failed") {
        "rebuild"
    } else {
        "enable"
    };
    match engine.graphs.act(project, action) {
        Ok(status) => {
            if wait && (status["build"].is_object() || status["state"] == "building") {
                wait_settled(&engine.graphs, project, Duration::from_secs(120))
            } else {
                Ok(status)
            }
        }
        Err(error) => {
            if let Some(imported) = engine.ensure_graph_or_import(project, root)? {
                Ok(imported)
            } else {
                Err(error)
            }
        }
    }
}

pub fn parse_followed(
    follow: Option<&str>,
    out_list: &[String],
    into_list: &[String],
) -> Result<Option<Followed>> {
    let Some(root) = follow.map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let out_json = serde_json::to_string(out_list)?;
    let into_json = serde_json::to_string(into_list)?;
    Followed::parse(root, &out_json, &into_json)
}

pub fn parse_side(side: Option<&str>) -> Result<Option<bool>> {
    match side.map(str::trim).filter(|s| !s.is_empty()) {
        None => Ok(None),
        Some("both_use") => Ok(Some(false)),
        Some("uses_both") => Ok(Some(true)),
        Some(other) => Err(GraphError::invalid(format!(
            "Invalid `side` value `{other}`; expected `both_use` or `uses_both`."
        ))),
    }
}

pub fn wait_settled(graphs: &Graphs, project: &str, timeout: Duration) -> Result<Value> {
    let started = Instant::now();
    loop {
        let status = graphs.status(project, false)?;
        if status["build"].is_null() {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

pub fn render_explanation(raw: &Value) -> String {
    if raw["kind"] == "link" {
        let from = raw["from"].as_str().unwrap_or("");
        let to = raw["to"].as_str().unwrap_or("");
        let links = raw["links"].as_u64().unwrap_or(0);
        let guessed = raw["guessed"].as_u64().unwrap_or(0);
        let mut out = format!("Link `{from}` -> `{to}`: {links} link(s)");
        if guessed > 0 {
            out.push_str(&format!(" ({guessed} inferred by name)"));
        }
        out.push_str(".\n");
        if let Some(edges) = raw["edges"].as_array()
            && !edges.is_empty()
        {
            out.push_str("\nEdges:\n");
            for edge in edges.iter().take(24) {
                let kind = edge["kind"].as_str().unwrap_or("depends");
                let ef = edge["from"].as_str().unwrap_or("");
                let et = edge["to"].as_str().unwrap_or("");
                let mark = if edge["guessed"].as_bool() == Some(true) {
                    " [guessed]"
                } else {
                    ""
                };
                out.push_str(&format!("- `{ef}` -[{kind}]-> `{et}`{mark}\n"));
            }
        }
        return out;
    }

    let subject = raw["subject"].as_str().unwrap_or("");
    let focus = &raw["focus"];
    let mut out = format!("Subject: `{subject}`");
    if let Some(kind) = focus["kind"].as_str() {
        out.push_str(&format!(" ({kind})"));
    }
    out.push('\n');
    if let Some(uses) = focus["uses"].as_array() {
        out.push_str(&format!("\nUses ({}):\n", uses.len()));
        for row in uses.iter().take(20) {
            let p = row["path"].as_str().or_else(|| row["name"].as_str()).unwrap_or("");
            let links = row["links"].as_u64().unwrap_or(0);
            out.push_str(&format!("- `{p}` ({links})\n"));
        }
    }
    if let Some(used_by) = focus["used_by"].as_array() {
        out.push_str(&format!("\nUsed by ({}):\n", used_by.len()));
        for row in used_by.iter().take(20) {
            let p = row["path"].as_str().or_else(|| row["name"].as_str()).unwrap_or("");
            let links = row["links"].as_u64().unwrap_or(0);
            out.push_str(&format!("- `{p}` ({links})\n"));
        }
    }
    out
}

pub fn json_list_strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub fn build_sharing<'a>(
    from: &'a str,
    to: &'a str,
    users: Option<bool>,
    skip: usize,
) -> Sharing<'a> {
    Sharing {
        from,
        to,
        users,
        skip,
    }
}
