use crate::{GraphError, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PreparedCommand {
    pub argv: Vec<String>,
    pub environment: BTreeMap<String, String>,
    pub input: Vec<u8>,
    pub readable: Vec<PathBuf>,
    pub remove: Vec<String>,
    pub prompt_prefix: String,
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub struct GitOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
}

pub fn sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

pub fn sha(bytes: &[u8]) -> String {
    sha256(bytes)
}

pub fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

pub fn now_millis_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn unique_hex() -> String {
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    sha256(format!("{pid}:{nanos}:{seq}").as_bytes())[..32].to_owned()
}

pub fn exact(value: &Value, required: &[&str], optional: &[&str]) -> bool {
    value.as_object().is_some_and(|map| {
        required.iter().all(|key| map.contains_key(*key))
            && map
                .keys()
                .all(|key| required.contains(&key.as_str()) || optional.contains(&key.as_str()))
    })
}

pub fn strict_json(bytes: &[u8], limit: usize) -> Result<Value> {
    if bytes.len() > limit {
        return Err(GraphError::invalid("JSON payload exceeds byte limit."));
    }
    let text = std::str::from_utf8(bytes)
        .map_err(|_| GraphError::invalid("JSON payload is not valid UTF-8."))?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for byte in text.as_bytes() {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else if *byte == b'"' {
            in_string = true;
        } else if *byte == b'{' || *byte == b'[' {
            depth += 1;
            if depth > 64 {
                return Err(GraphError::invalid("JSON nesting depth exceeds limit."));
            }
        } else if *byte == b'}' || *byte == b']' {
            depth = depth.saturating_sub(1);
        }
    }
    Ok(serde_json::from_str(text)?)
}

pub fn private_directory(path: &Path) -> Result<()> {
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(GraphError::forbidden(
                "State path must be a regular directory.",
            ));
        }
    } else {
        std::fs::create_dir_all(path)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

pub fn canonicalize(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

pub fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    if !resolved.is_dir() {
        return Err(GraphError::invalid(
            "Project path must be an existing directory.",
        ));
    }
    Ok(resolved)
}

fn absolute_lexical(path: &Path) -> Result<PathBuf> {
    let input = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for part in input.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            _ => out.push(part.as_os_str()),
        }
    }
    Ok(out)
}

pub fn python_realpath(path: &Path) -> Result<PathBuf> {
    let mut current = absolute_lexical(path)?;
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&current) {
            Ok(mut base) => {
                for item in missing.iter().rev() {
                    base.push(item);
                }
                return Ok(base);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = current
                    .file_name()
                    .ok_or_else(|| GraphError::unavailable("Cannot resolve filesystem root."))?
                    .to_os_string();
                missing.push(name);
                if !current.pop() {
                    return Err(GraphError::unavailable("Cannot resolve filesystem root."));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn is_linked(meta: &std::fs::Metadata) -> bool {
    if meta.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    false
}

pub fn admit_descendant(anchor: &Path, path: &Path) -> Result<()> {
    let suffix = path
        .strip_prefix(anchor)
        .map_err(|_| GraphError::forbidden("Authority path escapes its anchor."))?;
    let anchor_meta = std::fs::symlink_metadata(anchor)?;
    if !anchor_meta.is_dir() || is_linked(&anchor_meta) {
        return Err(GraphError::forbidden(
            "Authority directory is linked or not a directory.",
        ));
    }
    let mut current = anchor.to_path_buf();
    for part in suffix.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(GraphError::forbidden("Invalid authority path component."));
        }
        current.push(part.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if is_linked(&meta) => {
                return Err(GraphError::forbidden("Linked authority path is refused."));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct DirCustody {
    root: PathBuf,
    #[cfg(unix)]
    id: (u64, u64),
}

impl DirCustody {
    pub fn new(root: &Path) -> Result<Self> {
        let abs = absolute_lexical(root)?;
        let meta = std::fs::symlink_metadata(&abs)?;
        if !meta.is_dir() || is_linked(&meta) {
            return Err(GraphError::forbidden(
                "Publication directory is linked or not a directory.",
            ));
        }
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        };
        Ok(Self {
            root: abs,
            #[cfg(unix)]
            id,
        })
    }

    pub fn check(&self) -> Result<()> {
        let meta = std::fs::symlink_metadata(&self.root)?;
        if !meta.is_dir() || is_linked(&meta) {
            return Err(GraphError::conflict("Publication root custody changed."));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (meta.dev(), meta.ino()) != self.id {
                return Err(GraphError::conflict("Publication root custody changed."));
            }
        }
        Ok(())
    }
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let candidate = Path::new(name);
    if candidate.components().count() > 1 {
        return is_executable(candidate).then(|| candidate.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    #[cfg(windows)]
    let extensions: Vec<String> = std::env::var_os("PATHEXT")
        .map(|ext| {
            ext.to_string_lossy()
                .split(';')
                .filter(|item| !item.is_empty())
                .map(|item| item.to_string())
                .collect()
        })
        .unwrap_or_else(|| vec![".EXE".into(), ".CMD".into(), ".BAT".into()]);
    for folder in std::env::split_paths(&path_var) {
        let direct = folder.join(name);
        if is_executable(&direct) {
            return Some(direct);
        }
        #[cfg(windows)]
        {
            for ext in &extensions {
                let with_ext = folder.join(format!("{name}{ext}"));
                if is_executable(&with_ext) {
                    return Some(with_ext);
                }
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn git_override(key: &OsStr) -> bool {
    #[cfg(windows)]
    {
        key.to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
    }
    #[cfg(not(windows))]
    {
        key.to_string_lossy().starts_with("GIT_")
    }
}

pub fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

pub fn git_run(root: &Path, args: &[String], seconds: u64) -> Result<GitOutput> {
    let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
    git_run_with_input(root, &os_args, None, Duration::from_secs(seconds), 16 * 1024 * 1024)
}

pub fn git_run_with_input(
    root: &Path,
    arguments: &[OsString],
    input: Option<Vec<u8>>,
    timeout: Duration,
    output_limit: usize,
) -> Result<GitOutput> {
    if timeout.is_zero() {
        return Err(GraphError::unavailable("Git timeout must be positive."));
    }
    let mut command = Command::new("git");
    command
        .args(["--no-optional-locks", "--no-pager", "-c"])
        .arg(format!(
            "core.hooksPath={}",
            if cfg!(windows) { "NUL" } else { "/dev/null" }
        ))
        .args(["-c", "core.fsmonitor=false", "-C"])
        .arg(root)
        .args(arguments)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, _) in std::env::vars_os() {
        if git_override(&key) {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0");
    let mut child = command.spawn().map_err(|error| {
        GraphError::unavailable(format!("Git could not start: {error}"))
    })?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| GraphError::unavailable("Git stdout unavailable."))?;
    let limit = output_limit.max(1024);
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        while let Ok(read) = stdout.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let room = limit.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..read.min(room)]);
        }
        bytes
    });
    let writer = input.map(|bytes| {
        let stdin = child.stdin.take();
        std::thread::spawn(move || -> std::io::Result<()> {
            if let Some(mut stdin) = stdin {
                stdin.write_all(&bytes)?;
            }
            Ok(())
        })
    });
    let deadline = Instant::now() + timeout;
    let code = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.code().unwrap_or(1),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                if let Some(w) = writer {
                    let _ = w.join();
                }
                return Err(GraphError::unavailable("Git command timed out."));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                if let Some(w) = writer {
                    let _ = w.join();
                }
                return Err(GraphError::unavailable("Git command failed."));
            }
        }
    };
    if let Some(w) = writer {
        let _ = w.join();
    }
    let stdout = reader
        .join()
        .map_err(|_| GraphError::unavailable("Git output reader panicked."))?;
    Ok(GitOutput { code, stdout })
}
