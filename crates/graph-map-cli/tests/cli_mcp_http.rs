use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.test")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.test")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn setup_repo(dir: &Path) {
    fs::create_dir_all(dir.join("src/Controller")).unwrap();
    fs::create_dir_all(dir.join("src/Service")).unwrap();
    fs::create_dir_all(dir.join("src/Entity")).unwrap();
    fs::create_dir_all(dir.join("tests")).unwrap();
    fs::create_dir_all(dir.join("graphify-out")).unwrap();
    fs::create_dir_all(dir.join(".graph-map/runs")).unwrap();

    fs::write(dir.join("src/Controller/OrderController.php"), "<?php // controller\n").unwrap();
    fs::write(dir.join("src/Service/OrderService.php"), "<?php // service\n").unwrap();
    fs::write(dir.join("src/Entity/Order.php"), "<?php // order\n").unwrap();
    fs::write(dir.join("src/Entity/Price.php"), "<?php // price\n").unwrap();
    fs::write(dir.join("tests/OrderTest.php"), "<?php // test\n").unwrap();

    let graph = json!({
        "nodes": [
            {"id": "ctrl", "label": "OrderController", "source_file": "src/Controller/OrderController.php", "file_type": "code"},
            {"id": "svc", "label": "OrderService", "source_file": "src/Service/OrderService.php", "file_type": "code"},
            {"id": "ord", "label": "Order", "source_file": "src/Entity/Order.php", "file_type": "code"},
            {"id": "prc", "label": "Price", "source_file": "src/Entity/Price.php", "file_type": "code"},
            {"id": "tst", "label": "OrderTest", "source_file": "tests/OrderTest.php", "file_type": "test"}
        ],
        "links": [
            {"source": "ctrl", "target": "svc", "relation": "calls", "confidence": "EXTRACTED", "source_file": "src/Controller/OrderController.php"},
            {"source": "svc", "target": "ord", "relation": "calls", "confidence": "EXTRACTED", "source_file": "src/Service/OrderService.php"},
            {"source": "ord", "target": "prc", "relation": "calls", "confidence": "EXTRACTED", "source_file": "src/Entity/Order.php"},
            {"source": "ord", "target": "svc", "relation": "calls", "confidence": "EXTRACTED", "source_file": "src/Entity/Order.php"},
            {"source": "tst", "target": "ctrl", "relation": "calls", "confidence": "EXTRACTED", "source_file": "tests/OrderTest.php"}
        ]
    });
    fs::write(
        dir.join("graphify-out/graph.json"),
        serde_json::to_vec_pretty(&graph).unwrap(),
    )
    .unwrap();

    fs::write(
        dir.join(".graph-map/runs/run-1.json"),
        serde_json::to_vec_pretty(&json!({
            "session": {
                "id": "run-1",
                "title": "Edit order",
                "project_path": dir.to_string_lossy(),
                "provider": "claude",
                "status": "completed"
            },
            "events": [
                {"id": 1, "kind": "tool", "tool": "Read", "state": "started", "call": "c1", "path": "src/Entity/Order.php"},
                {"id": 2, "kind": "tool", "tool": "Read", "state": "completed", "call": "c1", "ok": true},
                {"id": 3, "kind": "tool", "tool": "Edit", "state": "started", "call": "c2", "path": "src/Entity/Order.php"},
                {"id": 4, "kind": "tool", "tool": "Edit", "state": "completed", "call": "c2", "ok": true},
                {"id": 5, "kind": "tool", "tool": "Shell", "state": "completed", "ok": true, "exit_code": 0, "detail": "sed -n '1p' src/Service/OrderService.php"}
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    git(dir, &["init", "-q", "--initial-branch=main"]);
    git(dir, &["add", "--all"]);
    git(dir, &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "initial"]);

    // Uncommitted change so impact & test-reach have live diff.
    fs::write(dir.join("src/Entity/Order.php"), "<?php // order updated\n").unwrap();
}

fn run_cli(state: &Path, repo: &Path, args: &[&str]) -> Value {
    let bin = env!("CARGO_BIN_EXE_graph-map");
    let out = Command::new(bin)
        .arg("--state")
        .arg(state)
        .arg("--project")
        .arg(repo)
        .arg("--json")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cli {args:?} failed: stdout={}, stderr={}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn cli_and_mcp_14_tools_and_plugin_cwd_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let state = tmp.path().join("state");
    setup_repo(&repo);

    let bin = env!("CARGO_BIN_EXE_graph-map");

    let help = Command::new(bin).arg("--help").output().unwrap();
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("graph-map"));

    // Import graph and test all CLI subcommands cleanly.
    let imported = run_cli(&state, &repo, &["build", "--import", repo.join("graphify-out/graph.json").to_str().unwrap()]);
    assert_eq!(imported["state"], "ready");

    let status = run_cli(&state, &repo, &["status", "--check"]);
    assert_eq!(status["state"], "ready");
    assert_eq!(status["graph"]["nodes"], 5);

    let found = run_cli(&state, &repo, &["find", "Order"]);
    assert!(found["found"].as_u64().unwrap() > 0);

    let neigh = run_cli(&state, &repo, &["neighbors", "src/Service/OrderService.php"]);
    assert_eq!(neigh["mode"], "follow");

    let link = run_cli(&state, &repo, &["link", "src/Controller/OrderController.php", "src/Service/OrderService.php"]);
    assert_eq!(link["mode"], "link");

    let path = run_cli(&state, &repo, &["path", "src/Controller/OrderController.php", "src/Entity/Price.php"]);
    assert_eq!(path["mode"], "path");

    let shared = run_cli(&state, &repo, &["shared", "src/Controller/OrderController.php", "src/Entity/Order.php"]);
    assert_eq!(shared["mode"], "shared");

    let cycles = run_cli(&state, &repo, &["cycles", "src/Service/OrderService.php"]);
    assert_eq!(cycles["mode"], "cycle");

    let impact = run_cli(&state, &repo, &["impact", "work"]);
    assert_eq!(impact["mode"], "changes");

    let layers = run_cli(&state, &repo, &["layers"]);
    assert!(layers.is_object());

    let scenario = run_cli(&state, &repo, &["scenario", "src/Controller/OrderController.php"]);
    assert_eq!(scenario["mode"], "scenario");

    let reach = run_cli(&state, &repo, &["test-reach", "work"]);
    assert!(reach["test_reach"].is_array());

    let hotspots = run_cli(&state, &repo, &["hotspots"]);
    assert!(hotspots["commits"].is_object());

    let runs = run_cli(&state, &repo, &["runs"]);
    assert!(!runs["runs"].as_array().unwrap().is_empty());
    let run_map = run_cli(&state, &repo, &["runs", "run-1"]);
    assert_eq!(run_map["run"]["session"], "run-1");

    let explained = run_cli(&state, &repo, &["explain", "src/Controller/OrderController.php", "--to", "src/Service/OrderService.php"]);
    assert!(explained["answer"].as_str().unwrap().contains("OrderController"));

    // Real stdio MCP server roundtrip covering all 14 tools.
    let mut child = Command::new(bin)
        .args(["--state", state.to_str().unwrap(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    fn rpc_call(
        stdin: &mut impl Write,
        reader: &mut impl BufRead,
        req: Value,
    ) -> Value {
        writeln!(stdin, "{}", req).unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    let init = rpc_call(
        &mut stdin,
        &mut reader,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        }),
    );
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");

    writeln!(
        stdin,
        "{}",
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    )
    .unwrap();
    stdin.flush().unwrap();

    let ping = rpc_call(
        &mut stdin,
        &mut reader,
        json!({"jsonrpc": "2.0", "id": 2, "method": "ping"}),
    );
    assert!(ping["result"].is_object());

    let tools = rpc_call(
        &mut stdin,
        &mut reader,
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
    );
    let listed_names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    let expected = [
        "graph_status",
        "graph_build",
        "graph_find",
        "graph_neighbors",
        "graph_link",
        "graph_path",
        "graph_shared",
        "graph_cycle",
        "graph_change_impact",
        "graph_architecture",
        "graph_scenario",
        "graph_test_reach",
        "graph_history_hotspots",
        "graph_run_audit",
    ];
    for name in expected {
        assert!(listed_names.contains(&name), "missing MCP tool {name}");
    }
    assert_eq!(listed_names.len(), 14);

    let p = repo.to_string_lossy().into_owned();
    let tool_calls = [
        ("graph_status", json!({"project": p, "check": true})),
        ("graph_build", json!({"project": p, "import": repo.join("graphify-out/graph.json").to_string_lossy()})),
        ("graph_find", json!({"project": p, "query": "Order"})),
        ("graph_neighbors", json!({"project": p, "node": "src/Service/OrderService.php"})),
        ("graph_link", json!({"project": p, "from": "src/Controller/OrderController.php", "to": "src/Service/OrderService.php"})),
        ("graph_path", json!({"project": p, "from": "src/Controller/OrderController.php", "to": "src/Entity/Price.php"})),
        ("graph_shared", json!({"project": p, "from": "src/Controller/OrderController.php", "to": "src/Entity/Order.php"})),
        ("graph_cycle", json!({"project": p, "cycle": "src/Service/OrderService.php"})),
        ("graph_change_impact", json!({"project": p, "target": "work"})),
        ("graph_architecture", json!({"project": p})),
        ("graph_scenario", json!({"project": p, "entry": "src/Controller/OrderController.php"})),
        ("graph_test_reach", json!({"project": p, "target": "work"})),
        ("graph_history_hotspots", json!({"project": p})),
        ("graph_run_audit", json!({"project": p, "session": "run-1"})),
    ];
    for (idx, (tool_name, args)) in tool_calls.into_iter().enumerate() {
        let resp = rpc_call(
            &mut stdin,
            &mut reader,
            json!({
                "jsonrpc": "2.0",
                "id": 100 + idx,
                "method": "tools/call",
                "params": {"name": tool_name, "arguments": args}
            }),
        );
        assert_eq!(
            resp["result"]["isError"],
            false,
            "{tool_name} failed: {resp}"
        );
    }

    drop(stdin);
    assert!(child.wait().unwrap().success());

    // Binary MCP guard check when spawned with cwd inside a simulated plugin dir under a git ancestor!
    let plugin_parent = tmp.path().join("marketplace");
    fs::create_dir_all(&plugin_parent).unwrap();
    git(&plugin_parent, &["init", "-q"]);
    let plugin_dir = plugin_parent.join("plugins/graph-map/.codex-plugin");
    fs::create_dir_all(&plugin_dir).unwrap();
    fs::write(plugin_dir.join("plugin.json"), "{\"name\":\"graph-map\"}").unwrap();
    let plugin_root = plugin_parent.join("plugins/graph-map");

    let mut guard_child = Command::new(bin)
        .args(["--state", state.to_str().unwrap(), "mcp"])
        .current_dir(&plugin_root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut g_in = guard_child.stdin.take().unwrap();
    let mut g_out = BufReader::new(guard_child.stdout.take().unwrap());
    writeln!(
        g_in,
        "{}",
        json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"graph_status","arguments":{}}})
    )
    .unwrap();
    g_in.flush().unwrap();
    let mut line = String::new();
    g_out.read_line(&mut line).unwrap();
    let guard_resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(guard_resp["result"]["isError"], true, "{guard_resp}");
    assert!(
        guard_resp["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Pass `project`"),
        "{guard_resp}"
    );
    writeln!(
        g_in,
        "{}",
        json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"graph_status","arguments":{"project":"."}}})
    )
    .unwrap();
    g_in.flush().unwrap();
    line.clear();
    g_out.read_line(&mut line).unwrap();
    let dot_resp: Value = serde_json::from_str(&line).unwrap();
    assert_eq!(dot_resp["result"]["isError"], true, "{dot_resp}");
    drop(g_in);
    let _ = guard_child.wait();
}

fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}

fn http_request(addr: &str, raw_req: &str) -> (u16, String, Vec<u8>) {
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream.write_all(raw_req.as_bytes()).unwrap();
    stream.flush().unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or_else(|| panic!("invalid HTTP response ({} bytes): {:?}", buf.len(), String::from_utf8_lossy(&buf)));
    let head = String::from_utf8_lossy(&buf[..split]).into_owned();
    let body = buf[split + 4..].to_vec();
    let status = head
        .lines()
        .next()
        .unwrap()
        .split_whitespace()
        .nth(1)
        .unwrap()
        .parse::<u16>()
        .unwrap();
    (status, head, body)
}

#[test]
fn http_serve_ui_wasm_csp_and_api_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let state = tmp.path().join("state");
    setup_repo(&repo);

    let port = free_port();
    let addr = format!("127.0.0.1:{port}");
    let bin = env!("CARGO_BIN_EXE_graph-map");
    let mut child = Command::new(bin)
        .args([
            "--state",
            state.to_str().unwrap(),
            "--project",
            repo.to_str().unwrap(),
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let started = Instant::now();
    while TcpStream::connect(&addr).is_err() {
        assert!(started.elapsed() < Duration::from_secs(5), "server never bound");
        std::thread::sleep(Duration::from_millis(25));
    }

    let (st, head, html) = http_request(&addr, &format!("GET / HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"));
    assert_eq!(st, 200);
    let head_lower = head.to_ascii_lowercase();
    assert!(head_lower.contains("content-security-policy:"));
    assert!(
        head_lower.contains("style-src 'self' 'unsafe-inline'"),
        "missing inline style CSP for WASM positioning: {head}"
    );
    assert!(
        head_lower.contains("'wasm-unsafe-eval'"),
        "missing wasm CSP directive: {head}"
    );
    assert!(String::from_utf8_lossy(&html).contains("id=\"graph-map\""));

    let (st, js_head, js_body) = http_request(
        &addr,
        &format!("GET /assets/graph_map_ui.js HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(st, 200);
    assert!(js_head.to_ascii_lowercase().contains("style-src 'self' 'unsafe-inline'"));
    assert!(!js_body.is_empty());

    let (st, _, wasm) = http_request(
        &addr,
        &format!("GET /assets/graph_map_ui_bg.wasm HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(st, 200);
    assert!(wasm.starts_with(b"\0asm"));

    let (st, _, boot_raw) = http_request(
        &addr,
        &format!("GET /api/bootstrap HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(st, 200);
    let boot: Value = serde_json::from_slice(&boot_raw).unwrap();
    let pid = boot["selected"].as_str().unwrap().to_owned();
    let csrf = boot["csrf"].as_str().unwrap().to_owned();

    let (st, _, map_raw) = http_request(
        &addr,
        &format!("GET /api/projects/{pid}/graph/map HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"),
    );
    assert_eq!(st, 200);
    let map: Value = serde_json::from_slice(&map_raw).unwrap();
    assert!(map["nodes"].is_array(), "{map}");

    // CSRF rejection without token
    let body = json!({"kind": "node", "subject": "src/Entity/Order.php"}).to_string();
    let (st, _, _) = http_request(
        &addr,
        &format!(
            "POST /api/projects/{pid}/graph/explain HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(st, 403);

    let (st, _, exp_raw) = http_request(
        &addr,
        &format!(
            "POST /api/projects/{pid}/graph/explain HTTP/1.1\r\nHost: {addr}\r\nX-Harness-Token: {csrf}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        ),
    );
    assert_eq!(st, 200);
    let exp: Value = serde_json::from_slice(&exp_raw).unwrap();
    assert!(exp["answer"].as_str().unwrap().contains("Order.php"));

    let _ = child.kill();
    let _ = child.wait();
}
