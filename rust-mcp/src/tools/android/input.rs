use serde_json::json;
use std::path::Path;

use super::adb::run_adb_cmd;
use super::AndroidParams;

pub fn handle_tap(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    match (p.x, p.y) {
        (Some(x), Some(y)) => {
            let xs = x.to_string();
            let ys = y.to_string();
            match run_adb_cmd(&["shell", "input", "tap", &xs, &ys], workspace_path) {
                Ok((code, stdout, stderr)) => json!({
                    "success": code == 0,
                    "action": "tap",
                    "x": x,
                    "y": y,
                    "stdout": stdout.trim(),
                    "stderr": stderr.trim(),
                }).to_string(),
                Err(e) => json!({ "success": false, "error": e }).to_string(),
            }
        }
        _ => json!({ "success": false, "error": "x and y coordinates are required for tap" }).to_string(),
    }
}

pub fn handle_type(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    match &p.text {
        Some(text) => {
            let formatted = text.replace(' ', "%s");
            match run_adb_cmd(&["shell", "input", "text", &formatted], workspace_path) {
                Ok((code, stdout, stderr)) => json!({
                    "success": code == 0,
                    "action": "type",
                    "stdout": stdout.trim(),
                    "stderr": stderr.trim(),
                }).to_string(),
                Err(e) => json!({ "success": false, "error": e }).to_string(),
            }
        }
        None => json!({ "success": false, "error": "text is required for type action" }).to_string(),
    }
}

pub fn handle_keyevent(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    let key = p.keycode.clone().unwrap_or_else(|| "KEYCODE_BACK".to_string());
    match run_adb_cmd(&["shell", "input", "keyevent", &key], workspace_path) {
        Ok((code, stdout, stderr)) => json!({
            "success": code == 0,
            "action": "keyevent",
            "keycode": key,
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }).to_string(),
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}
