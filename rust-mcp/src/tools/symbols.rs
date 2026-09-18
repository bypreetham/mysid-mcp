use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::{resolve_root, resolve_file_in_workspace};
use crate::adapters::detect_adapter;

pub fn execute_symbols(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let path = match params.get("path").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => return json!({ "success": false, "error": "Missing `path` parameter." }).to_string(),
    };

    let root = Path::new(&workspace_root);
    let full_path = match resolve_file_in_workspace(root, &path, &workspace_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let content = match fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) => {
            return json!({
                "success": false,
                "error": format!("Failed to read file: {}", e)
            })
            .to_string();
        }
    };

    let ext = full_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    // Delegate symbol extraction to the active StackAdapter (SOLID: Dependency Inversion & Single Responsibility)
    let adapter = detect_adapter(root);
    let symbols = adapter.extract_symbols(&content, &ext);

    json!({
        "success": true,
        "adapter": adapter.name(),
        "path": path,
        "symbols": symbols,
        "count": symbols.len()
    })
    .to_string()
}
