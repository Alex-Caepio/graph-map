use crate::common::{
    Engine, build_sharing, parse_followed, parse_side, parse_target, render_explanation, wait_settled,
};
use crate::{http, mcp};
use graph_map_core::{GraphError, Result, graphs::Shown};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};

pub struct GlobalOpts {
    pub project: Option<String>,
    pub graph: Option<PathBuf>,
    pub state: Option<PathBuf>,
    pub json: bool,
}

pub fn run(args: &[String]) -> Result<()> {
    if args.is_empty() || matches!(args[0].as_str(), "-h" | "--help" | "help") {
        print_help();
        return Ok(());
    }
    if matches!(args[0].as_str(), "-V" | "--version" | "version") {
        println!("graph-map {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let mut globals = GlobalOpts {
        project: None,
        graph: None,
        state: None,
        json: false,
    };
    let mut rest = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(());
            }
            "-V" | "--version" => {
                println!("graph-map {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--json" => {
                globals.json = true;
                i += 1;
            }
            "--project" | "-p" => {
                globals.project = Some(next_arg(args, &mut i, "--project")?);
            }
            "--graph" | "-g" => {
                globals.graph = Some(PathBuf::from(next_arg(args, &mut i, "--graph")?));
            }
            "--state" => {
                globals.state = Some(PathBuf::from(next_arg(args, &mut i, "--state")?));
            }
            _ => {
                rest.push(args[i].clone());
                i += 1;
            }
        }
    }

    if rest.is_empty() {
        print_help();
        return Ok(());
    }

    let engine = Arc::new(match globals.state.as_deref() {
        Some(dir) => Engine::open_at(dir)?,
        None => Engine::open_default()?,
    });

    let cmd = rest[0].as_str();
    let cmd_args = &rest[1..];
    match cmd {
        "mcp" => {
            let result = mcp::run_stdio(engine.clone());
            engine.graphs.close();
            return result;
        }
        "serve" => {
            let mut host = "127.0.0.1".to_owned();
            let mut port: u16 = 7840;
            let mut idx = 0;
            while idx < cmd_args.len() {
                match cmd_args[idx].as_str() {
                    "--host" => host = next_arg(cmd_args, &mut idx, "--host")?,
                    "--port" => {
                        let raw = next_arg(cmd_args, &mut idx, "--port")?;
                        port = raw
                            .parse::<u16>()
                            .map_err(|_| GraphError::invalid("Invalid port number."))?;
                    }
                    other => {
                        return Err(GraphError::invalid(format!(
                            "Unknown `serve` option `{other}`."
                        )));
                    }
                }
            }
            let selected = globals
                .project
                .as_deref()
                .map(|p| engine.resolve_project(Some(p), None))
                .transpose()?
                .map(|(id, _)| id);
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .map_err(|e| GraphError::unavailable(format!("Async runtime failed: {e}")))?;
            let result = rt.block_on(http::serve(engine.clone(), &host, port, selected));
            engine.graphs.close();
            return result;
        }
        _ => {}
    }

    let (project, root) = engine.resolve_project(globals.project.as_deref(), None)?;
    let graph_override = globals.graph.as_deref();

    let output = match cmd {
        "status" => {
            let check = has_flag(cmd_args, "--check");
            let mut status = engine.graphs.status(&project, check)?;
            if status["state"] == "off"
                && let Some(candidate) =
                    engine.fallback_graph_path(&project, &root, None, graph_override)?
            {
                status["fallback_graph"] = json!(candidate.to_string_lossy());
            }
            status
        }
        "build" => handle_build(&engine, &project, &root, cmd_args)?,
        "find" => {
            let (pos, flags) = split_flags(cmd_args, &["--target", "--notests", "--no-tests"]);
            let query = pos
                .first()
                .map(String::as_str)
                .ok_or_else(|| GraphError::invalid("Usage: graph-map find <query>"))?;
            let target = parse_target(flag_str(&flags, "--target"))?;
            let notests = flags.contains_key("--notests") || flags.contains_key("--no-tests");
            engine.run_find(&project, &root, target.as_ref(), query, notests, graph_override)?
        }
        "neighbors" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--scope",
                    "--target",
                    "--out",
                    "--into",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            let node = pos
                .first()
                .map(String::as_str)
                .ok_or_else(|| GraphError::invalid("Usage: graph-map neighbors <path-or-node>"))?;
            let target = parse_target(flag_str(&flags, "--target"))?;
            let out = flag_str(&flags, "--out")
                .map(|s| s.split(',').map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default();
            let into = flag_str(&flags, "--into")
                .map(|s| s.split(',').map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default();
            let followed = parse_followed(Some(node), &out, &into)?;
            let shown = Shown {
                scope: flag_str(&flags, "--scope").unwrap_or(""),
                focus: Some(node),
                followed: followed.as_ref(),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "link" => {
            let (pos, flags) =
                split_flags(cmd_args, &["--target", "--certain", "--notests", "--no-tests"]);
            if pos.len() < 2 {
                return Err(GraphError::invalid("Usage: graph-map link <from> <to>"));
            }
            let target = parse_target(flag_str(&flags, "--target"))?;
            let shown = Shown {
                link: Some((pos[0].as_str(), pos[1].as_str())),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "path" => {
            let (pos, flags) =
                split_flags(cmd_args, &["--target", "--certain", "--notests", "--no-tests"]);
            if pos.len() < 2 {
                return Err(GraphError::invalid("Usage: graph-map path <from> <to>"));
            }
            let target = parse_target(flag_str(&flags, "--target"))?;
            let shown = Shown {
                path: Some((pos[0].as_str(), pos[1].as_str())),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "shared" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--side",
                    "--skip",
                    "--target",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            if pos.len() < 2 {
                return Err(GraphError::invalid("Usage: graph-map shared <a> <b>"));
            }
            let target = parse_target(flag_str(&flags, "--target"))?;
            let users = parse_side(flag_str(&flags, "--side"))?;
            let skip = flag_str(&flags, "--skip")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let sharing = build_sharing(pos[0].as_str(), pos[1].as_str(), users, skip);
            let shown = Shown {
                shared: Some(sharing),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "cycles" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--scope",
                    "--target",
                    "--base",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            let cycle = pos
                .first()
                .map(String::as_str)
                .ok_or_else(|| GraphError::invalid("Usage: graph-map cycles <node-or-path>"))?;
            let target = parse_target(flag_str(&flags, "--target"))?;
            let shown = Shown {
                scope: flag_str(&flags, "--scope").unwrap_or(""),
                cycle: Some(cycle),
                base: flag_str(&flags, "--base"),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "impact" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--target",
                    "--scope",
                    "--focus",
                    "--base",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            let target_raw = flag_str(&flags, "--target")
                .or_else(|| pos.first().map(String::as_str))
                .unwrap_or("work");
            let target = parse_target(Some(target_raw))?;
            let shown = Shown {
                scope: flag_str(&flags, "--scope").unwrap_or(""),
                focus: flag_str(&flags, "--focus"),
                base: flag_str(&flags, "--base"),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "layers" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--scope",
                    "--pattern",
                    "--modules",
                    "--target",
                    "--base",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            let target = parse_target(flag_str(&flags, "--target"))?;
            let pattern = flag_str(&flags, "--pattern").or_else(|| pos.first().map(String::as_str));
            let modules = flags.contains_key("--modules");
            let shown = Shown {
                scope: flag_str(&flags, "--scope").unwrap_or(""),
                patterns: pattern.is_none() && !modules,
                pattern,
                modules,
                base: flag_str(&flags, "--base"),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "scenario" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--skip",
                    "--target",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            let entry = pos
                .first()
                .map(String::as_str)
                .ok_or_else(|| GraphError::invalid("Usage: graph-map scenario <entry>"))?;
            let target = parse_target(flag_str(&flags, "--target"))?;
            let skip = flag_str(&flags, "--skip")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let shown = Shown {
                scenario: Some((entry, skip)),
                certain: flags.contains_key("--certain"),
                notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                ..Shown::default()
            };
            engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
        }
        "test-reach" => {
            let (pos, flags) = split_flags(cmd_args, &["--target", "--certain"]);
            let target_raw = flag_str(&flags, "--target")
                .or_else(|| pos.first().map(String::as_str))
                .unwrap_or("work");
            let target = parse_target(Some(target_raw))?;
            let shown = Shown {
                certain: flags.contains_key("--certain"),
                ..Shown::default()
            };
            let view = engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?;
            json!({
                "mode": view["mode"],
                "target": view["target"],
                "chosen": view["chosen"],
                "change": view["change"],
                "test_reach": view["test_reach"],
            })
        }
        "hotspots" => {
            let (_pos, flags) =
                split_flags(cmd_args, &["--scope", "--branch", "--query", "--skip"]);
            let shown = Shown {
                scope: flag_str(&flags, "--scope").unwrap_or(""),
                history: true,
                ..Shown::default()
            };
            let view = engine.run_map(&project, &root, None, &shown, graph_override)?;
            let skip = flag_str(&flags, "--skip")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(0);
            let commits = engine.graphs.commits(
                &project,
                flag_str(&flags, "--branch").unwrap_or(""),
                flag_str(&flags, "--query").unwrap_or(""),
                skip,
            )?;
            json!({
                "map": view,
                "commits": commits,
            })
        }
        "runs" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &[
                    "--launch",
                    "--compare",
                    "--certain",
                    "--notests",
                    "--no-tests",
                ],
            );
            if let Some(session_id) = pos.first() {
                let target = parse_target(Some(&format!("run:{session_id}")))?;
                let shown = Shown {
                    launch: flag_str(&flags, "--launch"),
                    compare_launch: flag_str(&flags, "--compare"),
                    certain: flags.contains_key("--certain"),
                    notests: flags.contains_key("--notests") || flags.contains_key("--no-tests"),
                    ..Shown::default()
                };
                engine.run_map(&project, &root, target.as_ref(), &shown, graph_override)?
            } else {
                engine.graphs.list_runs(&project)?
            }
        }
        "explain" => {
            let (pos, flags) = split_flags(
                cmd_args,
                &["--target", "--to", "--certain", "--notests", "--no-tests"],
            );
            let from = pos
                .first()
                .map(String::as_str)
                .ok_or_else(|| GraphError::invalid("Usage: graph-map explain <from> [--to <to>]"))?;
            let to = flag_str(&flags, "--to").or_else(|| pos.get(1).map(String::as_str));
            let kind = if to.is_some() { "link" } else { "node" };
            let target = parse_target(flag_str(&flags, "--target"))?;
            let raw = engine.graphs.explain(
                &project,
                target.as_ref(),
                kind,
                from,
                to,
                flags.contains_key("--certain"),
                flags.contains_key("--notests") || flags.contains_key("--no-tests"),
            )?;
            json!({
                "raw": raw,
                "answer": render_explanation(&raw),
            })
        }
        other => {
            return Err(GraphError::invalid(format!(
                "Unknown subcommand `{other}`. Run `graph-map --help`."
            )));
        }
    };

    engine.graphs.close();
    if globals.json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print_human(cmd, &output);
    }
    Ok(())
}

fn handle_build(engine: &Engine, project: &str, root: &Path, args: &[String]) -> Result<Value> {
    let (_pos, flags) = split_flags(
        args,
        &[
            "--import",
            "--rebuild",
            "--no-wait",
            "--offer",
            "--withhold",
            "--delete",
        ],
    );
    if let Some(import_path) = flag_str(&flags, "--import") {
        return engine
            .graphs
            .import_graph(project, Path::new(import_path));
    }
    if flags.contains_key("--delete") {
        return engine.graphs.act(project, "delete");
    }
    if flags.contains_key("--offer") {
        return engine.graphs.act(project, "offer");
    }
    if flags.contains_key("--withhold") {
        return engine.graphs.act(project, "withhold");
    }

    let wait = !flags.contains_key("--no-wait");
    let current = engine.graphs.status(project, false)?;
    let action = if current["enabled"] == true && flags.contains_key("--rebuild") {
        "rebuild"
    } else {
        "enable"
    };
    if current["enabled"] == true && !flags.contains_key("--rebuild") {
        return Ok(current);
    }
    match engine.graphs.act(project, action) {
        Ok(status) => {
            if wait && status["build"].is_object() {
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

fn next_arg(args: &[String], index: &mut usize, name: &str) -> Result<String> {
    if *index + 1 >= args.len() {
        return Err(GraphError::invalid(format!("Missing value for {name}.")));
    }
    let val = args[*index + 1].clone();
    *index += 2;
    Ok(val)
}

fn has_flag(args: &[String], name: &str) -> bool {
    args.iter().any(|arg| arg == name)
}

fn split_flags(
    args: &[String],
    known_flags: &[&str],
) -> (Vec<String>, std::collections::BTreeMap<String, Option<String>>) {
    let value_flags = [
        "--target", "--scope", "--out", "--into", "--side", "--skip", "--base", "--focus",
        "--pattern", "--branch", "--query", "--launch", "--compare", "--import", "--to",
    ];
    let mut pos = Vec::new();
    let mut flags = std::collections::BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if let Some((k, v)) = arg.split_once('=')
            && known_flags.contains(&k)
        {
            flags.insert(k.to_owned(), Some(v.to_owned()));
            i += 1;
            continue;
        }
        if known_flags.contains(&arg.as_str()) {
            if value_flags.contains(&arg.as_str()) && i + 1 < args.len() {
                flags.insert(arg.clone(), Some(args[i + 1].clone()));
                i += 2;
            } else {
                flags.insert(arg.clone(), None);
                i += 1;
            }
            continue;
        }
        pos.push(arg.clone());
        i += 1;
    }
    (pos, flags)
}

fn flag_str<'a>(
    flags: &'a std::collections::BTreeMap<String, Option<String>>,
    key: &str,
) -> Option<&'a str> {
    flags
        .get(key)
        .and_then(|opt| opt.as_deref())
        .filter(|s| !s.is_empty())
}

fn print_human(cmd: &str, value: &Value) {
    match cmd {
        "status" | "build" => {
            println!("state: {}", value["state"].as_str().unwrap_or("unknown"));
            if let Some(g) = value["graph"].as_object() {
                println!(
                    "graph: {} nodes, {} edges (stale={})",
                    g.get("nodes").and_then(Value::as_u64).unwrap_or(0),
                    g.get("edges").and_then(Value::as_u64).unwrap_or(0),
                    value["stale"].as_bool().unwrap_or(false)
                );
            } else if let Some(fb) = value["fallback_graph"].as_str() {
                println!("fallback_graph: {fb}");
            }
            if let Some(msg) = value["failure"]["message"].as_str() {
                println!("failure: {msg}");
            }
        }
        "explain" => {
            println!("{}", value["answer"].as_str().unwrap_or(""));
        }
        _ => {
            println!(
                "{}",
                serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string())
            );
        }
    }
}

fn print_help() {
    println!(
        "graph-map {} — Project, Commits & Current Work Graph Map

USAGE:
  graph-map [--project <dir>] [--graph <graph.json>] [--state <dir>] [--json] <command> [args]

COMMANDS:
  status [--check]                   Show graph availability, counts, and staleness
  build [--import <path>] [--rebuild] Build, rebuild, or import graph.json
  find <query> [--target <t>]        Search files, folders, classes, and functions
  neighbors <node> [--out ...]       Inspect callers, callees, and step-by-step unfolding
  link <from> <to>                   Inspect symbol edges between two places
  path <from> <to>                   Find shortest directed path between two places
  shared <a> <b> [--side ...]        Show shared dependencies or shared callers
  cycles <node>                      Trace shortest cycle back to an origin
  impact [<target>]                  Map changes (work|uncommitted|commit:..|branch:..|pull:..|run:..)
  layers [<pattern>] [--modules]     Inspect architectural layers, patterns, or modules
  scenario <entry> [--skip <n>]      Trace downstream call scenario from an entry point
  test-reach [<target>]              Find shortest incoming paths from tests to changed files
  hotspots [--scope <s>] [--branch]  Analyze git co-change hotspots and commit history
  runs [<session-id>]                List or audit recorded Claude/Codex/Harness runs
  explain <from> [--to <to>]         Render markdown summary for a node or link
  serve [--host <ip>] [--port <n>]   Start standalone localhost Web UI + HTTP API
  mcp                                Run stdio JSON-RPC 2.0 MCP server (14 tools)

OPTIONS:
  -h, --help                         Show this help message
  -V, --version                      Print version information",
        env!("CARGO_PKG_VERSION")
    );
}
