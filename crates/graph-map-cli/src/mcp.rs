use crate::common::{
    Engine, build_or_import, build_sharing, json_list_strings, parse_followed, parse_map_params,
    parse_side, parse_target, parse_target_or_range,
};
use graph_map_core::{GraphError, MapQueryParams, Result, graphs::Shown};
use serde_json::{Map, Value, json};
use std::{
    io::{self, BufRead, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

const PROTOCOL_VERSION: &str = "2025-06-18";

pub fn run_stdio(engine: Arc<Engine>) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();
    let mut line = String::new();
    let mut default_root: Option<PathBuf> = None;

    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parsed: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(error) => {
                let err = jsonrpc_error(Value::Null, -32700, &format!("Parse error: {error}"));
                write_frame(&mut writer, &err)?;
                continue;
            }
        };
        if let Some(reply) = handle_message(&engine, &mut default_root, &parsed) {
            write_frame(&mut writer, &reply)?;
        }
    }
    Ok(())
}

pub fn handle_message(
    engine: &Engine,
    default_root: &mut Option<PathBuf>,
    msg: &Value,
) -> Option<Value> {
    let id = msg.get("id").cloned();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return id.map(|req_id| jsonrpc_error(req_id, -32600, "Invalid JSON-RPC request."));
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // Notifications without an id never receive a response.
    if id.is_none() || matches!(id, Some(Value::Null)) {
        return None;
    }
    let req_id = id.unwrap_or(Value::Null);

    let result = match method {
        "initialize" => {
            if let Some(detected) = extract_client_root(&params) {
                *default_root = Some(detected);
            }
            Ok(json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {
                    "tools": { "listChanged": false }
                },
                "serverInfo": {
                    "name": "graph-map",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_definitions() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let arguments = params
                .get("arguments")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            match dispatch_tool(engine, default_root.as_deref(), name, &arguments) {
                Ok(payload) => Ok(tool_success(payload)),
                Err(error) => Ok(tool_failure(&error.message)),
            }
        }
        other => Err(GraphError::missing(format!("Unknown method: {other}"))),
    };

    Some(match result {
        Ok(value) => json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": value,
        }),
        Err(error) => jsonrpc_error(req_id, -32601, &error.message),
    })
}

fn extract_client_root(params: &Value) -> Option<PathBuf> {
    let uri = params
        .get("workspaceFolders")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|item| item.get("uri"))
        .and_then(Value::as_str)
        .or_else(|| params.get("rootUri").and_then(Value::as_str));

    if let Some(raw_uri) = uri
        && let Some(path_str) = raw_uri.strip_prefix("file://")
    {
        let decoded = percent_decode(path_str);
        let p = PathBuf::from(decoded);
        if p.is_dir() {
            return Some(p);
        }
    }
    if let Some(root_path) = params.get("rootPath").and_then(Value::as_str) {
        let p = PathBuf::from(root_path);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(hex) = std::str::from_utf8(&bytes[i + 1..i + 3])
            && let Ok(val) = u8::from_str_radix(hex, 16)
        {
            out.push(val);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn write_frame(writer: &mut impl Write, payload: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, payload)?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn jsonrpc_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        }
    })
}

fn tool_success(data: Value) -> Value {
    let text = serde_json::to_string_pretty(&data).unwrap_or_else(|_| data.to_string());
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": data,
        "isError": false,
    })
}

fn tool_failure(message: &str) -> Value {
    json!({
        "content": [{ "type": "text", "text": message }],
        "isError": true,
    })
}

fn arg_str<'a>(args: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn arg_bool(args: &Map<String, Value>, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn arg_usize(args: &Map<String, Value>, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(Value::as_u64)
        .map(|n| n as usize)
        .unwrap_or(default)
}

fn arg_num_str(args: &Map<String, Value>, key: &str) -> Option<String> {
    match args.get(key) {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_owned()),
        _ => None,
    }
}

fn arg_map_params(args: &Map<String, Value>) -> Result<MapQueryParams> {
    let offset = arg_num_str(args, "offset");
    let limit = arg_num_str(args, "limit");
    let depth = arg_num_str(args, "depth");
    let min_weight = arg_num_str(args, "min_weight");
    let lod = arg_str(args, "lod");
    parse_map_params(
        offset.as_deref(),
        limit.as_deref(),
        depth.as_deref(),
        lod,
        min_weight.as_deref(),
    )
}

fn dispatch_tool(
    engine: &Engine,
    default_root: Option<&Path>,
    name: &str,
    args: &Map<String, Value>,
) -> Result<Value> {
    let (project, root) = engine.resolve_mcp_project(arg_str(args, "project"), default_root)?;
    let graph_override = arg_str(args, "graph").map(PathBuf::from);
    let certain = arg_bool(args, "certain", false);
    let notests = arg_bool(args, "notests", false);
    let whole = arg_bool(args, "whole", false);
    let no_incoming = arg_bool(args, "no_incoming", false);
    let focus_session = arg_str(args, "focus_session");
    let agent = arg_str(args, "agent");
    let map_params = arg_map_params(args)?;
    let parse_target_with_default = |default_target: Option<&str>| -> Result<Option<_>> {
        if args.get("from").is_some() || args.get("to").is_some() {
            parse_target_or_range(
                arg_str(args, "target"),
                arg_str(args, "from"),
                arg_str(args, "to"),
            )
        } else {
            parse_target(arg_str(args, "target").or(default_target))
        }
    };

    match name {
        "graph_status" => {
            let check = arg_bool(args, "check", false);
            let mut status = engine.graphs.status(&project, check)?;
            if status["state"] == "off"
                && let Some(candidate) = engine
                    .fallback_graph_path(&project, &root, None, graph_override.as_deref())?
            {
                status["fallback_graph"] = json!(candidate.to_string_lossy());
            }
            Ok(status)
        }
        "graph_build" => {
            if let Some(import_path) = arg_str(args, "import") {
                return engine.graphs.import_graph(&project, Path::new(import_path));
            }
            let rebuild = arg_bool(args, "rebuild", false);
            let wait = arg_bool(args, "wait", true);
            build_or_import(engine, &project, &root, rebuild, wait)
        }
        "graph_find" => {
            let query = arg_str(args, "query")
                .ok_or_else(|| GraphError::invalid("Missing required `query` parameter."))?;
            let target = parse_target(arg_str(args, "target"))?;
            engine.run_find(
                &project,
                &root,
                target.as_ref(),
                query,
                notests,
                graph_override.as_deref(),
            )
        }
        "graph_neighbors" => {
            let node = arg_str(args, "node")
                .ok_or_else(|| GraphError::invalid("Missing required `node` parameter."))?;
            let target = parse_target_with_default(None)?;
            let scope = arg_str(args, "scope").unwrap_or("");
            let out_list = json_list_strings(args.get("out"));
            let into_list = json_list_strings(args.get("into"));
            let followed = parse_followed(Some(node), &out_list, &into_list)?;
            let shown = Shown {
                scope,
                focus: Some(node),
                focus_session,
                agent,
                followed: followed.as_ref(),
                certain,
                notests,
                no_incoming,
                whole,
                offset: map_params.offset,
                limit: map_params.limit,
                depth: map_params.depth,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_link" => {
            let target = parse_target(arg_str(args, "target"))?;
            let from = arg_str(args, "from")
                .ok_or_else(|| GraphError::invalid("Missing required `from` parameter."))?;
            let to = arg_str(args, "to")
                .ok_or_else(|| GraphError::invalid("Missing required `to` parameter."))?;
            let shown = Shown {
                link: Some((from, to)),
                base: arg_str(args, "base"),
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_path" => {
            let target = parse_target(arg_str(args, "target"))?;
            let from = arg_str(args, "from")
                .ok_or_else(|| GraphError::invalid("Missing required `from` parameter."))?;
            let to = arg_str(args, "to")
                .ok_or_else(|| GraphError::invalid("Missing required `to` parameter."))?;
            let longer = arg_usize(args, "longer", 0);
            let shown = Shown {
                path: Some((from, to)),
                longer,
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_shared" => {
            let target = parse_target(arg_str(args, "target"))?;
            let from = arg_str(args, "from")
                .ok_or_else(|| GraphError::invalid("Missing required `from` parameter."))?;
            let to = arg_str(args, "to")
                .ok_or_else(|| GraphError::invalid("Missing required `to` parameter."))?;
            let users = parse_side(arg_str(args, "side"))?;
            let skip = arg_usize(args, "skip", 0);
            let sharing = build_sharing(from, to, users, skip);
            let shown = Shown {
                shared: Some(sharing),
                certain,
                notests,
                offset: map_params.offset,
                limit: map_params.limit,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_cycle" => {
            let target = parse_target(arg_str(args, "target"))?;
            let cycle = arg_str(args, "cycle")
                .ok_or_else(|| GraphError::invalid("Missing required `cycle` parameter."))?;
            let scope = arg_str(args, "scope").unwrap_or("");
            let base = arg_str(args, "base");
            let shown = Shown {
                scope,
                cycle: Some(cycle),
                base,
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_change_impact" => {
            let target = parse_target_with_default(Some("work"))?;
            if arg_bool(args, "catalog_only", false) {
                return engine.graphs.changes_catalog_with_agent(
                    &project,
                    target.as_ref(),
                    focus_session,
                    agent,
                    notests,
                );
            }
            let scope = arg_str(args, "scope").unwrap_or("");
            let focus = arg_str(args, "focus");
            let base = arg_str(args, "base");
            let shown = Shown {
                scope,
                focus,
                focus_session,
                agent,
                base,
                certain,
                notests,
                whole,
                offset: map_params.offset,
                limit: map_params.limit,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_architecture" => {
            let target = parse_target_with_default(None)?;
            let scope = arg_str(args, "scope").unwrap_or("");
            let pattern = arg_str(args, "pattern");
            let base = arg_str(args, "base");
            let modules = arg_bool(args, "modules", false);
            let shown = Shown {
                scope,
                patterns: pattern.is_none() && !modules,
                pattern,
                modules,
                no_incoming,
                base,
                certain,
                notests,
                whole,
                offset: map_params.offset,
                limit: map_params.limit,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_scenario" => {
            let target = parse_target(arg_str(args, "target"))?;
            let entry = arg_str(args, "entry")
                .ok_or_else(|| GraphError::invalid("Missing required `entry` parameter."))?;
            let skip = arg_usize(args, "skip", 0);
            let shown = Shown {
                scenario: Some((entry, skip)),
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_test_reach" => {
            let target = parse_target_with_default(Some("work"))?;
            let shown = Shown {
                certain,
                notests: false,
                ..Shown::default()
            };
            let view = engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )?;
            Ok(json!({
                "mode": view["mode"],
                "target": view["target"],
                "chosen": view["chosen"],
                "change": view["change"],
                "test_reach": view["test_reach"],
            }))
        }
        "graph_history_hotspots" => {
            let target = parse_target(arg_str(args, "target"))?;
            let scope = arg_str(args, "scope").unwrap_or("");
            let branch = arg_str(args, "branch").unwrap_or("");
            let query = arg_str(args, "query").unwrap_or("");
            let skip = arg_usize(args, "skip", 0);
            let shown = Shown {
                scope,
                history: true,
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            let view = engine.run_map(
                &project,
                &root,
                target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )?;
            let commits = engine.graphs.commits(&project, branch, query, skip)?;
            Ok(json!({
                "map": view,
                "commits": commits,
            }))
        }
        "graph_run_audit" => {
            let Some(session) = arg_str(args, "session") else {
                return engine.graphs.list_runs(&project);
            };
            let run_target = parse_target(Some(&format!("run:{session}")))?;
            let launch = arg_str(args, "launch");
            let compare_launch = arg_str(args, "compare_launch");
            let shown = Shown {
                launch,
                compare_launch,
                certain,
                notests,
                lod: map_params.lod,
                min_weight: map_params.min_weight,
                ..Shown::default()
            };
            engine.run_map(
                &project,
                &root,
                run_target.as_ref(),
                &shown,
                graph_override.as_deref(),
            )
        }
        "graph_file_impact" => {
            let path = arg_str(args, "path")
                .ok_or_else(|| GraphError::invalid("Missing required `path` parameter."))?;
            let target = parse_target_with_default(Some("work"))?;
            engine.graphs.file_impact_with_agent(
                &project,
                path,
                target.as_ref(),
                focus_session,
                agent,
                certain,
            )
        }
        "graph_memory_links" => {
            if let Some(files) = args.get("files") {
                let raw = files
                    .as_array()
                    .ok_or_else(|| GraphError::invalid("`files` must be a JSON array."))?;
                let normalized: Vec<Value> = raw
                    .iter()
                    .map(|item| match item {
                        Value::String(s) => json!({ "path": s }),
                        other => other.clone(),
                    })
                    .collect();
                return engine
                    .graphs
                    .memory_links_batch(&project, &json!({ "files": normalized }));
            }
            let path = arg_str(args, "path")
                .ok_or_else(|| GraphError::invalid("Missing `path` or `files` parameter."))?;
            let mut query = json!({ "path": path });
            if let Some(prev) = arg_str(args, "previous_path") {
                query["previous_path"] = json!(prev);
            }
            engine.graphs.memory_links(&project, &query)
        }
        unknown => Err(GraphError::missing(format!("Unknown MCP tool: {unknown}"))),
    }
}

fn tool_def(
    name: &str,
    description: &str,
    read_only: bool,
    props: Value,
    required: &[&str],
) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": props,
            "required": required,
            "additionalProperties": false,
        },
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": !read_only,
            "openWorldHint": false,
        }
    })
}

pub fn tool_definitions() -> Vec<Value> {
    let project_prop = || {
        json!({
            "type": "string",
            "description": "Repository root path (defaults to current working directory when not inside a plugin cache)."
        })
    };
    let graph_prop = || {
        json!({
            "type": "string",
            "description": "Optional explicit path to a node-link graph.json override."
        })
    };
    let target_prop = || {
        json!({
            "type": "string",
            "description": "Optional target: 'work', 'uncommitted', 'commit:<sha>', 'ref:<name>', 'pr:<num>', 'range:<from>..<to>' (or '<from>..<to>'), or 'run:<session-id>'."
        })
    };
    let lod_prop = || {
        json!({
            "type": "string",
            "enum": ["overview", "folder", "file", "symbol"],
            "description": "Progressive level-of-detail slice."
        })
    };
    let min_weight_prop = || {
        json!({
            "type": "integer",
            "minimum": 1,
            "description": "Minimum edge link weight threshold to retain."
        })
    };

    vec![
        tool_def(
            "graph_status",
            "Check code graph availability, node/edge counts, backend, staleness, and active build progress for a project.",
            true,
            json!({
                "project": project_prop(),
                "graph": graph_prop(),
                "check": { "type": "boolean", "description": "Whether to check git/workspace staleness." }
            }),
            &[],
        ),
        tool_def(
            "graph_build",
            "Enable, build (native 13-language tree-sitter AST + SQLite IR by default, or graphify), rebuild, or import a node-link code graph.",
            false,
            json!({
                "project": project_prop(),
                "import": { "type": "string", "description": "Path to an existing node-link graph.json file to validate and import." },
                "rebuild": { "type": "boolean", "description": "Force a full rebuild when already enabled." },
                "wait": { "type": "boolean", "description": "Wait for the background build to finish (default true)." }
            }),
            &[],
        ),
        tool_def(
            "graph_find",
            "Search files, folders, classes, functions, and methods in the code graph index.",
            true,
            json!({
                "project": project_prop(),
                "query": { "type": "string", "description": "Search term matching paths or symbols." },
                "target": target_prop(),
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["query"],
        ),
        tool_def(
            "graph_neighbors",
            "Inspect what a folder, file, or symbol uses and what depends on it, unfolding step by step with optional depth and LOD.",
            true,
            json!({
                "project": project_prop(),
                "node": { "type": "string", "description": "File path, folder path, or symbol identifier to inspect." },
                "scope": { "type": "string", "description": "Optional folder scope." },
                "out": { "type": "array", "items": { "type": "string" }, "description": "Places unfolded along outgoing uses." },
                "into": { "type": "array", "items": { "type": "string" }, "description": "Places unfolded along incoming dependents." },
                "depth": { "type": "integer", "minimum": 1, "maximum": 6, "description": "Neighborhood hop depth (1..6)." },
                "no_incoming": { "type": "boolean", "description": "Highlight nodes with no incoming dependents." },
                "target": target_prop(),
                "from": { "type": "string", "description": "Range start commit/branch/tag (used with 'to')." },
                "to": { "type": "string", "description": "Range end commit/branch/tag (used with 'from')." },
                "focus_session": { "type": "string" },
                "agent": { "type": "string" },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "whole": { "type": "boolean" },
                "offset": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["node"],
        ),
        tool_def(
            "graph_link",
            "Inspect every symbol-level edge connecting two files or folders.",
            true,
            json!({
                "project": project_prop(),
                "from": { "type": "string" },
                "to": { "type": "string" },
                "target": target_prop(),
                "base": { "type": "string" },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["from", "to"],
        ),
        tool_def(
            "graph_path",
            "Find the shortest directed dependency paths between two files or folders.",
            true,
            json!({
                "project": project_prop(),
                "from": { "type": "string" },
                "to": { "type": "string" },
                "longer": { "type": "integer", "minimum": 0, "description": "Include near-shortest paths up to N extra hops." },
                "target": target_prop(),
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["from", "to"],
        ),
        tool_def(
            "graph_shared",
            "Discover dependencies both places use or callers that depend on both places.",
            true,
            json!({
                "project": project_prop(),
                "from": { "type": "string" },
                "to": { "type": "string" },
                "side": { "type": "string", "enum": ["both_use", "uses_both"] },
                "skip": { "type": "integer", "minimum": 0 },
                "offset": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "target": target_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["from", "to"],
        ),
        tool_def(
            "graph_cycle",
            "Trace the shortest strongly connected cycle back to a file, folder, or layer.",
            true,
            json!({
                "project": project_prop(),
                "cycle": { "type": "string", "description": "Origin path or layer node to trace cycles from." },
                "scope": { "type": "string" },
                "target": target_prop(),
                "base": { "type": "string" },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["cycle"],
        ),
        tool_def(
            "graph_change_impact",
            "Map uncommitted changes, current branch work, commit diffs, commit ranges, pull requests, or agent runs against the code graph.",
            true,
            json!({
                "project": project_prop(),
                "target": target_prop(),
                "from": { "type": "string", "description": "Range start commit/branch/tag (used with 'to')." },
                "to": { "type": "string", "description": "Range end commit/branch/tag (used with 'from')." },
                "scope": { "type": "string" },
                "focus": { "type": "string" },
                "focus_session": { "type": "string", "description": "Optional live agent session ID to overlay prompt/edited targets." },
                "agent": { "type": "string", "description": "Optional agent filter." },
                "base": { "type": "string" },
                "catalog_only": { "type": "boolean", "description": "Return compact changed-files catalog and symbol diffs without full map layout." },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "whole": { "type": "boolean" },
                "offset": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &[],
        ),
        tool_def(
            "graph_architecture",
            "Audit architectural layers, design pattern detections, module boundaries, root nodes, or custom .graph-map/layers rules.",
            true,
            json!({
                "project": project_prop(),
                "scope": { "type": "string" },
                "pattern": { "type": "string", "description": "Optional pattern key (e.g. 'layers@') or community filter." },
                "modules": { "type": "boolean", "description": "Show graph community modules view instead of layer patterns." },
                "no_incoming": { "type": "boolean", "description": "Mark nodes without incoming callers." },
                "target": target_prop(),
                "from": { "type": "string", "description": "Range start commit/branch/tag (used with 'to')." },
                "to": { "type": "string", "description": "Range end commit/branch/tag (used with 'from')." },
                "base": { "type": "string" },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "whole": { "type": "boolean" },
                "offset": { "type": "integer", "minimum": 0 },
                "limit": { "type": "integer", "minimum": 1, "maximum": 500 },
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &[],
        ),
        tool_def(
            "graph_scenario",
            "Trace the directed downstream call/dependency graph forward from a scenario entry point.",
            true,
            json!({
                "project": project_prop(),
                "entry": { "type": "string", "description": "Entrypoint file path or symbol key." },
                "skip": { "type": "integer", "minimum": 0 },
                "target": target_prop(),
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &["entry"],
        ),
        tool_def(
            "graph_test_reach",
            "Compute shortest transitive incoming paths from test files to changed source files for a target or range.",
            true,
            json!({
                "project": project_prop(),
                "target": target_prop(),
                "from": { "type": "string", "description": "Range start commit/branch/tag (used with 'to')." },
                "to": { "type": "string", "description": "Range end commit/branch/tag (used with 'from')." },
                "certain": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &[],
        ),
        tool_def(
            "graph_history_hotspots",
            "Analyze Git co-change hotspots and browse commit history for the repository.",
            true,
            json!({
                "project": project_prop(),
                "scope": { "type": "string" },
                "branch": { "type": "string" },
                "query": { "type": "string" },
                "skip": { "type": "integer", "minimum": 0 },
                "target": target_prop(),
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &[],
        ),
        tool_def(
            "graph_run_audit",
            "List or audit recorded Claude Code, Codex, and Harness agent runs against what their changes depended on vs what they opened.",
            true,
            json!({
                "project": project_prop(),
                "session": { "type": "string", "description": "Optional run/session ID to inspect; omit to list runs." },
                "launch": { "type": "string" },
                "compare_launch": { "type": "string" },
                "lod": lod_prop(),
                "min_weight": min_weight_prop(),
                "certain": { "type": "boolean" },
                "notests": { "type": "boolean" },
                "graph": graph_prop()
            }),
            &[],
        ),
        tool_def(
            "graph_file_impact",
            "Inspect blast radius, symbol diffs, callers/callees, and reaching tests for a single file.",
            true,
            json!({
                "project": project_prop(),
                "path": { "type": "string", "description": "Repository-relative path of the file to analyze." },
                "target": target_prop(),
                "from": { "type": "string", "description": "Range start commit/branch/tag (used with 'to')." },
                "to": { "type": "string", "description": "Range end commit/branch/tag (used with 'from')." },
                "focus_session": { "type": "string" },
                "agent": { "type": "string" },
                "certain": { "type": "boolean" }
            }),
            &["path"],
        ),
        tool_def(
            "graph_memory_links",
            "Look up Project Brain decisions/findings and Memory Bank chunks referencing one file or a batch of files.",
            true,
            json!({
                "project": project_prop(),
                "path": { "type": "string", "description": "Single repository-relative file path to query." },
                "previous_path": { "type": "string", "description": "Optional previous path before rename." },
                "files": {
                    "type": "array",
                    "description": "Batch list of file paths (strings or objects with 'path' and optional 'previous_path').",
                    "items": {
                        "anyOf": [
                            { "type": "string" },
                            {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string" },
                                    "previous_path": { "type": "string" }
                                },
                                "required": ["path"]
                            }
                        ]
                    }
                }
            }),
            &[],
        ),
    ]
}
