use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// Workspace root resolution

pub fn resolve_root(params: &Value, key: &str, default_root: Option<&str>) -> Result<String, String> {
    if let Some(v) = params.get(key).and_then(|v| v.as_str()) {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    match default_root {
        Some(r) => Ok(r.to_string()),
        None => Err(json!({
            "success": false,
            "error": format!("No workspace set. Pass `{}` in your request or call set_workspace first.", key)
        }).to_string()),
    }
}

pub fn resolve_existing_root(params: &Value, key: &str, default_root: Option<&str>) -> Result<String, String> {
    let root_str = resolve_root(params, key, default_root)?;
    if !Path::new(&root_str).exists() {
        return Err(json!({
            "success": false,
            "error": format!("Workspace root does not exist: {}", root_str)
        }).to_string());
    }
    Ok(root_str)
}

// File cache (LRU, disk-persisted, max 10 entries per workspace)

fn cache_path() -> PathBuf {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
    exe.parent().unwrap_or_else(|| Path::new(".")).join(".mcp_file_cache.json")
}

const MAX_CACHE_ENTRIES: usize = 150;

#[derive(Debug)]
struct WorkspaceCache {
    order: Vec<String>,
    map:   HashMap<String, String>,
}

fn read_cache(workspace: &str) -> WorkspaceCache {
    let path = cache_path();
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(root_val) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(ws_val) = root_val.get(workspace) {
                let order: Vec<String> = ws_val
                    .get("order").and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                let map: HashMap<String, String> = ws_val
                    .get("map").and_then(|v| v.as_object())
                    .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                    .unwrap_or_default();
                return WorkspaceCache { order, map };
            }
        }
    }
    WorkspaceCache { order: Vec::new(), map: HashMap::new() }
}

fn write_cache(workspace: &str, cache: &WorkspaceCache) {
    let path = cache_path();
    let mut root_obj: serde_json::Map<String, serde_json::Value> =
        std::fs::read_to_string(&path).ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
    root_obj.insert(workspace.to_string(), json!({ "order": cache.order, "map": cache.map }));
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&root_obj).unwrap_or_default());
}

fn cache_put(workspace: &str, filename: &str, rel_path: &str) {
    let mut cache = read_cache(workspace);
    cache.order.retain(|k| k != filename);
    cache.map.remove(filename);
    cache.order.insert(0, filename.to_string());
    cache.map.insert(filename.to_string(), rel_path.to_string());
    while cache.order.len() > MAX_CACHE_ENTRIES {
        if let Some(evicted) = cache.order.pop() {
            cache.map.remove(&evicted);
        }
    }
    write_cache(workspace, &cache);
}

// Fuzzy file resolver
//
// 1. Try exact path (absolute or relative to root).
// 2. For bare filenames (no /): check LRU cache, then WalkDir.
//    1 match -> cache + return.  0 -> error.  2+ -> list candidates.

pub fn resolve_file_in_workspace(root: &Path, name: &str, workspace: &str) -> Result<PathBuf, String> {
    // Step 1: exact resolution
    let candidate = if Path::new(name).is_absolute() {
        PathBuf::from(name)
    } else {
        root.join(name)
    };
    if candidate.exists() {
        return Ok(candidate);
    }

    // Fallback: If exact path not found, extract bare filename for LRU cache / WalkDir
    let bare_name = Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);

    // Step 2: cache lookup
    let cache = read_cache(workspace);
    if let Some(cached_rel) = cache.map.get(bare_name) {
        let cached_path = root.join(cached_rel);
        if cached_path.exists() {
            cache_put(workspace, bare_name, cached_rel);
            return Ok(cached_path);
        }
        // Stale entry, fall through to WalkDir
    }

    // Step 3: WalkDir (gitignore-aware)
    let mut matches: Vec<String> = Vec::new();
    for result in WalkBuilder::new(root).hidden(false).git_ignore(true).git_global(true).git_exclude(true).build() {
        let entry = match result { Ok(e) => e, Err(_) => continue };
        if entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
            if entry.file_name().to_string_lossy() == bare_name {
                if let Ok(rel) = entry.path().strip_prefix(root) {
                    matches.push(rel.to_string_lossy().replace('\\', "/"));
                }
            }
        }
    }

    match matches.len() {
        0 => Err(json!({
            "success": false,
            "error": format!("'{}' not found in workspace (searched: {})", name, root.display())
        }).to_string()),
        1 => {
            cache_put(workspace, bare_name, &matches[0]);
            Ok(root.join(&matches[0]))
        }
        _ => Err(json!({
            "success": false,
            "error": format!("'{}' is ambiguous - {} files found:\n{}", name, matches.len(),
                matches.iter().map(|m| format!("  {}", m)).collect::<Vec<_>>().join("\n"))
        }).to_string()),
    }
}