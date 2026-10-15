//! `mysid file new <path>`: a new file with the given content (an existing file is kept unless --overwrite).

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tools::workspace::resolve_root;

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
