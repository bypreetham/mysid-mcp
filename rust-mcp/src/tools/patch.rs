use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::{resolve_root, resolve_file_in_workspace};

fn get_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

pub fn execute_patch(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Accept "path", "file", or "filepath" — "file" is the most common mistake
    let path = match ["path", "file", "filepath"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()))
    {
        Some(p) => p.to_string(),
        None => return json!({
            "success": false,
            "error": "Missing file path. Use 'path', 'file', or 'filepath' key."
        }).to_string(),
    };
    let old_string = match get_param(params, &["old_string", "search", "find", "target", "from"]) {
        Some(s) => s,
        None => return json!({ "success": false, "error": "Missing `old_string` (or search/find/target/from) parameter." }).to_string(),
    };
    let new_string = match get_param(params, &["new_string", "replace", "replacement", "with", "to", "content"]) {
        Some(s) => s,
        None => return json!({ "success": false, "error": "Missing `new_string` (or replace/replacement/with/to/content) parameter." }).to_string(),
    };
    let replace_all = params
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

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

    // 1. Direct exact match
    if content.contains(&old_string) {
        let new_content = if replace_all {
            content.replace(&old_string, &new_string)
        } else {
            content.replacen(&old_string, &new_string, 1)
        };

        return match fs::write(&full_path, &new_content) {
            Ok(_) => json!({
                "success": true,
                "path": path,
                "strategy": "exact_match"
            })
            .to_string(),
            Err(e) => json!({
                "success": false,
                "error": format!("Failed to write modified file: {}", e)
            })
            .to_string(),
        };
    }

    // 2. Fallback: Normalized line endings / trimmed match
    let norm_content = content.replace("\r\n", "\n");
    let norm_old = old_string.replace("\r\n", "\n");
    let norm_new = new_string.replace("\r\n", "\n");

    if norm_content.contains(&norm_old) {
        let new_content = if replace_all {
            norm_content.replace(&norm_old, &norm_new)
        } else {
            norm_content.replacen(&norm_old, &norm_new, 1)
        };

        // Restore CRLF if the original file had CRLF
        let final_content = if content.contains("\r\n") {
            new_content.replace("\n", "\r\n")
        } else {
            new_content
        };

        return match fs::write(&full_path, &final_content) {
            Ok(_) => json!({
                "success": true,
                "path": path,
                "strategy": "crlf_normalized"
            })
            .to_string(),
            Err(e) => json!({
                "success": false,
                "error": format!("Failed to write modified file: {}", e)
            })
            .to_string(),
        };
    }

    // 3. Fallback: Trimmed lines match
    let trimmed_old = old_string.trim();
    if !trimmed_old.is_empty() && content.contains(trimmed_old) {
        let new_content = content.replacen(trimmed_old, new_string.trim(), 1);
        return match fs::write(&full_path, &new_content) {
            Ok(_) => json!({
                "success": true,
                "path": path,
                "strategy": "trimmed_match"
            })
            .to_string(),
            Err(e) => json!({
                "success": false,
                "error": format!("Failed to write modified file: {}", e)
            })
            .to_string(),
        };
    }

    json!({
        "success": false,
        "error": "old_string block could not be found in target file"
    })
    .to_string()
}

/// Apply an ordered list of patches to one or more files in a single call.
/// Params: { "patches": [ { "path": "...", "old_string": "...", "new_string": "...", "replace_all"?: bool }, ... ] }
/// Stops at the first failed patch and returns which index failed.
pub fn execute_patch_batch(params: &Value, default_root: Option<&str>) -> String {
    let patches = match params.get("patches").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return json!({ "success": false, "error": "Missing `patches` array." }).to_string(),
    };
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let mut results = Vec::new();
    for (i, patch) in patches.iter().enumerate() {
        let mut merged = patch.clone();
        if let Some(obj) = merged.as_object_mut() {
            obj.entry("workspace_root")
                .or_insert_with(|| serde_json::Value::String(workspace_root.clone()));
        }
        let result_str = execute_patch(&merged, Some(&workspace_root));
        let result_val: Value = serde_json::from_str(&result_str)
            .unwrap_or_else(|_| json!({ "success": false, "error": result_str }));
        let success = result_val.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        results.push(json!({
            "index": i,
            "path": patch.get("path").and_then(|v| v.as_str()).unwrap_or("?"),
            "success": success,
            "strategy": result_val.get("strategy"),
            "error": result_val.get("error"),
        }));
        if !success {
            return json!({
                "success": false,
                "applied": i,
                "failed_at": i,
                "results": results,
            }).to_string();
        }
    }
    json!({ "success": true, "applied": results.len(), "results": results }).to_string()
}
