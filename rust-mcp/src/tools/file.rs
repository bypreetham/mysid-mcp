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
            "bytes_written": content.len()
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
        root.join(&old_path)
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
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let full: bool = params
        .get("full")
        .or_else(|| params.get("all"))
        .and_then(|v| v.as_bool())
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

    json!({
        "success": true,
        "directory": sub_path.unwrap_or_else(|| ".".to_string()),
        "entries": entries,
        "count": entries.len()
    })
    .to_string()
}

/// Universal dispatcher for `file <action> ...` subcommands:
/// `new`, `create`, `write`, `delete`, `remove`, `rm`, `rename`, `mv`, `mkdir`, `list`, `ls`
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
  mysid file mkdir <path>                 Recursively create directory\n  \
  mysid file rename <old_path> <new_path> Rename or move file/directory\n  \
  mysid file delete <path> [--recursive]  Delete file or directory\n  \
  mysid file list [path] [-L<depth>]      List files (defaults to -L1, respects .gitignore)\n".to_string()
        }
        "new" | "create" | "write" => execute_create_file(params, default_root),
        "mkdir" | "new_folder" | "create_folder" => execute_mkdir(params, default_root),
        "rename" | "mv" | "move" => execute_rename(params, default_root),
        "delete" | "remove" | "rm" => execute_delete(params, default_root),
        "list" | "ls" | "list_dir" | "dir" => execute_list_dir(params, default_root),
        _ => json!({
            "success": false,
            "error": format!("Unknown file action '{}'. Run 'mysid file --help' for usage.", action)
        }).to_string(),
    }
}

