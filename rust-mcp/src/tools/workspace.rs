use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

// ── Workspace root resolution ───────────────────────────────────────────────

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

// ── Multi-Tenant Workspace Manager & Registry ──────────────────────────────

fn workspaces_registry_file() -> PathBuf {
    if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        let dir = PathBuf::from(local_app_data).join("mysid");
        let _ = std::fs::create_dir_all(&dir);
        dir.join("workspaces.json")
    } else {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("."));
        exe.parent().unwrap_or_else(|| Path::new(".")).join("workspaces.json")
    }
}

fn is_valid_project_dir(path: &Path) -> bool {
    if !path.is_dir() {
        return false;
    }

    // Skip root paths like C:\ or \
    if let Some(parent) = path.parent() {
        if parent == path {
            return false;
        }
    } else {
        return false;
    }

    // Skip Windows system directories
    if let Some(sys_root) = std::env::var_os("SystemRoot") {
        let sys_path = PathBuf::from(sys_root);
        if path.starts_with(&sys_path) {
            return false;
        }
    }

    // Skip raw user profile folder e.g. C:\Users\username
    if let Some(user_profile) = std::env::var_os("USERPROFILE") {
        if path == Path::new(&user_profile) {
            return false;
        }
    }

    true
}

fn read_legacy_workspace() -> Option<String> {
    // 1. Check current working directory for .active_workspace
    if let Ok(cwd) = std::env::current_dir() {
        let candidate = cwd.join(".active_workspace");
        if candidate.exists() {
            if let Ok(content) = std::fs::read_to_string(&candidate) {
                let trimmed = content.trim().to_string();
                if !trimmed.is_empty() && Path::new(&trimmed).is_dir() {
                    return Some(trimmed);
                }
            }
        }
    }

    // 2. Check well-known Orchestra repo location
    let orchestra_ws = Path::new("E:\\Orchestra\\.active_workspace");
    if orchestra_ws.exists() {
        if let Ok(content) = std::fs::read_to_string(orchestra_ws) {
            let trimmed = content.trim().to_string();
            if !trimmed.is_empty() && Path::new(&trimmed).is_dir() {
                return Some(trimmed);
            }
        }
    }

    // 3. Check ~/.active_workspace
    if let Some(user_home) = std::env::var_os("USERPROFILE") {
        let home_ws = Path::new(&user_home).join(".active_workspace");
        if home_ws.exists() {
            if let Ok(content) = std::fs::read_to_string(home_ws) {
                let trimmed = content.trim().to_string();
                if !trimmed.is_empty() && Path::new(&trimmed).is_dir() {
                    return Some(trimmed);
                }
            }
        }
    }

    None
}

pub fn parse_file_uri(uri: &str) -> String {
    let mut s = uri.trim();
    if let Some(stripped) = s.strip_prefix("file://") {
        s = stripped;
    }

    // Percent-decoding (e.g. %20 -> space, %3A -> :)
    let mut decoded = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            let h1 = chars.next();
            let h2 = chars.next();
            if let (Some(c1), Some(c2)) = (h1, h2) {
                let hex_str = format!("{}{}", c1, c2);
                if let Ok(byte) = u8::from_str_radix(&hex_str, 16) {
                    decoded.push(byte as char);
                    continue;
                }
                decoded.push('%');
                decoded.push(c1);
                decoded.push(c2);
                continue;
            }
        }
        decoded.push(c);
    }

    // Handle Windows drive path: /E:/path or /e:/path -> E:\path
    let trimmed = decoded.trim();
    if trimmed.starts_with('/') && trimmed.len() >= 3 && trimmed.chars().nth(2) == Some(':') {
        trimmed[1..].replace('/', "\\")
    } else {
        trimmed.replace('/', "\\")
    }
}

#[derive(Debug, Clone)]
pub struct WorkspaceManager {
    pub current: Option<String>,
    pub registry: HashMap<String, String>, // alias (lowercase) -> canonical path
}

impl WorkspaceManager {
    pub fn new() -> Self {
        let mut mgr = Self {
            current: None,
            registry: HashMap::new(),
        };

        // 1. Load registry from persistent storage
        mgr.load_registry();

        // 2. Auto-detect from CWD (when launched from IDE or Antigravity window)
        if let Ok(cwd) = std::env::current_dir() {
            if is_valid_project_dir(&cwd) {
                let p_str = cwd.to_string_lossy().to_string();
                mgr.register_and_activate(&p_str, None);
            }
        }

        // 3. Fallback to legacy workspace if CWD wasn't a valid project folder
        if mgr.current.is_none() {
            if let Some(persisted) = read_legacy_workspace() {
                if Path::new(&persisted).is_dir() {
                    mgr.register_and_activate(&persisted, None);
                }
            }
        }

        mgr
    }

    pub fn load_registry(&mut self) {
        let file = workspaces_registry_file();
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                if let Some(reg_obj) = val.get("registry").and_then(|v| v.as_object()) {
                    for (k, v) in reg_obj {
                        if let Some(p) = v.as_str() {
                            self.registry.insert(k.to_lowercase(), p.to_string());
                        }
                    }
                }
            }
        }
    }

    pub fn save_registry(&self) {
        let file = workspaces_registry_file();
        let payload = json!({
            "registry": self.registry,
        });
        let _ = std::fs::write(&file, serde_json::to_string_pretty(&payload).unwrap_or_default());
    }

    pub fn register_and_activate(&mut self, path_str: &str, custom_alias: Option<&str>) {
        let path = Path::new(path_str);
        let final_path = if let Ok(canonical) = path.canonicalize() {
            canonical.to_string_lossy().trim_start_matches(r"\\?\").to_string()
        } else {
            path_str.trim().to_string()
        };

        // Register by folder name (e.g. "mysid-mcp")
        if let Some(name) = Path::new(&final_path).file_name().and_then(|n| n.to_str()) {
            self.registry.insert(name.to_lowercase(), final_path.clone());
        }

        // Register custom alias if provided
        if let Some(alias) = custom_alias {
            let trimmed = alias.trim().to_lowercase();
            if !trimmed.is_empty() {
                self.registry.insert(trimmed, final_path.clone());
            }
        }

        self.current = Some(final_path);
        self.save_registry();
    }

    pub fn switch_workspace(&mut self, query: &str) -> Result<String, String> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Err("Cannot switch to empty workspace path or alias.".to_string());
        }

        // 1. Check alias in registry (e.g., "mysid-mcp", "hypersonus", "backend")
        let lower = trimmed.to_lowercase();
        if let Some(target) = self.registry.get(&lower).cloned() {
            if Path::new(&target).is_dir() {
                self.current = Some(target.clone());
                return Ok(target);
            }
        }

        // 2. Try directly resolving as a path on disk
        let path = Path::new(trimmed);
        if path.is_dir() {
            self.register_and_activate(trimmed, None);
            return Ok(self.current.clone().unwrap_or_else(|| trimmed.to_string()));
        }

        Err(format!("Workspace '{}' not found in registry and does not exist as a directory.", trimmed))
    }

    pub fn get_current(&self) -> Option<String> {
        self.current.clone()
    }

    pub fn get_known_workspaces(&self) -> HashMap<String, String> {
        self.registry.clone()
    }
}

// ── Partitioned File Cache (LRU, sharded per workspace) ─────────────────────

fn cache_path(workspace: &str) -> PathBuf {
    let mut hasher = DefaultHasher::new();
    workspace.to_lowercase().hash(&mut hasher);
    let hash = hasher.finish();

    let dir = if let Some(local_app_data) = std::env::var_os("LOCALAPPDATA") {
        PathBuf::from(local_app_data).join("mysid").join("cache")
    } else {
        std::env::temp_dir().join("mysid").join("cache")
    };
    let _ = std::fs::create_dir_all(&dir);
    dir.join(format!("cache_{:x}.json", hash))
}

const MAX_CACHE_ENTRIES: usize = 150;

#[derive(Debug)]
struct WorkspaceCache {
    order: Vec<String>,
    map:   HashMap<String, String>,
}

fn read_cache(workspace: &str) -> WorkspaceCache {
    let path = cache_path(workspace);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
            // Check direct {order, map} structure
            if let Some(order_val) = val.get("order").and_then(|v| v.as_array()) {
                let order: Vec<String> = order_val
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
                let map: HashMap<String, String> = val
                    .get("map")
                    .and_then(|v| v.as_object())
                    .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                    .unwrap_or_default();
                return WorkspaceCache { order, map };
            }
            // Backward-compatibility: if workspace was keyed inside object
            if let Some(ws_val) = val.get(workspace) {
                let order: Vec<String> = ws_val
                    .get("order")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default();
                let map: HashMap<String, String> = ws_val
                    .get("map")
                    .and_then(|v| v.as_object())
                    .map(|o| o.iter().filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string()))).collect())
                    .unwrap_or_default();
                return WorkspaceCache { order, map };
            }
        }
    }
    WorkspaceCache { order: Vec::new(), map: HashMap::new() }
}

fn write_cache(workspace: &str, cache: &WorkspaceCache) {
    let path = cache_path(workspace);
    let payload = json!({
        "order": cache.order,
        "map": cache.map
    });
    let _ = std::fs::write(&path, serde_json::to_string_pretty(&payload).unwrap_or_default());
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

// ── Fuzzy file resolver ─────────────────────────────────────────────────────

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