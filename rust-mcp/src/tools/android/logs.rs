use serde_json::json;
use std::path::Path;

use super::adb::run_adb_cmd;
use super::AndroidParams;

pub fn handle_logcat_or_crashes(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    let lines_count = p.lines.unwrap_or(200).to_string();
    let is_crashes = p.action == "crashes";
    let args: Vec<&str> = if is_crashes {
        vec!["logcat", "-d", "-b", "crash", "-t", &lines_count]
    } else {
        vec!["logcat", "-d", "-t", &lines_count]
    };

    match run_adb_cmd(&args, workspace_path) {
        Ok((code, mut stdout, stderr)) => {
            if is_crashes && stdout.trim().lines().count() <= 1 {
                // Fallback to error buffer
                if let Ok((c2, s2, e2)) = run_adb_cmd(&["logcat", "-d", "*:E", "-t", &lines_count], workspace_path) {
                    return json!({
                        "success": c2 == 0,
                        "action": "crashes",
                        "stdout": s2.trim(),
                        "stderr": e2.trim(),
                    }).to_string();
                }
            }

            if let Some(pkg) = &p.package_name {
                let filtered: Vec<&str> = stdout.lines().filter(|l| l.contains(pkg)).collect();
                stdout = filtered.join("\n");
            }

            json!({
                "success": code == 0,
                "action": p.action,
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
            }).to_string()
        }
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}

pub fn handle_clear_logcat(workspace_path: Option<&Path>) -> String {
    match run_adb_cmd(&["logcat", "-c"], workspace_path) {
        Ok((code, stdout, stderr)) => json!({
            "success": code == 0,
            "action": "clear_logcat",
            "stdout": stdout.trim(),
            "stderr": stderr.trim(),
        }).to_string(),
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}
