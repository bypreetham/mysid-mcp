//! `mysid file ls [path]` / `mysid list`: what is in a folder.

use ignore::WalkBuilder;
use serde_json::{json, Value};
use std::path::Path;

use crate::tools::workspace::resolve_root;

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
        result["output"] = json!(crate::tools::scanner::orient::list_view(root, &target_dir));
    }
    result.to_string()
}
