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

/// Files/dirs whose presence marks a directory as a project root.
const PROJECT_MARKERS: &[&str] = &[
    ".mysid", ".git", "package.json", "Cargo.toml", "pom.xml", "build.gradle", "build.gradle.kts",
    "settings.gradle", "settings.gradle.kts", "pyproject.toml", "setup.py", "go.mod",
    "CMakeLists.txt", "Package.swift",
];

/// Nearest ancestor of `start` (inclusive) that looks like a project root.
/// Stops before drive roots, system folders and the user profile directory.
pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if !is_valid_project_dir(dir) {
            break;
        }
        if PROJECT_MARKERS.iter().any(|m| dir.join(m).exists()) {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Project root for the current directory: nearest marker-bearing ancestor, else the directory
/// itself when it is an ordinary folder.
fn infer_from_cwd() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    find_project_root(&cwd).or_else(|| if is_valid_project_dir(&cwd) { Some(cwd) } else { None })
}

/// Where a command's workspace came from; only `Saved` is worth telling the caller about.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Source {
    Env,
    /// `set_workspace` run from this folder (or one above it).
    Pin,
    /// The project around the current folder.
    Project,
    /// The old machine-wide saved workspace: used only when the current folder says nothing.
    Saved,
}

fn source_text(s: Source) -> &'static str {
    match s {
        Source::Env => "the MYSID_WORKSPACE environment variable",
        Source::Pin => "a `mysid set` pin for this folder (made in this session)",
        Source::Project => "the project around the current folder",
        Source::Saved => "the saved workspace (the current folder is not inside a project)",
    }
}

/// The workspace for a command run in `cwd`, from state that belongs to that command alone:
/// `MYSID_WORKSPACE`, a pin made by `set_workspace` in this folder or above, the project around `cwd`,
/// and only then the old machine-wide saved workspace. Two agents in two folders never see each other.
pub fn resolve_for(env_ws: Option<&str>, cwd: &Path, pins: &HashMap<String, String>, saved: Option<&str>) -> Option<(PathBuf, Source)> {
    resolve_for_session(env_ws, None, cwd, pins, saved)
}

/// Pins are filed under the session that made them (`claude:18892|d:\work`), so an agent that shares a folder with
/// another agent, or whose shell is reset to that folder before every command, still gets its own choice. Without a
/// session (no identifiable parent process) the folder alone is the key, as before.
pub fn pin_key(session: Option<&str>, dir: &Path) -> String {
    match session {
        Some(s) => format!("{}|{}", s, canon_lower(dir)),
        None => canon_lower(dir),
    }
}

pub fn resolve_for_session(
    env_ws: Option<&str>,
    session: Option<&str>,
    cwd: &Path,
    pins: &HashMap<String, String>,
    saved: Option<&str>,
) -> Option<(PathBuf, Source)> {
    if let Some(e) = env_ws.map(str::trim).filter(|e| !e.is_empty() && Path::new(e).is_dir()) {
        return Some((PathBuf::from(e), Source::Env));
    }
    if !pins.is_empty() {
        for dir in cwd.ancestors() {
            if let Some(target) = pins.get(&pin_key(session, dir)).filter(|t| Path::new(t.as_str()).is_dir()) {
                return Some((PathBuf::from(target), Source::Pin));
            }
        }
    }
    if let Some(root) = find_project_root(cwd).or_else(|| if is_valid_project_dir(cwd) { Some(cwd.to_path_buf()) } else { None }) {
        return Some((root, Source::Project));
    }
    saved.filter(|s| Path::new(s).is_dir()).map(|s| (PathBuf::from(s), Source::Saved))
}

/// The project that the absolute paths in a command's arguments belong to (`mysid read D:\apps\web-ui\src\App.tsx#L1-9`),
/// so an agent running from somewhere else still edits the right project. None when there are none, or they disagree.
pub fn root_from_paths(params: &Value) -> Option<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut consider = |tok: &str| {
        let tok = tok.trim().trim_matches('"');
        let tok = tok.split('#').next().unwrap_or(tok);
        // `C:\x\A.kt:12` -> drop a trailing line number, but keep the drive colon.
        let tok = match tok.rfind(':') {
            Some(i) if i > 1 && tok[i + 1..].chars().all(|c| c.is_ascii_digit() || c == '-') => &tok[..i],
            _ => tok,
        };
        let p = Path::new(tok);
        if p.is_absolute() && p.exists() {
            let start = if p.is_dir() { p } else { p.parent().unwrap_or(p) };
            if let Some(r) = find_project_root(start) {
                if !roots.iter().any(|x| canon_lower(x) == canon_lower(&r)) {
                    roots.push(r);
                }
            }
        }
    };
    for key in ["path", "file", "files", "query", "symbol", "old_path", "new_path"] {
        match params.get(key) {
            Some(Value::String(s)) => s.split_whitespace().for_each(&mut consider),
            Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).for_each(&mut consider),
            _ => {}
        }
    }
    if roots.len() == 1 { roots.pop() } else { None }
}

fn canon_lower(p: &Path) -> String {
    p.canonicalize()
        .map(|c| c.to_string_lossy().trim_start_matches(r"\\?\").to_string())
        .unwrap_or_else(|_| p.to_string_lossy().to_string())
        .to_lowercase()
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
    /// Set only by an explicit `set_workspace`; survives across CLI invocations.
    pub persisted_current: Option<String>,
    /// One-line heads-up shown to the caller (e.g. cwd is in a different project).
    pub notice: Option<String>,
    /// `set_workspace` run from a folder pins that folder (and the ones below it) to a workspace. Keyed by
    /// the folder, so each agent's own working folder is its own scope.
    pub pins: HashMap<String, String>,
    /// How `current` was chosen, for `get_workspace`.
    pub resolved_from: Option<&'static str>,
    /// One-shot command line (as opposed to the long-lived MCP server).
    cli: bool,
    /// The agent or terminal this command runs under (see `session`): pins are filed under it.
    session: Option<String>,
}

impl WorkspaceManager {
    /// Long-lived (MCP) startup: `MYSID_WORKSPACE`, else the launching client's directory, else what was saved.
    pub fn new() -> Self {
        let mut mgr = Self::empty();
        mgr.load_registry();
        let cwd = std::env::current_dir().unwrap_or_default();
        let env_ws = std::env::var("MYSID_WORKSPACE").ok();
        if let Some((root, source)) = resolve_for(env_ws.as_deref(), &cwd, &HashMap::new(), None) {
            mgr.activate(&root.to_string_lossy(), source != Source::Env);
            mgr.resolved_from = Some(source_text(source));
        }
        if mgr.current.is_none() {
            mgr.restore_persisted();
        }
        if mgr.current.is_none() {
            if let Some(persisted) = read_legacy_workspace() {
                if Path::new(&persisted).is_dir() {
                    mgr.activate(&persisted, true);
                }
            }
        }
        mgr
    }

    /// One-shot CLI startup. The workspace comes from the folder the command runs in (see `resolve_for`), never from
    /// state another terminal or agent could have changed; the saved workspace is a last resort and says so.
    pub fn new_cli() -> Self {
        let mut mgr = Self::empty();
        mgr.cli = true;
        mgr.load_registry();
        let cwd = std::env::current_dir().unwrap_or_default();
        let env_ws = std::env::var("MYSID_WORKSPACE").ok();
        let saved = mgr.persisted_current.clone().or_else(read_legacy_workspace);
        mgr.session = super::session::current();
        if let Some((root, source)) = resolve_for_session(env_ws.as_deref(), mgr.session.as_deref(), &cwd, &mgr.pins, saved.as_deref()) {
            mgr.activate(&root.to_string_lossy(), matches!(source, Source::Project));
            mgr.resolved_from = Some(source_text(source));
            if source == Source::Saved {
                mgr.notice = Some(format!(
                    "[workspace: {} (saved earlier; this folder is not in a project). Run mysid inside a project folder, or pass --workspace_root]",
                    root.display()
                ));
            }
        }
        mgr
    }

    /// Makes `path` the active workspace without writing anything (the registry is touched only for a new alias).
    fn activate(&mut self, path: &str, register: bool) {
        if register {
            self.register_and_activate(path, None);
        } else {
            self.current = Some(path.to_string());
        }
    }

    fn empty() -> Self {
        Self {
            current: None,
            registry: HashMap::new(),
            persisted_current: None,
            notice: None,
            pins: HashMap::new(),
            resolved_from: None,
            cli: false,
            session: None,
        }
    }

    /// Activates the persisted explicit workspace if it still exists on disk.
    fn restore_persisted(&mut self) -> bool {
        match self.persisted_current.clone() {
            Some(p) if Path::new(&p).is_dir() => {
                self.current = Some(p);
                true
            }
            _ => false,
        }
    }

    /// Command line only: later commands run from this folder (or below) use `path`. Nothing is shared with other
    /// folders. Over MCP the choice lives in this server process alone.
    fn pin_here(&mut self, path: &str) {
        if !self.cli {
            return;
        }
        if let Ok(cwd) = std::env::current_dir() {
            // Another agent may have pinned in the meantime: merge what is on disk first, drop pins of sessions that
            // have ended, then add this one, so nobody's choice is lost or kept past its session.
            self.merge_pins_from_disk();
            let table = super::session::table();
            self.pins.retain(|k, _| k.split_once('|').map_or(true, |(s, _)| super::session::alive(s, &table)));
            self.pins.insert(pin_key(self.session.as_deref(), &cwd), path.to_string());
            self.save_registry();
        }
    }

    fn merge_pins_from_disk(&mut self) {
        if let Ok(text) = std::fs::read_to_string(workspaces_registry_file()) {
            if let Some(pins) = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v.get("pins").and_then(|p| p.as_object().cloned())) {
                for (k, v) in pins {
                    if let Some(p) = v.as_str() {
                        self.pins.entry(k).or_insert_with(|| p.to_string());
                    }
                }
            }
        }
    }

    /// A workspace named by alias (`--workspace_root web-ui`) when it is not a folder itself.
    pub fn resolve_alias(&self, name: &str) -> Option<String> {
        if Path::new(name).is_dir() {
            return None;
        }
        self.registry.get(&name.trim().to_lowercase()).filter(|p| Path::new(p.as_str()).is_dir()).cloned()
    }

    pub fn load_registry(&mut self) {
        let file = workspaces_registry_file();
        if let Ok(text) = std::fs::read_to_string(&file) {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                self.persisted_current = val.get("current").and_then(|v| v.as_str()).map(|s| s.to_string());
                if let Some(pins) = val.get("pins").and_then(|v| v.as_object()) {
                    for (k, v) in pins {
                        if let Some(p) = v.as_str() {
                            self.pins.insert(k.clone(), p.to_string());
                        }
                    }
                }
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
            "current": self.persisted_current,
            "pins": self.pins,
        });
        // Write-then-rename: another agent reading (or writing) at the same moment never sees half a file.
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, serde_json::to_string_pretty(&payload).unwrap_or_default()).is_ok() {
            if std::fs::rename(&tmp, &file).is_err() {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    pub fn register_and_activate(&mut self, path_str: &str, custom_alias: Option<&str>) {
        let path = Path::new(path_str);
        let final_path = if let Ok(canonical) = path.canonicalize() {
            canonical.to_string_lossy().trim_start_matches(r"\\?\").to_string()
        } else {
            path_str.trim().to_string()
        };

        let before = self.registry.clone();

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
        // Most commands only re-activate a known project: no file write (and no race) unless something is new.
        if self.registry != before {
            self.save_registry();
        }
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
                self.pin_here(&target);
                return Ok(target);
            }
        }

        // 2. Try directly resolving as a path on disk
        let path = Path::new(trimmed);
        if path.is_dir() {
            self.register_and_activate(trimmed, None);
            let active = self.current.clone().unwrap_or_else(|| trimmed.to_string());
            self.pin_here(&active);
            return Ok(active);
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

    // Step 2: cache lookup (bare names only: `ui/Foo.kt` must not be answered with another `Foo.kt`)
    let is_bare = !name.contains('/') && !name.contains('\\');
    if is_bare {
        let cache = read_cache(workspace);
        if let Some(cached_rel) = cache.map.get(bare_name) {
            let cached_path = root.join(cached_rel);
            if cached_path.exists() {
                cache_put(workspace, bare_name, cached_rel);
                return Ok(cached_path);
            }
            // Stale entry, fall through to the file list
        }
    }

    // Step 3: the workspace file list (gitignore-aware, minus build output and vendored dirs)
    let matches = super::scanner::paths::find_by_name(&crate::index::scan::list_files(root), name);
    match matches.len() {
        0 => Err(json!({
            "success": false,
            "error": format!("'{}' not found in workspace (searched: {}). Pass a file name such as Foo.kt, or part of its path.", name, root.display())
        }).to_string()),
        1 => {
            if is_bare {
                cache_put(workspace, bare_name, &matches[0]);
            }
            Ok(root.join(&matches[0]))
        }
        _ => Err(json!({
            "success": false,
            "error": format!("'{}' is ambiguous - {} files; repeat with a folder, e.g. one of:\n{}", name, matches.len(),
                matches.iter().map(|m| format!("  {}", ambiguous_hint(&matches, m))).collect::<Vec<_>>().join("\n"))
        }).to_string()),
    }
}

/// The readable text of a `resolve_file_in_workspace` error (it is a JSON object with an `error` field).
pub fn resolution_message(err: &str) -> String {
    serde_json::from_str::<Value>(err).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or_else(|| err.to_string())
}

/// Like `resolve_file_in_workspace`, but a name that cannot be resolved (missing or ambiguous) comes back
/// as the plain `root.join(name)`, so the caller's own "not found" handling still applies.
pub fn resolve_or_join(root: &Path, name: &str, workspace: &str) -> PathBuf {
    resolve_file_in_workspace(root, name, workspace).unwrap_or_else(|_| root.join(name))
}

/// The shortest folder-qualified name that tells `m` apart from the other candidates, e.g. `ui/Foo.kt`.
fn ambiguous_hint(all: &[String], m: &str) -> String {
    let parts: Vec<&str> = m.split('/').collect();
    for take in 2..=parts.len() {
        let tail = parts[parts.len() - take..].join("/");
        if all.iter().filter(|g| g.as_str() == tail || g.ends_with(&format!("/{}", tail))).count() == 1 {
            return format!("{}   ({})", tail, m);
        }
    }
    m.to_string()
}

#[cfg(test)]
mod resolver_tests {
    use super::*;
    use std::fs;

    fn two_projects(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let top = std::env::temp_dir().join(format!("mysid_ws_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&top);
        let (a, b) = (top.join("web-ui"), top.join("ui-host"));
        fs::create_dir_all(a.join("src")).unwrap();
        fs::create_dir_all(b.join("native/deep")).unwrap();
        fs::write(a.join("package.json"), "{}").unwrap();
        fs::write(b.join("Host.sln"), "").unwrap();
        fs::create_dir_all(b.join(".git")).unwrap();
        fs::write(a.join("src/App.tsx"), "x").unwrap();
        (top, a, b)
    }

    #[test]
    fn each_folder_gets_its_own_project_with_no_shared_state() {
        let (top, a, b) = two_projects("each");
        let none = HashMap::new();
        let (ra, sa) = resolve_for(None, &a.join("src"), &none, None).unwrap();
        let (rb, sb) = resolve_for(None, &b.join("native/deep"), &none, None).unwrap();
        assert_eq!((canon_lower(&ra), sa), (canon_lower(&a), Source::Project));
        assert_eq!((canon_lower(&rb), sb), (canon_lower(&b), Source::Project));
        // a saved workspace elsewhere changes nothing while the folder is inside a project
        let (r, s) = resolve_for(None, &a.join("src"), &none, Some(&b.to_string_lossy())).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&a), Source::Project));
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn env_wins_then_pins_by_folder_and_the_saved_workspace_is_only_a_last_resort() {
        let (top, a, b) = two_projects("order");
        let mut pins = HashMap::new();
        pins.insert(canon_lower(&a), b.to_string_lossy().to_string());
        // set_workspace was run from web-ui (pinned to ui-host): subfolders follow the pin, other folders do not
        let (r, s) = resolve_for(None, &a.join("src"), &pins, None).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&b), Source::Pin));
        let (r, s) = resolve_for(None, &b, &pins, None).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&b), Source::Project));
        // the environment overrides everything
        let (r, s) = resolve_for(Some(&a.to_string_lossy()), &b, &pins, None).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&a), Source::Env));
        // a folder that is no project at all falls back to the saved workspace, and says so
        let nowhere = top.join("does-not-exist");
        let (r, s) = resolve_for(None, &nowhere, &HashMap::new(), Some(&a.to_string_lossy())).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&a), Source::Saved));
        assert!(resolve_for(None, &nowhere, &HashMap::new(), None).is_none());
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn pins_belong_to_the_session_that_made_them() {
        let (top, a, b) = two_projects("sessions");
        let shared = top.join("shared-folder");
        fs::create_dir_all(&shared).unwrap();
        // Two agents share one working folder; each pinned a different project from it.
        let mut pins = HashMap::new();
        pins.insert(pin_key(Some("claude:100"), &shared), a.to_string_lossy().to_string());
        pins.insert(pin_key(Some("claude:200"), &shared), b.to_string_lossy().to_string());
        let (r, s) = resolve_for_session(None, Some("claude:100"), &shared, &pins, None).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&a), Source::Pin));
        let (r, s) = resolve_for_session(None, Some("claude:200"), &shared, &pins, None).unwrap();
        assert_eq!((canon_lower(&r), s), (canon_lower(&b), Source::Pin));
        // A session that pinned nothing does not inherit another's pin (and the folder is no project here).
        assert!(!matches!(resolve_for_session(None, Some("claude:300"), &shared, &pins, None), Some((_, Source::Pin))));
        // Without a session the old folder-only key applies.
        let mut old = HashMap::new();
        old.insert(pin_key(None, &shared), a.to_string_lossy().to_string());
        assert_eq!(resolve_for_session(None, None, &shared, &old, None).unwrap().1, Source::Pin);
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn absolute_paths_in_arguments_name_their_project() {
        let (top, a, b) = two_projects("paths");
        let file = a.join("src/App.tsx").to_string_lossy().to_string();
        let r = root_from_paths(&json!({ "query": format!("{}#L1-4", file) })).unwrap();
        assert_eq!(canon_lower(&r), canon_lower(&a));
        let r = root_from_paths(&json!({ "files": [format!("{}:12", file)] })).unwrap();
        assert_eq!(canon_lower(&r), canon_lower(&a));
        // two different projects in one command: ambiguous, so no guess
        let other = b.join("Host.sln").to_string_lossy().to_string();
        assert!(root_from_paths(&json!({ "files": [file.clone(), other] })).is_none());
        // relative names say nothing
        assert!(root_from_paths(&json!({ "query": "App.tsx" })).is_none());
        let _ = fs::remove_dir_all(&top);
    }
}
