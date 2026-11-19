use crate::{GraphError, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

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

pub fn canonical_dir(path: &Path) -> Result<PathBuf> {
    let resolved = std::fs::canonicalize(path)?;
    if !resolved.is_dir() {
        return Err(GraphError::invalid(
            "Project path must be an existing directory.",
        ));
    }
    Ok(resolved)
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

pub fn git_run(root: &Path, args: &[String], seconds: u64) -> Result<GitOutput> {
    if seconds == 0 {
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
        .args(args)
        .stdin(Stdio::null())
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
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 8192];
        let limit: usize = 16 * 1024 * 1024;
        while let Ok(read) = stdout.read(&mut chunk) {
            if read == 0 {
                break;
            }
            let room = limit.saturating_sub(bytes.len());
            bytes.extend_from_slice(&chunk[..read.min(room)]);
        }
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(seconds);
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
                return Err(GraphError::unavailable("Git command timed out."));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(GraphError::unavailable("Git command failed."));
            }
        }
    };
    let stdout = reader
        .join()
        .map_err(|_| GraphError::unavailable("Git output reader panicked."))?;
    Ok(GitOutput { code, stdout })
}
