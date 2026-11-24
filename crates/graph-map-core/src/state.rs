use crate::{GraphError, Result, util};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Clone, Debug)]
pub struct StateStore {
    pub state: PathBuf,
    lock: Arc<Mutex<()>>,
    active_runs: Arc<Mutex<BTreeMap<String, bool>>>,
    sessions: Arc<Mutex<Vec<Value>>>,
    session_events: Arc<Mutex<BTreeMap<String, Vec<Value>>>>,
    next_event_id: Arc<std::sync::atomic::AtomicI64>,
}

impl StateStore {
    pub fn open(state: &Path) -> Result<Self> {
        util::private_directory(state)?;
        let resolved = util::canonical_dir(state)?;
        util::private_directory(&resolved.join("settings"))?;
        Ok(Self {
            state: resolved,
            lock: Arc::new(Mutex::new(())),
            active_runs: Arc::new(Mutex::new(BTreeMap::new())),
            sessions: Arc::new(Mutex::new(Vec::new())),
            session_events: Arc::new(Mutex::new(BTreeMap::new())),
            next_event_id: Arc::new(std::sync::atomic::AtomicI64::new(1)),
        })
    }

    pub fn default_state_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("GRAPH_MAP_STATE").filter(|v| !v.is_empty()) {
            return PathBuf::from(dir);
        }
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(xdg).join("graph-map");
        }
        if let Some(home) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
            return PathBuf::from(home).join(".local/state/graph-map");
        }
        std::env::temp_dir().join("graph-map-state")
    }

    pub fn open_default() -> Result<Self> {
        Self::open(&Self::default_state_dir())
    }

    pub fn project_id_for(path: &Path) -> Result<String> {
        let root = util::canonical_dir(path)?;
        let text = root
            .to_str()
            .ok_or_else(|| GraphError::invalid("Project path is not valid UTF-8."))?;
        let normalized = if cfg!(windows) {
            text.replace('/', "\\").to_lowercase()
        } else {
            text.to_owned()
        };
        Ok(util::sha256(normalized.as_bytes())[..16].to_owned())
    }

    fn projects_file(&self) -> PathBuf {
        self.state.join("projects.json")
    }

    fn read_projects_locked(&self) -> Result<BTreeMap<String, String>> {
        let path = self.projects_file();
        match std::fs::read(&path) {
            Ok(bytes) => {
                let parsed = util::strict_json(&bytes, 4 * 1024 * 1024)?;
                let mut map = BTreeMap::new();
                if let Some(items) = parsed.as_array() {
                    for item in items {
                        if let (Some(id), Some(p)) =
                            (item["id"].as_str(), item["path"].as_str())
                        {
                            map.insert(id.to_owned(), p.to_owned());
                        }
                    }
                }
                Ok(map)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(error.into()),
        }
    }

    fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            util::private_directory(parent)?;
        }
        let temp = path.with_extension(format!("tmp.{}", util::unique_hex()));
        std::fs::write(&temp, bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o600));
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    pub fn register(&self, path: &Path) -> Result<Value> {
        let root = util::canonical_dir(path)?;
        let path_str = root
            .to_str()
            .ok_or_else(|| GraphError::invalid("Project path is not valid UTF-8."))?
            .to_owned();
        let key = Self::project_id_for(&root)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| GraphError::unavailable("State store lock poisoned."))?;
        let mut projects = self.read_projects_locked()?;
        projects.insert(key.clone(), path_str.clone());
        let rows: Vec<Value> = projects
            .into_iter()
            .map(|(id, path)| {
                let name = Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                json!({"id": id, "name": name, "path": path})
            })
            .collect();
        self.write_atomic(&self.projects_file(), serde_json::to_vec_pretty(&rows)?.as_slice())?;
        Ok(json!({
            "id": key,
            "name": root.file_name().unwrap_or_default().to_string_lossy(),
            "path": path_str,
        }))
    }

    pub fn projects(&self) -> Result<Vec<Value>> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| GraphError::unavailable("State store lock poisoned."))?;
        let projects = self.read_projects_locked()?;
        Ok(projects
            .into_iter()
            .map(|(id, path)| {
                let name = Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                json!({"id": id, "name": name, "path": path})
            })
            .collect())
    }

    pub fn project(&self, key: &str) -> Result<Value> {
        self.projects()?
            .into_iter()
            .find(|v| v["id"] == key)
            .ok_or_else(|| GraphError::missing("Project not found."))
    }

    fn setting_path(&self, name: &str) -> Result<PathBuf> {
        if name.is_empty()
            || name.len() > 160
            || name.contains(['/', '\\', '\0'])
            || name.contains("..")
        {
            return Err(GraphError::invalid("Invalid setting name."));
        }
        Ok(self.state.join("settings").join(format!("{name}.json")))
    }

    pub fn setting(&self, name: &str) -> Result<Option<Value>> {
        let path = self.setting_path(name)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| GraphError::unavailable("State store lock poisoned."))?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(util::strict_json(&bytes, 16 * 1024 * 1024)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn set_setting(&self, name: &str, value: &Value) -> Result<()> {
        let path = self.setting_path(name)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| GraphError::unavailable("State store lock poisoned."))?;
        self.write_atomic(&path, serde_json::to_vec(value)?.as_slice())
    }

    pub fn remove_setting(&self, name: &str) -> Result<()> {
        let path = self.setting_path(name)?;
        let _guard = self
            .lock
            .lock()
            .map_err(|_| GraphError::unavailable("State store lock poisoned."))?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn create(&self, spec: &Value, title: &str, _attachments: &[Value]) -> Result<Value> {
        let id = spec["id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(util::unique_hex);
        let status = spec["status"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or("completed")
            .to_owned();
        let record = json!({
            "id": id,
            "title": if title.is_empty() { spec["title"].as_str().unwrap_or("") } else { title },
            "project_id": spec["project_id"].as_str().unwrap_or(""),
            "project_path": spec["project_path"].as_str().unwrap_or(""),
            "provider": spec["provider"].as_str().unwrap_or("claude"),
            "mode": spec["mode"].as_str().unwrap_or("edit"),
            "status": status,
            "created_at": util::now_rfc3339(),
            "updated_at": util::now_rfc3339(),
        });
        if let Ok(mut list) = self.sessions.lock() {
            list.retain(|item| item["id"] != record["id"]);
            list.push(record.clone());
        }
        Ok(record)
    }

    pub fn get(&self, session: &str) -> Result<Value> {
        let list = self
            .sessions
            .lock()
            .map_err(|_| GraphError::unavailable("Session lock poisoned."))?;
        list.iter()
            .find(|item| item["id"] == session)
            .cloned()
            .ok_or_else(|| GraphError::missing("Session not found."))
    }

    pub fn event(&self, session: &str, payload: &Value) -> Result<i64> {
        let _ = self.get(session)?;
        let id = self
            .next_event_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut stored = payload.clone();
        if let Some(obj) = stored.as_object_mut() {
            obj.entry("id").or_insert_with(|| json!(id));
            obj.entry("at")
                .or_insert_with(|| json!(util::now_rfc3339()));
        }
        if let Ok(mut map) = self.session_events.lock() {
            map.entry(session.to_owned())
                .or_default()
                .push(stored);
        }
        Ok(id)
    }

    pub fn events(&self, session: &str, after: i64) -> Result<Vec<Value>> {
        let map = self
            .session_events
            .lock()
            .map_err(|_| GraphError::unavailable("Session event lock poisoned."))?;
        Ok(map
            .get(session)
            .map(|items| {
                items
                    .iter()
                    .filter(|ev| ev["id"].as_i64().unwrap_or(0) > after)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn summaries(&self) -> Result<Vec<Value>> {
        let list = self
            .sessions
            .lock()
            .map_err(|_| GraphError::unavailable("Session lock poisoned."))?;
        Ok(list.iter().rev().cloned().collect())
    }

    pub fn has_active_runs(&self, project: &str) -> bool {
        let explicit = self
            .active_runs
            .lock()
            .ok()
            .and_then(|map| map.get(project).copied())
            .unwrap_or(false);
        if explicit {
            return true;
        }
        self.sessions.lock().ok().is_some_and(|list| {
            list.iter().any(|s| {
                s["project_id"] == project
                    && matches!(s["status"].as_str(), Some("queued" | "running"))
            })
        })
    }

    pub fn set_active_run(&self, project: &str, active: bool) {
        if let Ok(mut map) = self.active_runs.lock() {
            if active {
                map.insert(project.to_owned(), true);
            } else {
                map.remove(project);
            }
        }
    }
}
