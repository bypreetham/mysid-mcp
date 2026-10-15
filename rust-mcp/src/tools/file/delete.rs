//! `mysid file rm <path>` (also delete, remove): delete a file, or a folder with --recursive.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tools::workspace::resolve_root;

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
