//! Pluggable run activity sources: Harness JSON, Claude Code JSONL, and Codex CLI JSONL.
use crate::{Result, graphs, shell_reads};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug)]
pub struct RunSummary {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub status: String,
    pub updated_at: Option<String>,
    pub launches: usize,
}

#[derive(Clone, Debug)]
pub struct LaunchRecord {
    pub id: String,
    pub at: Option<String>,
    pub steps: u64,
    pub opened: BTreeSet<String>,
    pub edited: Vec<(String, char)>,
    pub shell_reads: BTreeMap<String, u64>,
    pub shell_commands: BTreeMap<u64, Value>,
    pub outside_count: u64,
    pub searched: u64,
    pub commands: u64,
    pub graph_queries: u64,
    pub helpers: u64,
    pub limited: bool,
}

#[derive(Clone, Debug)]
pub struct RecordedRun {
    pub session_id: String,
    pub title: String,
    pub provider: String,
    pub status: String,
    pub project_path: PathBuf,
    pub launches: Vec<LaunchRecord>,
}

pub trait RunActivitySource: Send + Sync {
    fn list_runs(&self, project_root: &Path) -> Result<Vec<RunSummary>>;
    fn load_run(&self, project_root: &Path, session_id: &str) -> Result<Option<RecordedRun>>;
    fn load_shell_event(
        &self,
        project_root: &Path,
        session_id: &str,
        event_id: u64,
    ) -> Result<Option<Value>> {
        let Some(run) = self.load_run(project_root, session_id)? else {
            return Ok(None);
        };
        for launch in &run.launches {
            if let Some(event) = launch.shell_commands.get(&event_id) {
                return Ok(Some(event.clone()));
            }
        }
        Ok(None)
    }
}

/// Composite source checking Harness JSON runs (`.graph-map/runs/*.json`),
/// Claude Code (`~/.claude/projects/.../*.jsonl`), and Codex (`~/.codex/sessions/**/*.jsonl`).
#[derive(Clone, Debug, Default)]
pub struct CompositeRunSource {
    pub extra_runs_dirs: Vec<PathBuf>,
    pub claude_home: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    pub discover_global_homes: bool,
}

impl CompositeRunSource {
    pub fn standalone() -> Self {
        let claude_home = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".claude")));
        let codex_home = std::env::var_os("CODEX_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".codex")));
        Self {
            extra_runs_dirs: std::env::var_os("GRAPH_MAP_RUNS_DIR")
                .filter(|v| !v.is_empty())
                .map(|v| vec![PathBuf::from(v)])
                .unwrap_or_default(),
            claude_home,
            codex_home,
            discover_global_homes: true,
        }
    }

    pub fn with_dirs(
        extra_runs_dirs: Vec<PathBuf>,
        claude_home: Option<PathBuf>,
        codex_home: Option<PathBuf>,
    ) -> Self {
        Self {
            extra_runs_dirs,
            claude_home,
            codex_home,
            discover_global_homes: false,
        }
    }

    fn candidate_harness_dirs(&self, project_root: &Path) -> Vec<(PathBuf, bool)> {
        let mut dirs = Vec::new();
        let local = project_root.join(".graph-map").join("runs");
        if local.is_dir() {
            dirs.push((local, true));
        }
        for dir in &self.extra_runs_dirs {
            if dir.is_dir() {
                dirs.push((dir.clone(), false));
            }
        }
        dirs
    }

    fn collect_claude_files(&self, project_root: &Path) -> Vec<PathBuf> {
        let Some(home) = &self.claude_home else {
            return Vec::new();
        };
        let projects = home.join("projects");
        if !projects.is_dir() {
            return Vec::new();
        }
        let canonical = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        let encoded = encode_claude_project_dir(&canonical);
        let mut files = Vec::new();
        let exact = projects.join(&encoded);
        if exact.is_dir() {
            collect_jsonl_shallow(&exact, &mut files, 100);
        } else if let Ok(entries) = fs::read_dir(&projects) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir()
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name == encoded)
                {
                    collect_jsonl_shallow(&path, &mut files, 100);
                }
            }
        }
        files
    }

    fn collect_codex_files(&self) -> Vec<PathBuf> {
        let Some(home) = &self.codex_home else {
            return Vec::new();
        };
        let sessions = home.join("sessions");
        if !sessions.is_dir() {
            return Vec::new();
        }
        let mut files = Vec::new();
        collect_jsonl_recursive(&sessions, &mut files, 200);
        files
    }
}

impl RunActivitySource for CompositeRunSource {
    fn list_runs(&self, project_root: &Path) -> Result<Vec<RunSummary>> {
        let mut out = Vec::new();
        let mut seen = BTreeSet::new();
        let canonical = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

        for (dir, is_local) in self.candidate_harness_dirs(&canonical) {
            let mut files = Vec::new();
            collect_json_or_jsonl(&dir, &mut files, 100);
            for file in files {
                if let Some(run) = parse_harness_file(&file, &canonical, is_local)
                    && seen.insert(run.session_id.clone())
                {
                    out.push(summary_from_run(&run));
                }
            }
        }

        for file in self.collect_claude_files(&canonical) {
            if let Some(run) = parse_claude_jsonl(&file, &canonical)
                && seen.insert(run.session_id.clone())
            {
                out.push(summary_from_run(&run));
            }
        }

        for file in self.collect_codex_files() {
            if let Some(run) = parse_codex_jsonl(&file, &canonical)
                && seen.insert(run.session_id.clone())
            {
                out.push(summary_from_run(&run));
            }
        }

        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at).then(a.id.cmp(&b.id)));
        out.truncate(50);
        Ok(out)
    }

    fn load_run(&self, project_root: &Path, session_id: &str) -> Result<Option<RecordedRun>> {
        let canonical = fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());
        for (dir, is_local) in self.candidate_harness_dirs(&canonical) {
            let mut files = Vec::new();
            collect_json_or_jsonl(&dir, &mut files, 200);
            for file in files {
                if let Some(run) = parse_harness_file(&file, &canonical, is_local)
                    && run.session_id == session_id
                {
                    return Ok(Some(run));
                }
            }
        }
        for file in self.collect_claude_files(&canonical) {
            if let Some(run) = parse_claude_jsonl(&file, &canonical)
                && run.session_id == session_id
            {
                return Ok(Some(run));
            }
        }
        for file in self.collect_codex_files() {
            if let Some(run) = parse_codex_jsonl(&file, &canonical)
                && run.session_id == session_id
            {
                return Ok(Some(run));
            }
        }
        Ok(None)
    }
}

fn summary_from_run(run: &RecordedRun) -> RunSummary {
    RunSummary {
        id: run.session_id.clone(),
        title: run.title.clone(),
        provider: run.provider.clone(),
        status: run.status.clone(),
        updated_at: run.launches.last().and_then(|l| l.at.clone()),
        launches: run.launches.len(),
    }
}

fn encode_claude_project_dir(path: &Path) -> String {
    let text = path.to_string_lossy();
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn collect_json_or_jsonl(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e, "json" | "jsonl"))
        {
            out.push(path);
        }
    }
}

fn collect_jsonl_shallow(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.push(path);
        }
    }
}

fn collect_jsonl_recursive(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(folder) = stack.pop() {
        if out.len() >= limit {
            break;
        }
        let Ok(entries) = fs::read_dir(&folder) else {
            continue;
        };
        let mut children: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        children.sort();
        children.reverse();
        for path in children {
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                out.push(path);
                if out.len() >= limit {
                    break;
                }
            }
        }
    }
}

fn normalize_project_path(raw: &str, workspace: &Path) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let path = Path::new(trimmed);
    let rel = if path.is_absolute() {
        let canonical_ws = fs::canonicalize(workspace).unwrap_or_else(|_| workspace.to_path_buf());
        let rel = path
            .strip_prefix(workspace)
            .or_else(|_| path.strip_prefix(&canonical_ws))
            .ok()?;
        rel.to_string_lossy().replace('\\', "/")
    } else {
        trimmed.replace('\\', "/")
    };
    let clean = rel.trim_start_matches("./").trim_start_matches('/').to_owned();
    if clean.is_empty() || clean.starts_with("..") {
        None
    } else {
        Some(clean)
    }
}

/// Extracted from `infra-harness/src/run_activity.rs` for Harness event JSON logs.
#[derive(Default)]
struct Entry {
    read: Option<String>,
    keys: Vec<(String, Option<String>, String)>,
    changes: Vec<(String, Option<String>, String)>,
    command: Option<u64>,
}

pub struct HarnessRunLedger {
    provider: String,
    pub limited: bool,
    steps: u64,
    helpers: u64,
    searched: u64,
    commands: u64,
    graph: u64,
    opened: Vec<(String, Option<String>)>,
    edited: Vec<(String, Option<String>)>,
    created: BTreeSet<String>,
    deleted: BTreeSet<String>,
    calls: BTreeMap<String, Entry>,
    uncalled: BTreeMap<String, VecDeque<Entry>>,
    command_id: u64,
    reads: u64,
    outside_reads: u64,
}

impl HarnessRunLedger {
    pub fn new(provider: &str) -> Self {
        Self {
            provider: provider.to_owned(),
            limited: false,
            steps: 0,
            helpers: 0,
            searched: 0,
            commands: 0,
            graph: 0,
            opened: Vec::new(),
            edited: Vec::new(),
            created: BTreeSet::new(),
            deleted: BTreeSet::new(),
            calls: BTreeMap::new(),
            uncalled: BTreeMap::new(),
            command_id: 0,
            reads: 0,
            outside_reads: 0,
        }
    }

    fn is_read(kind: &str) -> bool {
        matches!(
            kind,
            "read" | "view" | "cat" | "read_file" | "view_file" | "open_file" | "notebookread"
        )
    }

    fn is_edit(kind: &str) -> bool {
        matches!(
            kind,
            "edit" | "write" | "multiedit" | "notebookedit" | "apply_patch" | "replace" | "filechange"
        )
    }

    fn is_delete(kind: &str) -> bool {
        matches!(kind, "delete" | "remove" | "rm")
    }

    fn is_search(kind: &str) -> bool {
        matches!(
            kind,
            "grep" | "glob" | "ls" | "search" | "grep_search" | "file_search" | "glob_file_search"
        )
    }

    fn is_command(kind: &str) -> bool {
        matches!(
            kind,
            "bash" | "shell" | "powershell" | "run_terminal_cmd" | "run_command" | "terminal"
                | "exec_command" | "shell_command"
        )
    }

    fn is_helper(kind: &str) -> bool {
        matches!(kind, "task" | "agent" | "collab_agent" | "spawn_agent")
    }

    fn changes(fields: &Value) -> Vec<(String, Option<String>, String)> {
        fields["changes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|c| c.is_object() && c["path"].as_str().is_some_and(|p| !p.is_empty()))
            .map(|c| {
                let path = c["path"].as_str().unwrap_or("");
                let outside = c["outside"] == true;
                let key = format!("{}{path}", if outside { "outside:" } else { "" });
                (
                    key,
                    if outside { None } else { Some(path.into()) },
                    c["kind"].as_str().unwrap_or("update").into(),
                )
            })
            .collect()
    }

    pub fn observe(&mut self, event: &Value) {
        if event["targets"] == "limited" {
            self.limited = true;
        }
        if event["kind"] != "tool" {
            return;
        }
        let state = event["state"].as_str().unwrap_or("");
        let call = event["call"].as_str().filter(|v| !v.is_empty());
        let kind = event["tool"].as_str().unwrap_or("").to_lowercase();

        let entry = if state == "completed" && call.is_some_and(|c| self.calls.contains_key(c)) {
            self.calls.remove(call.unwrap_or("")).unwrap_or_default()
        } else if state == "completed"
            && call.is_none()
            && self
                .uncalled
                .get(&kind)
                .is_some_and(|entries| !entries.is_empty())
        {
            self.uncalled
                .get_mut(&kind)
                .and_then(VecDeque::pop_front)
                .unwrap_or_default()
        } else if state == "completed" && (call.is_none() || kind.is_empty()) {
            Entry::default()
        } else {
            self.steps += 1;
            let outside = event["outside"] == true;
            let path = if outside {
                None
            } else {
                event["path"].as_str().map(str::to_owned)
            };
            let key = format!(
                "{}{}",
                if outside { "outside:" } else { "" },
                event["path"].as_str().unwrap_or("")
            );
            let mut entry = Entry {
                changes: Self::changes(event),
                ..Default::default()
            };
            if Self::is_read(&kind) && event["path"].as_str().is_some_and(|p| !p.is_empty()) {
                if !self.opened.iter().any(|(old, _)| old == &key) {
                    self.opened.push((key, path.clone()));
                }
                self.reads += 1;
                if outside {
                    self.outside_reads += 1;
                }
                entry.read = path;
            } else if (Self::is_edit(&kind) || Self::is_delete(&kind))
                && event["path"].as_str().is_some_and(|p| !p.is_empty())
            {
                let created = kind == "write"
                    && !self.opened.iter().any(|(old, _)| old == &key)
                    && !self.edited.iter().any(|(old, _)| old == &key);
                entry.keys.push((
                    key,
                    path,
                    if Self::is_delete(&kind) {
                        "delete"
                    } else if created {
                        "add"
                    } else {
                        "update"
                    }
                    .into(),
                ));
            } else if Self::is_search(&kind) {
                self.searched += 1;
            } else if Self::is_command(&kind) {
                self.commands += 1;
                if graphs::asked(event["detail"].as_str().unwrap_or("")) {
                    self.graph += 1;
                }
                self.command_id += 1;
                entry.command = Some(self.command_id);
            } else if Self::is_helper(&kind)
                && (self.provider != "codex" || event["detail"] == "spawn_agent")
            {
                self.helpers += 1;
            }
            if state == "started" {
                if let Some(call) = call {
                    self.calls.insert(call.into(), entry);
                } else if !kind.is_empty() {
                    self.uncalled.entry(kind).or_default().push_back(entry);
                }
                return;
            }
            entry
        };

        if event["ok"] == false {
            return;
        }
        let changes = if event["changes"].as_array().is_some_and(|c| !c.is_empty()) {
            Self::changes(event)
        } else {
            entry.changes
        };
        for (key, path, kind) in entry.keys.into_iter().chain(changes) {
            if !self.edited.iter().any(|(old, _)| old == &key) {
                self.edited.push((key.clone(), path));
            }
            if kind == "add" {
                self.created.insert(key);
            } else if kind == "delete" {
                self.deleted.insert(key);
            }
        }
    }

    pub fn into_launch(
        self,
        id: String,
        at: Option<String>,
        shell_reads: BTreeMap<String, u64>,
        shell_commands: BTreeMap<u64, Value>,
    ) -> LaunchRecord {
        let plain = |p: &str| p.replace('\\', "/").trim_start_matches("./").to_owned();
        let mut opened: BTreeSet<String> = self
            .opened
            .iter()
            .filter_map(|(_, p)| p.as_deref().map(plain))
            .collect();
        let explicit_opened = opened.len();
        opened.extend(shell_reads.keys().cloned());
        let edited: Vec<(String, char)> = self
            .edited
            .iter()
            .filter_map(|(key, p)| {
                let path = plain(p.as_deref()?);
                let done = if self.deleted.contains(key) {
                    'D'
                } else if self.created.contains(key) {
                    'A'
                } else {
                    'M'
                };
                Some((path, done))
            })
            .collect();
        let total_touched = (self.opened.len() + self.edited.len()) as u64;
        let in_project_touched = (explicit_opened + edited.len()) as u64;
        LaunchRecord {
            id,
            at,
            steps: self.steps,
            opened,
            edited,
            shell_reads,
            shell_commands,
            outside_count: total_touched.saturating_sub(in_project_touched),
            searched: self.searched,
            commands: self.commands,
            graph_queries: self.graph,
            helpers: self.helpers,
            limited: self.limited,
        }
    }
}

fn parse_harness_file(path: &Path, project_root: &Path, is_local: bool) -> Option<RecordedRun> {
    let raw = fs::read_to_string(path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let (session_id, title, provider, status, workspace, events): (
        String,
        String,
        String,
        String,
        PathBuf,
        Vec<Value>,
    ) = if trimmed.starts_with('{') {
        let doc: Value = serde_json::from_str(trimmed).ok()?;
        let session = if doc["session"].is_object() {
            &doc["session"]
        } else {
            &doc
        };
        let events = doc["events"].as_array()?.clone();
        let ws = session["project_path"]
            .as_str()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_root.to_path_buf());
        let id = session["id"]
            .as_str()
            .or_else(|| path.file_stem().and_then(|s| s.to_str()))?
            .to_owned();
        (
            id,
            session["title"].as_str().unwrap_or("Harness run").to_owned(),
            session["provider"].as_str().unwrap_or("harness").to_owned(),
            session["status"].as_str().unwrap_or("completed").to_owned(),
            ws,
            events,
        )
    } else {
        let mut events = Vec::new();
        for line in trimmed.lines() {
            if let Ok(val) = serde_json::from_str::<Value>(line) {
                events.push(val);
            }
        }
        let id = path.file_stem()?.to_str()?.to_owned();
        (
            id.clone(),
            id,
            "harness".to_owned(),
            "completed".to_owned(),
            project_root.to_path_buf(),
            events,
        )
    };

    let canonical_ws = fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone());
    if !is_local && canonical_ws != project_root && workspace != project_root {
        return None;
    }

    Some(record_from_events(
        session_id,
        title,
        provider,
        status,
        workspace,
        &events,
    ))
}

pub fn record_from_events(
    session_id: String,
    title: String,
    provider: String,
    status: String,
    workspace: PathBuf,
    events: &[Value],
) -> RecordedRun {
    let mut launches_order = Vec::new();
    type LedgerEntry = (Option<String>, HarnessRunLedger, BTreeMap<String, u64>, BTreeMap<u64, Value>);
    let mut ledgers: BTreeMap<String, LedgerEntry> = BTreeMap::new();
    for (idx, event) in events.iter().enumerate() {
        let lid = event["launch_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("")
            .to_owned();
        if !ledgers.contains_key(&lid) {
            launches_order.push(lid.clone());
            ledgers.insert(
                lid.clone(),
                (
                    event["at"].as_str().map(str::to_owned),
                    HarnessRunLedger::new(&provider),
                    BTreeMap::new(),
                    BTreeMap::new(),
                ),
            );
        }
        let (_, ledger, shell_reads, shell_commands) = ledgers.get_mut(&lid).unwrap();
        ledger.observe(event);
        if let Some((ev_id, files)) = shell_reads::completed(event, &workspace) {
            for f in files {
                shell_reads.insert(f, ev_id);
            }
            shell_commands.insert(ev_id, event.clone());
        } else if event["tool"] == "Shell" && event["state"] == "completed" {
            let ev_id = event["id"].as_u64().unwrap_or((idx + 1) as u64);
            shell_commands.insert(ev_id, event.clone());
        }
    }

    let launches = launches_order
        .into_iter()
        .filter_map(|lid| {
            let (at, ledger, reads, cmds) = ledgers.remove(&lid)?;
            Some(ledger.into_launch(lid, at, reads, cmds))
        })
        .collect();

    RecordedRun {
        session_id,
        title,
        provider,
        status,
        project_path: workspace,
        launches,
    }
}

fn parse_claude_jsonl(path: &Path, project_root: &Path) -> Option<RecordedRun> {
    let raw = fs::read_to_string(path).ok()?;
    let mut session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("claude-run")
        .to_owned();
    let mut title = String::new();
    let mut cwd: Option<PathBuf> = None;
    let mut started_at: Option<String> = None;
    let mut steps = 0u64;
    let mut opened = BTreeSet::new();
    let mut edited_map: BTreeMap<String, char> = BTreeMap::new();
    let mut outside_count = 0u64;
    let mut searched = 0u64;
    let mut commands = 0u64;
    let mut graph_queries = 0u64;
    let mut helpers = 0u64;
    let mut pending_bash: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut shell_reads = BTreeMap::new();
    let mut shell_commands = BTreeMap::new();
    let mut next_event_id = 1u64;

    for line in raw.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(sid) = record["sessionId"].as_str() {
            session_id = sid.to_owned();
        }
        if cwd.is_none()
            && let Some(c) = record["cwd"].as_str()
        {
            cwd = Some(PathBuf::from(c));
        }
        if started_at.is_none() {
            started_at = record["timestamp"].as_str().map(str::to_owned);
        }
        let kind = record["type"].as_str().unwrap_or("");
        if kind == "user"
            && title.is_empty()
            && let Some(text) = record["message"]["content"].as_str()
        {
            title = text.lines().next().unwrap_or("").chars().take(80).collect();
        }
        if kind == "assistant"
            && let Some(items) = record["message"]["content"].as_array()
        {
            for item in items {
                if item["type"] != "tool_use" {
                    continue;
                }
                steps += 1;
                let tool = item["name"].as_str().unwrap_or("");
                let input = &item["input"];
                let lower = tool.to_lowercase();
                if matches!(lower.as_str(), "read" | "view" | "notebookread") {
                    if let Some(raw_path) = input["file_path"]
                        .as_str()
                        .or_else(|| input["notebook_path"].as_str())
                        .or_else(|| input["path"].as_str())
                    {
                        match normalize_project_path(raw_path, project_root) {
                            Some(rel) => {
                                opened.insert(rel);
                            }
                            None => outside_count += 1,
                        }
                    }
                } else if matches!(
                    lower.as_str(),
                    "edit" | "write" | "multiedit" | "notebookedit"
                ) {
                    if let Some(raw_path) = input["file_path"]
                        .as_str()
                        .or_else(|| input["notebook_path"].as_str())
                        .or_else(|| input["path"].as_str())
                    {
                        match normalize_project_path(raw_path, project_root) {
                            Some(rel) => {
                                let status = if lower == "write" && !opened.contains(&rel) && !edited_map.contains_key(&rel) {
                                    'A'
                                } else {
                                    'M'
                                };
                                edited_map.entry(rel).or_insert(status);
                            }
                            None => outside_count += 1,
                        }
                    }
                } else if matches!(lower.as_str(), "grep" | "glob" | "ls") {
                    searched += 1;
                } else if lower == "bash" {
                    commands += 1;
                    let cmd = input["command"].as_str().unwrap_or("");
                    if graphs::asked(cmd) {
                        graph_queries += 1;
                    }
                    if let Some(tool_id) = item["id"].as_str() {
                        let ev_id = next_event_id;
                        next_event_id += 1;
                        pending_bash.insert(tool_id.to_owned(), (ev_id, cmd.to_owned()));
                    }
                } else if matches!(lower.as_str(), "task" | "agent") {
                    helpers += 1;
                }
            }
        } else if kind == "user"
            && let Some(items) = record["message"]["content"].as_array()
        {
            for item in items {
                if item["type"] == "tool_result"
                    && let Some(tool_id) = item["tool_use_id"].as_str()
                    && let Some((ev_id, cmd)) = pending_bash.remove(tool_id)
                {
                    let ok = item["is_error"] != true;
                    let synth = json!({
                        "id": ev_id,
                        "kind": "tool",
                        "tool": "Shell",
                        "state": "completed",
                        "ok": ok,
                        "exit_code": if ok { 0 } else { 1 },
                        "detail": cmd,
                    });
                    if let Some((_, files)) = shell_reads::completed(&synth, project_root) {
                        for f in files {
                            opened.insert(f.clone());
                            shell_reads.insert(f, ev_id);
                        }
                        shell_commands.insert(ev_id, synth);
                    }
                }
            }
        }
    }

    let workspace = cwd.unwrap_or_else(|| project_root.to_path_buf());
    if fs::canonicalize(&workspace).unwrap_or_else(|_| workspace.clone()) != project_root {
        return None;
    }
    if steps == 0 && opened.is_empty() && edited_map.is_empty() {
        return None;
    }
    if title.is_empty() {
        title = format!("Claude {session_id}");
    }
    Some(RecordedRun {
        session_id: session_id.clone(),
        title,
        provider: "claude".to_owned(),
        status: "completed".to_owned(),
        project_path: workspace,
        launches: vec![LaunchRecord {
            id: "launch-1".to_owned(),
            at: started_at,
            steps,
            opened,
            edited: edited_map.into_iter().collect(),
            shell_reads,
            shell_commands,
            outside_count,
            searched,
            commands,
            graph_queries,
            helpers,
            limited: false,
        }],
    })
}

fn parse_codex_jsonl(path: &Path, project_root: &Path) -> Option<RecordedRun> {
    let raw = fs::read_to_string(path).ok()?;
    let mut session_id = None;
    let mut workspace = None;
    let mut started_at = None;
    let mut title = String::new();
    let mut steps = 0u64;
    let mut opened = BTreeSet::new();
    let mut edited_map: BTreeMap<String, char> = BTreeMap::new();
    let mut outside_count = 0u64;
    let mut searched = 0u64;
    let mut commands = 0u64;
    let mut graph_queries = 0u64;
    let mut helpers = 0u64;
    let mut pending_cmds: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut shell_reads = BTreeMap::new();
    let mut shell_commands = BTreeMap::new();
    let mut next_event_id = 1u64;

    for (idx, line) in raw.lines().enumerate() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if idx == 0 && record["type"] == "session_meta" {
            let payload = &record["payload"];
            let cwd = payload["cwd"].as_str().map(PathBuf::from)?;
            let canonical_ws = fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
            if canonical_ws != project_root {
                return None;
            }
            workspace = Some(cwd);
            session_id = payload["id"].as_str().map(str::to_owned);
            started_at = payload["timestamp"]
                .as_str()
                .or_else(|| record["timestamp"].as_str())
                .map(str::to_owned);
            continue;
        }
        workspace.as_ref()?;
        if record["type"] != "response_item" {
            continue;
        }
        let payload = &record["payload"];
        let ptype = payload["type"].as_str().unwrap_or("");
        if ptype == "message" && payload["role"] == "user" && title.is_empty() {
            if let Some(text) = payload["content"]
                .as_array()
                .and_then(|arr| arr.iter().find_map(|c| c["text"].as_str()))
            {
                title = text.lines().next().unwrap_or("").chars().take(80).collect();
            }
        } else if matches!(ptype, "function_call" | "custom_tool_call") {
            steps += 1;
            let name = payload["name"].as_str().unwrap_or("").to_lowercase();
            let args_val = if let Some(s) = payload["arguments"].as_str() {
                serde_json::from_str::<Value>(s).unwrap_or(Value::Null)
            } else {
                payload["arguments"].clone()
            };
            if name == "spawn_agent" {
                helpers += 1;
            } else if matches!(name.as_str(), "read_file" | "view_file" | "open_file") {
                if let Some(p) = args_val["path"]
                    .as_str()
                    .or_else(|| args_val["file_path"].as_str())
                {
                    match normalize_project_path(p, project_root) {
                        Some(rel) => {
                            opened.insert(rel);
                        }
                        None => outside_count += 1,
                    }
                }
            } else if matches!(name.as_str(), "grep_search" | "find_by_name" | "list_dir") {
                searched += 1;
            } else if name == "apply_patch" {
                let patch_text = payload["input"]
                    .as_str()
                    .or_else(|| args_val["patch"].as_str())
                    .unwrap_or("");
                for patch_line in patch_text.lines() {
                    if let Some(p) = patch_line.strip_prefix("*** Add File: ") {
                        if let Some(rel) = normalize_project_path(p, project_root) {
                            edited_map.entry(rel).or_insert('A');
                        } else {
                            outside_count += 1;
                        }
                    } else if let Some(p) = patch_line.strip_prefix("*** Update File: ") {
                        if let Some(rel) = normalize_project_path(p, project_root) {
                            edited_map.entry(rel).or_insert('M');
                        } else {
                            outside_count += 1;
                        }
                    } else if let Some(p) = patch_line.strip_prefix("*** Delete File: ") {
                        if let Some(rel) = normalize_project_path(p, project_root) {
                            edited_map.insert(rel, 'D');
                        } else {
                            outside_count += 1;
                        }
                    }
                }
            } else if matches!(
                name.as_str(),
                "exec_command" | "shell_command" | "shell"
            ) {
                commands += 1;
                let cmd = args_val["cmd"]
                    .as_str()
                    .or_else(|| args_val["command"].as_str())
                    .unwrap_or("");
                if graphs::asked(cmd) {
                    graph_queries += 1;
                }
                if let Some(call_id) = payload["call_id"].as_str() {
                    let ev_id = next_event_id;
                    next_event_id += 1;
                    pending_cmds.insert(call_id.to_owned(), (ev_id, cmd.to_owned()));
                }
            }
        } else if matches!(ptype, "function_call_output" | "custom_tool_call_output")
            && let Some(call_id) = payload["call_id"].as_str()
            && let Some((ev_id, cmd)) = pending_cmds.remove(call_id)
        {
            let output_text = payload["output"].as_str().unwrap_or("");
            let exit_code = extract_codex_exit_code(output_text).unwrap_or(0);
            let ok = exit_code == 0;
            let synth = json!({
                "id": ev_id,
                "kind": "tool",
                "tool": "Shell",
                "state": "completed",
                "ok": ok,
                "exit_code": exit_code,
                "detail": cmd,
            });
            if let Some((_, files)) = shell_reads::completed(&synth, project_root) {
                for f in files {
                    opened.insert(f.clone());
                    shell_reads.insert(f, ev_id);
                }
                shell_commands.insert(ev_id, synth);
            }
        }
    }

    let workspace = workspace?;
    let session_id = session_id.or_else(|| {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_owned)
    })?;
    if steps == 0 && opened.is_empty() && edited_map.is_empty() {
        return None;
    }
    if title.is_empty() {
        title = format!("Codex {session_id}");
    }
    Some(RecordedRun {
        session_id,
        title,
        provider: "codex".to_owned(),
        status: "completed".to_owned(),
        project_path: workspace,
        launches: vec![LaunchRecord {
            id: "launch-1".to_owned(),
            at: started_at,
            steps,
            opened,
            edited: edited_map.into_iter().collect(),
            shell_reads,
            shell_commands,
            outside_count,
            searched,
            commands,
            graph_queries,
            helpers,
            limited: false,
        }],
    })
}

fn extract_codex_exit_code(output: &str) -> Option<i64> {
    for line in output.lines().take(5) {
        if let Some(rest) = line.strip_prefix("Process exited with code ")
            && let Ok(code) = rest.trim().parse::<i64>()
        {
            return Some(code);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_and_codex_jsonl_logs_map_to_project_runs() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let src = repo.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("a.rs"), "fn a() {}\n").unwrap();
        fs::write(src.join("b.rs"), "fn b() {}\n").unwrap();
        let canonical_repo = fs::canonicalize(&repo).unwrap();

        let claude_home = root.path().join("claude");
        let encoded = encode_claude_project_dir(&canonical_repo);
        let claude_dir = claude_home.join("projects").join(&encoded);
        fs::create_dir_all(&claude_dir).unwrap();
        let claude_lines = [
            json!({
                "type": "user",
                "sessionId": "claude-run-1",
                "cwd": canonical_repo.to_string_lossy(),
                "timestamp": "2026-06-01T10:00:00Z",
                "message": {"content": "Inspect a.rs and update b.rs"},
            }),
            json!({
                "type": "assistant",
                "sessionId": "claude-run-1",
                "message": {"content": [
                    {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": canonical_repo.join("src/a.rs").to_string_lossy()}},
                    {"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": canonical_repo.join("src/b.rs").to_string_lossy()}},
                    {"type": "tool_use", "id": "t3", "name": "Grep", "input": {"pattern": "fn a"}}
                ]},
            }),
        ];
        fs::write(
            claude_dir.join("claude-run-1.jsonl"),
            claude_lines
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let codex_home = root.path().join("codex");
        let codex_dir = codex_home.join("sessions/2026/06/01");
        fs::create_dir_all(&codex_dir).unwrap();
        let codex_lines = [
            json!({
                "type": "session_meta",
                "timestamp": "2026-06-01T11:00:00Z",
                "payload": {
                    "id": "codex-run-1",
                    "cwd": canonical_repo.to_string_lossy(),
                    "timestamp": "2026-06-01T11:00:00Z"
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Audit module boundaries"}]
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call",
                    "call_id": "c1",
                    "name": "exec_command",
                    "arguments": "{\"cmd\":\"sed -n '1p' src/a.rs\"}"
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "function_call_output",
                    "call_id": "c1",
                    "output": "Chunk ID\nProcess exited with code 0\nfn a() {}\n"
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "custom_tool_call",
                    "call_id": "c2",
                    "name": "apply_patch",
                    "input": "*** Begin Patch\n*** Update File: src/b.rs\n@@\n*** End Patch"
                }
            }),
        ];
        fs::write(
            codex_dir.join("rollout-codex-run-1.jsonl"),
            codex_lines
                .iter()
                .map(|v| v.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let src_adapter = CompositeRunSource::with_dirs(vec![], Some(claude_home), Some(codex_home));
        let listed = src_adapter.list_runs(&canonical_repo).unwrap();
        assert_eq!(listed.len(), 2, "{listed:?}");
        assert!(listed.iter().any(|r| r.id == "claude-run-1" && r.provider == "claude"));
        assert!(listed.iter().any(|r| r.id == "codex-run-1" && r.provider == "codex"));

        let claude = src_adapter.load_run(&canonical_repo, "claude-run-1").unwrap().unwrap();
        assert!(claude.launches[0].opened.contains("src/a.rs"));
        assert_eq!(claude.launches[0].edited, vec![("src/b.rs".to_owned(), 'M')]);
        assert_eq!(claude.launches[0].searched, 1);

        let codex = src_adapter.load_run(&canonical_repo, "codex-run-1").unwrap().unwrap();
        assert!(codex.launches[0].opened.contains("src/a.rs"));
        assert!(!codex.launches[0].shell_reads.is_empty());
        assert_eq!(codex.launches[0].edited, vec![("src/b.rs".to_owned(), 'M')]);
    }
}
