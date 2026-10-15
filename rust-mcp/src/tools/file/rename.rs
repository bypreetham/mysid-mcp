//! `mysid file mv <old> <new>` (also rename, move): rename or move a file or folder.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tools::workspace::resolve_root;

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
        crate::tools::workspace::resolve_or_join(root, &old_path, &workspace_root)
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
