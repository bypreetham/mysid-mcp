use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::resolve_root;

/// Create a new file (or overwrite) with content. Auto-creates parent directories.
pub fn execute_create_file(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `path` parameter." }).to_string(),
    };
    let content = match params.get("content").and_then(|v| v.as_str()) {
        Some(c) => c.to_string(),
        None => return json!({ "success": false, "error": "Missing `content` parameter." }).to_string(),
    };

    let root = Path::new(&workspace_root);
    let full_path = if Path::new(&path).is_absolute() {
        path.clone().into()
    } else {
        root.join(&path)
    };

    // An existing file is not replaced unless asked: a new-file call that lands on a file the author forgot about would
    // otherwise destroy it silently.
    let overwrite = ["overwrite", "force"].iter().any(|k| match params.get(*k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"),
        _ => false,
    });
    if full_path.is_dir() {
        return json!({ "success": false, "error": format!("NOT CREATED - {} is a folder.", path) }).to_string();
    }
    let existed = full_path.is_file();
    if existed && !overwrite {
        let size = fs::metadata(&full_path).map(|m| m.len()).unwrap_or(0);
        return json!({
            "success": false,
            "error": format!("NOT CREATED - {} already exists ({} bytes). To replace the whole file add --overwrite; to change part of it use mysid patch or mysid edit.", path, size)
        })
        .to_string();
    }

    if let Some(parent) = full_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return json!({
                "success": false,
                "error": format!("Failed to create parent directories: {}", e)
            })
            .to_string();
        }
    }

    match fs::write(&full_path, &content) {
        Ok(_) => json!({
            "success": true,
            "path": path,
            "bytes_written": content.len(),
            "output": format!("{} {} ({} line(s), {} bytes)", if existed { "replaced" } else { "created" }, path, content.lines().count(), content.len())
        })
        .to_string(),
        Err(e) => json!({
            "success": false,
            "error": format!("Failed to write file: {}", e)
        })
        .to_string(),
    }
}

/// Create a directory (and all necessary parents).
pub fn execute_mkdir(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `path` parameter." }).to_string(),
    };

    let root = Path::new(&workspace_root);
    let full_path = if Path::new(&path).is_absolute() {
        path.clone().into()
    } else {
        root.join(&path)
    };

    match fs::create_dir_all(&full_path) {
        Ok(_) => json!({
            "success": true,
            "path": path,
            "message": format!("Directory created: {}", full_path.display())
        })
        .to_string(),
        Err(e) => json!({
            "success": false,
            "error": format!("Failed to create directory {}: {}", full_path.display(), e)
        })
        .to_string(),
    }
}

/// Rename or move a file or folder.
pub fn execute_rename(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let old_path = match params.get("old_path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `old_path` parameter." }).to_string(),
    };
    let new_path = match params.get("new_path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `new_path` parameter." }).to_string(),
    };

    let root = Path::new(&workspace_root);
    let from_path = if Path::new(&old_path).is_absolute() {
        old_path.clone().into()
    } else {
        super::workspace::resolve_or_join(root, &old_path, &workspace_root)
    };

    let to_path = if Path::new(&new_path).is_absolute() {
        new_path.clone().into()
    } else {
        root.join(&new_path)
    };

    if !from_path.exists() {
        return json!({
            "success": false,
            "error": format!("Source path does not exist: {}", from_path.display())
        })
        .to_string();
    }

    if let Some(parent) = to_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return json!({
                "success": false,
                "error": format!("Failed to create destination parent directories: {}", e)
            })
            .to_string();
        }
    }

    match fs::rename(&from_path, &to_path) {
        Ok(_) => json!({
            "success": true,
            "old_path": old_path,
            "new_path": new_path,
            "message": format!("Renamed/moved '{}' to '{}'", from_path.display(), to_path.display())
        })
        .to_string(),
        Err(e) => json!({
            "success": false,
            "error": format!("Failed to rename '{}' to '{}': {}", from_path.display(), to_path.display(), e)
        })
        .to_string(),
    }
}

/// Files and folders under `dir` (stops counting at 100,000).
fn count_items(dir: &Path, so_far: usize) -> usize {
    let mut n = so_far;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            n += 1;
            if n >= 100_000 {
                return n;
            }
            if e.path().is_dir() {
                n = count_items(&e.path(), n);
            }
        }
    }
    n
}

/// Delete a file or directory (supports optional recursive deletion).
pub fn execute_delete(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `path` parameter." }).to_string(),
    };
    let recursive = params
        .get("recursive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let root = Path::new(&workspace_root);
    let target_path = if Path::new(&path).is_absolute() {
        path.clone().into()
    } else {
        root.join(&path)
    };

    if !target_path.exists() {
        return json!({
            "success": false,
            "error": format!("Path does not exist: {}", target_path.display())
        })
        .to_string();
    }

    if target_path.is_dir() && !recursive {
        let inside = count_items(&target_path, 0);
        if inside > 0 {
            return json!({
                "success": false,
                "error": format!(
                    "NOT DELETED - {} is not empty ({}{} item(s) inside). Use --recursive to delete it and everything in it.",
                    path, inside, if inside >= 100_000 { "+" } else { "" }
                )
            })
            .to_string();
        }
    }

    let res = if target_path.is_dir() {
        if recursive {
            fs::remove_dir_all(&target_path)
        } else {
            fs::remove_dir(&target_path)
        }
    } else {
        fs::remove_file(&target_path)
    };

    match res {
        Ok(_) => json!({
            "success": true,
            "path": path,
            "message": format!("Deleted '{}'", target_path.display())
        })
        .to_string(),
        Err(e) => json!({
            "success": false,
            "error": format!("Failed to delete '{}': {}", target_path.display(), e)
        })
        .to_string(),
    }
}

/// List contents of a directory with optional depth limit and gitignore filtering.
pub fn execute_list_dir(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let sub_path: Option<String> = params
        .get("path")
        .or_else(|| params.get("dir"))
        .or_else(|| {
            params.get("args")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(0))
        })
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let full: bool = params
        .get("full")
        .or_else(|| params.get("all"))
        .or_else(|| params.get("recursive"))
        .and_then(|v| v.as_bool())
        .or_else(|| {
            params.get("args").and_then(|v| v.as_array()).map(|arr| {
                arr.iter().any(|item| {
                    item.as_str()
                        .map(|s| s == "--recursive" || s == "-r" || s == "--full")
                        .unwrap_or(false)
                })
            })
        })
        .unwrap_or(false);

    let max_depth: Option<usize> = if full {
        None
    } else {
        params
            .get("max_depth")
            .or_else(|| params.get("depth"))
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .or(Some(1)) // Default to L1 (depth 1) to prevent context bloat!
    };

    let root = Path::new(&workspace_root);
    let target_dir = match &sub_path {
        Some(sub) if !sub.is_empty() && sub != "." => root.join(sub),
        _ => root.to_path_buf(),
    };

    if !target_dir.exists() {
        return json!({
            "success": false,
            "error": format!("Directory not found: {}", target_dir.display())
        })
        .to_string();
    }

    let mut builder = WalkBuilder::new(&target_dir);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true);

    if let Some(depth) = max_depth {
        builder.max_depth(Some(depth));
    }

    let mut entries = Vec::new();
    for result in builder.build() {
        let entry = match result {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if path == target_dir {
            continue;
        }

        let rel_path = match path.strip_prefix(root) {
            Ok(r) => r.to_string_lossy().replace('\\', "/"),
            Err(_) => path.to_string_lossy().replace('\\', "/"),
        };

        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);

        entries.push(json!({
            "path": rel_path,
            "is_directory": is_dir
        }));
    }

    // The plain call (no --depth, no --full) answers with the readable view: folders ranked by code, code files with line counts.
    let plain = !full && params.get("max_depth").is_none() && params.get("depth").is_none() && target_dir.is_dir();
    let mut result = json!({
        "success": true,
        "directory": sub_path.unwrap_or_else(|| ".".to_string()),
        "entries": entries,
        "count": entries.len()
    });
    if plain {
        result["output"] = json!(super::orient::list_view(root, &target_dir));
    }
    result.to_string()
}

/// Universal dispatcher for `file <action> ...` subcommands:
/// `new`, `create`, `write`, `delete`, `remove`, `rm`, `rename`, `mv`, `mkdir`, `list`, `ls`
/// Files to copy as (source file, destination file); `.git`, `node_modules` and `.mysid` are not entered.
fn collect_copy(src: &Path, dst: &Path, out: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>, skipped: &mut Vec<String>) {
    if src.is_dir() {
        let Ok(rd) = fs::read_dir(src) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".git" | "node_modules" | ".mysid") && e.path().is_dir() {
                skipped.push(name);
                continue;
            }
            collect_copy(&e.path(), &dst.join(&name), out, skipped);
        }
    } else {
        out.push((src.to_path_buf(), dst.to_path_buf()));
    }
}

/// `mysid file copy <src>... <dest>`: files or folders, many in one call, into the workspace. Nothing is overwritten
/// unless `--overwrite`, and nothing is copied when any source is missing or any target exists.
pub fn execute_copy(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let reply = |ok: bool, t: String| json!({ "success": ok, "output": t }).to_string();
    let sources: Vec<String> = params.get("sources").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let dest = params.get("dest").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if sources.is_empty() || dest.is_empty() {
        return reply(false, "NOT COPIED: usage: mysid file copy <source>... <destination>  (several sources need a destination folder)".to_string());
    }
    let overwrite = ["overwrite", "force"].iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false));
    let root = Path::new(&workspace_root);

    let dest_path = if Path::new(&dest).is_absolute() { std::path::PathBuf::from(&dest) } else { root.join(&dest) };
    if !super::exec::inside(root, &dest_path) {
        return reply(false, format!("NOT COPIED: the destination {} is outside the workspace ({}). Sources may be anywhere; the destination must be inside it.", dest, workspace_root));
    }
    let mut from: Vec<std::path::PathBuf> = Vec::new();
    for s in &sources {
        let p = if Path::new(s).is_absolute() { std::path::PathBuf::from(s) } else { super::workspace::resolve_or_join(root, s, &workspace_root) };
        if !p.exists() {
            return reply(false, format!("NOT COPIED: the source {} does not exist ({})", s, p.display()));
        }
        from.push(p);
    }
    // `copy src/ dest/` (trailing slash), several sources, or a file into an existing folder: inside the destination.
    // One folder to a destination without a slash: its contents ARE the destination (never src/src on a second run).
    let into_dir = from.len() > 1 || dest.ends_with('/') || dest.ends_with('\\') || (dest_path.is_dir() && !from[0].is_dir());
    let mut pairs = Vec::new();
    let mut skipped = Vec::new();
    for p in &from {
        let target = if into_dir { dest_path.join(p.file_name().unwrap_or_default()) } else { dest_path.clone() };
        collect_copy(p, &target, &mut pairs, &mut skipped);
    }
    if pairs.is_empty() {
        return reply(false, "NOT COPIED: nothing to copy (the sources are empty folders)".to_string());
    }
    let existing: Vec<String> = pairs.iter().filter(|(_, d)| d.exists()).map(|(_, d)| d.strip_prefix(root).unwrap_or(d).to_string_lossy().replace('\\', "/")).collect();
    if !existing.is_empty() && !overwrite {
        let shown: Vec<&str> = existing.iter().take(8).map(String::as_str).collect();
        return reply(false, format!("NOT COPIED: {} target file(s) already exist: {}{}. Nothing was copied; add --overwrite to replace them.", existing.len(), shown.join(", "), if existing.len() > 8 { ", ..." } else { "" }));
    }
    let mut bytes = 0u64;
    for (s, d) in &pairs {
        if let Some(parent) = d.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return reply(false, format!("COPY STOPPED: cannot create {}: {}", parent.display(), e));
            }
        }
        match fs::copy(s, d) {
            Ok(n) => bytes += n,
            Err(e) => return reply(false, format!("COPY STOPPED after an error on {}: {}", s.display(), e)),
        }
    }
    let mut out = format!("copied {} file(s), {} KB to {}", pairs.len(), bytes / 1024, dest);
    if !skipped.is_empty() {
        skipped.sort();
        skipped.dedup();
        out.push_str(&format!("\nnot copied: {} (never copied)", skipped.join(", ")));
    }
    reply(true, out)
}

pub fn execute_file(params: &Value, default_root: Option<&str>) -> String {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("args")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_ascii_lowercase();

    match action.as_str() {
        "" | "help" | "-h" | "--help" => {
            "mysid file: Filesystem management utilities\n\n\
Usage:\n  \
  mysid file new <path> <content>         Create or overwrite file (auto-creates parent dirs)\n  \
  mysid file new <path> --stdin | -       ... with the content from stdin (heredoc / pipe), no shell quoting\n  \
  mysid file new <path> --from <file>     ... copied from a file (piped input also works)\n  \
                                          (mysid write <path> is the same but refuses to replace an existing file)\n  \
  mysid file mkdir <path>                 Recursively create directory\n  \
  mysid file rename <old_path> <new_path> Rename or move file/directory\n  \
  mysid file copy <src>... <dest>         Copy files or folders (many at once; --overwrite to replace)\n  \
  mysid file delete <path> [--recursive]  Delete file or directory\n  \
  mysid file list [path] [-L<depth>]      List files (defaults to -L1, respects .gitignore)\n".to_string()
        }
        "new" | "create" | "write" => execute_create_file(params, default_root),
        "mkdir" | "new_folder" | "create_folder" => execute_mkdir(params, default_root),
        "rename" | "mv" | "move" => execute_rename(params, default_root),
        "copy" | "cp" => execute_copy(params, default_root),
        "delete" | "remove" | "rm" => execute_delete(params, default_root),
        "list" | "ls" | "list_dir" | "dir" => execute_list_dir(params, default_root),
        _ => json!({
            "success": false,
            "error": format!("Unknown file action '{}'. Run 'mysid file --help' for usage.", action)
        }).to_string(),
    }
}

