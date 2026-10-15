//! `mysid file mkdir <path>`: a folder, with its parents.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tools::workspace::resolve_root;

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
