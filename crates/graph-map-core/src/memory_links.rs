//! Standalone Project Brain & Memory Bank citation reader for file-level memory links.
use crate::{GraphError, Result, util};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Component, Path, PathBuf},
};

pub const MEMORY_LINKS_BATCH_LIMIT: usize = 128;

pub fn clean_relative_path(raw: &str) -> Option<String> {
    let trimmed = raw.trim().replace('\\', "/");
    let stripped = trimmed
        .strip_prefix("./")
        .unwrap_or(&trimmed)
        .trim_start_matches('/');
    if stripped.is_empty() || stripped.len() > 1024 || stripped.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for comp in Path::new(stripped).components() {
        match comp {
            Component::Normal(s) => {
                let part = s.to_str()?;
                if !part.is_empty() {
                    parts.push(part);
                }
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn collapse_preview(raw: &str, max_chars: usize) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut out = String::new();
    for (idx, ch) in collapsed.chars().enumerate() {
        if idx >= max_chars {
            out.push('…');
            break;
        }
        out.push(ch);
    }
    out
}

pub fn memory_links(root: &Path, query: &Value) -> Result<Value> {
    let path = query["path"]
        .as_str()
        .and_then(clean_relative_path)
        .ok_or_else(|| GraphError::invalid("Choose a file of the project."))?;
    let previous = query["previous_path"]
        .as_str()
        .filter(|prev| !prev.trim().is_empty())
        .map(|prev| {
            clean_relative_path(prev)
                .ok_or_else(|| GraphError::invalid("Choose a file of the project."))
        })
        .transpose()?
        .filter(|prev| *prev != path);
    let items = memory_links_for_files(root, &[(path, previous)])?;
    items
        .into_iter()
        .next()
        .ok_or_else(|| GraphError::unavailable("Memory links item missing."))
}

pub fn memory_links_batch(root: &Path, project_id: &str, body: &Value) -> Result<Value> {
    if !util::exact(body, &["files"], &[]) {
        return Err(GraphError::invalid(
            "Provide a files list for memory links.",
        ));
    }
    let raw_files = body["files"]
        .as_array()
        .ok_or_else(|| GraphError::invalid("Provide a files list for memory links."))?;
    if raw_files.len() > MEMORY_LINKS_BATCH_LIMIT {
        return Err(GraphError::invalid(format!(
            "Request at most {MEMORY_LINKS_BATCH_LIMIT} files per memory-links batch."
        )));
    }
    let mut specs = Vec::with_capacity(raw_files.len());
    for entry in raw_files {
        if !util::exact(entry, &["path"], &["previous_path"]) {
            return Err(GraphError::invalid(
                "Each batch file must provide a path and optional previous_path.",
            ));
        }
        let path = entry["path"]
            .as_str()
            .and_then(clean_relative_path)
            .ok_or_else(|| GraphError::invalid("Choose a file of the project."))?;
        let previous = match entry.get("previous_path") {
            None | Some(Value::Null) => None,
            Some(Value::String(raw)) if raw.trim().is_empty() => None,
            Some(Value::String(raw)) => Some(
                clean_relative_path(raw)
                    .ok_or_else(|| GraphError::invalid("Choose a file of the project."))?,
            ),
            Some(_) => return Err(GraphError::invalid("Choose a file of the project.")),
        }
        .filter(|prev| *prev != path);
        specs.push((path, previous));
    }
    let items = memory_links_for_files(root, &specs)?;
    Ok(json!({
        "project_id": project_id,
        "items": items,
    }))
}

pub fn memory_links_for_files(
    root: &Path,
    specs: &[(String, Option<String>)],
) -> Result<Vec<Value>> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let mut wanted = BTreeSet::new();
    for (path, previous) in specs {
        wanted.insert(path.clone());
        if let Some(prev) = previous {
            wanted.insert(prev.clone());
        }
    }
    let pick_cited = |hits: &[String], path: &str, previous: Option<&str>| -> Option<String> {
        hits.iter()
            .find(|cited| cited.as_str() == path || previous == Some(cited.as_str()))
            .cloned()
    };

    let mut per_brain: Vec<Vec<Value>> = vec![Vec::new(); specs.len()];
    let brain_dir = root.join("project-brain");
    if brain_dir.is_dir() && !is_symlink(&brain_dir) {
        let mut json_files = Vec::new();
        collect_regular_files(&brain_dir, "json", 4096, &mut json_files);
        for file in json_files {
            if file
                .strip_prefix(&brain_dir)
                .ok()
                .and_then(|rel| rel.components().next())
                .is_some_and(|c| c.as_os_str() == "config")
            {
                continue;
            }
            let Ok(bytes) = read_regular_limited(&file, 1024 * 1024) else {
                continue;
            };
            let Ok(r) = util::strict_json(&bytes, 1024 * 1024) else {
                continue;
            };
            if !r.is_object() {
                continue;
            }
            let matching = |values: &Value| -> Vec<String> {
                values
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .filter_map(|raw| clean_relative_path(raw.split('#').next().unwrap_or(raw)))
                    .filter(|cited| wanted.contains(cited))
                    .collect()
            };
            let file_hits = matching(&r["files"]);
            let source_hits = matching(&r["sources"]);
            if file_hits.is_empty() && source_hits.is_empty() {
                continue;
            }
            let title = collapse_preview(r["title"].as_str().unwrap_or(""), 240);
            for (idx, (path, previous)) in specs.iter().enumerate() {
                let Some(cited) = pick_cited(&file_hits, path, previous.as_deref())
                    .or_else(|| pick_cited(&source_hits, path, previous.as_deref()))
                else {
                    continue;
                };
                per_brain[idx].push(json!({
                    "id": r["id"],
                    "external_id": r["external_id"],
                    "type": r["type"],
                    "title": title,
                    "status": r["status"],
                    "authority": r["authority"],
                    "cited_path": cited,
                }));
            }
        }
    }

    let mut per_chunks: Vec<Vec<Value>> = vec![Vec::new(); specs.len()];
    let bank_dir = root.join("memory-bank");
    if bank_dir.is_dir() && !is_symlink(&bank_dir) {
        let mut md_files = Vec::new();
        let chunks_dir = bank_dir.join("chunks");
        if chunks_dir.is_dir() && !is_symlink(&chunks_dir) {
            collect_regular_files(&chunks_dir, "md", 4096, &mut md_files);
        }
        collect_regular_files_depth(&bank_dir, "md", 1, 4096, &mut md_files);
        for file in md_files {
            let Some(name) = file.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if !name.starts_with("MEM-") && !file.starts_with(&chunks_dir) {
                continue;
            }
            let Ok(bytes) = read_regular_limited(&file, 512 * 1024) else {
                continue;
            };
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            let Some(meta) = parse_frontmatter(text) else {
                continue;
            };
            if meta.status.as_deref().is_some_and(|s| s != "active") {
                continue;
            }
            let source_hits: Vec<String> = meta
                .sources
                .iter()
                .filter_map(|raw| clean_relative_path(raw.split('#').next().unwrap_or(raw)))
                .filter(|cited| wanted.contains(cited))
                .collect();
            if source_hits.is_empty() {
                continue;
            }
            let id = meta
                .id
                .clone()
                .unwrap_or_else(|| name.trim_end_matches(".md").to_owned());
            let title = collapse_preview(meta.title.as_deref().unwrap_or(""), 240);
            let auto_promoted = meta.tags.iter().any(|tag| tag == "auto-promoted");
            for (idx, (path, previous)) in specs.iter().enumerate() {
                let Some(cited) = pick_cited(&source_hits, path, previous.as_deref()) else {
                    continue;
                };
                per_chunks[idx].push(json!({
                    "id": id,
                    "title": title,
                    "type": meta.kind,
                    "review_after": meta.review_after,
                    "auto_promoted": auto_promoted,
                    "cited_path": cited,
                }));
            }
        }
    }

    let mut items = Vec::with_capacity(specs.len());
    for ((path, previous), (brain_records, memory_chunks)) in
        specs.iter().zip(per_brain.into_iter().zip(per_chunks))
    {
        let linked = |name: &str| {
            brain_records
                .iter()
                .chain(&memory_chunks)
                .filter(|row| row["cited_path"] == name)
                .filter_map(|row| row["id"].as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        let mut hygiene_warnings = Vec::new();
        let gone = !root.join(path).exists();
        let here = linked(path);
        if gone && !here.is_empty() {
            hygiene_warnings.push(json!({
                "kind": "cited-file-missing",
                "path": path,
                "linked": here,
                "recommended_action": "review",
                "text": "This file is gone from the workspace; the memory that cites it may be stale.",
            }));
        }
        if let Some(prev) = previous {
            let old = linked(prev);
            if !old.is_empty() {
                hygiene_warnings.push(json!({
                    "kind": "cited-file-renamed",
                    "path": path,
                    "previous_path": prev,
                    "linked": old,
                    "recommended_action": "review",
                    "text": format!("This file was {prev}; the memory that cites the old name may need its citation updated."),
                }));
            }
        }
        items.push(json!({
            "path": path,
            "previous_path": previous,
            "brain_records": brain_records,
            "memory_chunks": memory_chunks,
            "omnigraph_atoms": [],
            "hygiene_warnings": hygiene_warnings,
            "contradiction_warnings": [],
        }));
    }
    Ok(items)
}

fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|m| m.file_type().is_symlink())
        .unwrap_or(true)
}

fn read_regular_limited(path: &Path, limit: usize) -> std::io::Result<Vec<u8>> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > limit as u64 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "skipped",
        ));
    }
    fs::read(path)
}

fn collect_regular_files(dir: &Path, ext: &str, max_files: usize, out: &mut Vec<PathBuf>) {
    collect_regular_files_depth(dir, ext, 8, max_files, out);
}

fn collect_regular_files_depth(
    dir: &Path,
    ext: &str,
    depth: usize,
    max_files: usize,
    out: &mut Vec<PathBuf>,
) {
    if out.len() >= max_files || depth == 0 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut sorted = entries.filter_map(|e| e.ok()).collect::<Vec<_>>();
    sorted.sort_by_key(|e| e.file_name());
    for entry in sorted {
        if out.len() >= max_files {
            break;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            let name = entry.file_name();
            if name != ".git" && name != "local" {
                collect_regular_files_depth(&path, ext, depth - 1, max_files, out);
            }
        } else if meta.is_file() && path.extension().and_then(|s| s.to_str()) == Some(ext) {
            out.push(path);
        }
    }
}

struct Frontmatter {
    id: Option<String>,
    title: Option<String>,
    kind: Option<String>,
    status: Option<String>,
    review_after: Option<String>,
    tags: Vec<String>,
    sources: Vec<String>,
}

fn parse_frontmatter(text: &str) -> Option<Frontmatter> {
    let rest = text.strip_prefix("---\n").or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---").or_else(|| rest.find("\r\n---"))?;
    let yaml = &rest[..end];
    let mut fm = Frontmatter {
        id: None,
        title: None,
        kind: None,
        status: None,
        review_after: None,
        tags: Vec::new(),
        sources: Vec::new(),
    };
    enum ListField {
        None,
        Tags,
        Sources,
    }
    let mut active_list = ListField::None;
    for raw_line in yaml.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(item) = trimmed.strip_prefix("- ") {
            let val = unquote_yaml(item.trim());
            match active_list {
                ListField::Tags if !val.is_empty() => fm.tags.push(val),
                ListField::Sources if !val.is_empty() => fm.sources.push(val),
                _ => {}
            }
            continue;
        }
        active_list = ListField::None;
        let Some((key, val_raw)) = trimmed.split_once(':') else {
            continue;
        };
        let key = key.trim();
        let val_raw = val_raw.trim();
        match key {
            "id" => fm.id = non_empty_yaml(val_raw),
            "title" => fm.title = non_empty_yaml(val_raw),
            "type" => fm.kind = non_empty_yaml(val_raw),
            "status" => fm.status = non_empty_yaml(val_raw),
            "review_after" => fm.review_after = non_empty_yaml(val_raw),
            "tags" => {
                if let Some(items) = inline_yaml_list(val_raw) {
                    fm.tags.extend(items);
                } else {
                    active_list = ListField::Tags;
                }
            }
            "sources" => {
                if let Some(items) = inline_yaml_list(val_raw) {
                    fm.sources.extend(items);
                } else {
                    active_list = ListField::Sources;
                }
            }
            _ => {}
        }
    }
    Some(fm)
}

fn unquote_yaml(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn non_empty_yaml(raw: &str) -> Option<String> {
    let val = unquote_yaml(raw);
    (!val.is_empty() && val != "null" && val != "~").then_some(val)
}

fn inline_yaml_list(raw: &str) -> Option<Vec<String>> {
    let inner = raw.strip_prefix('[')?.strip_suffix(']')?;
    Some(
        inner
            .split(',')
            .map(unquote_yaml)
            .filter(|s| !s.is_empty())
            .collect(),
    )
}
