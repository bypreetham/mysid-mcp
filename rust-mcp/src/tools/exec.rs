use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use super::workspace::resolve_existing_root;

const MAX_OUTPUT_BYTES: usize = 32_768;

pub fn execute_exec(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let command = match params.get("command").and_then(|v| v.as_str()) {
        Some(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => return json!({ "success": false, "error": "Missing or empty `command` parameter." }).to_string(),
    };
    let _timeout_secs: u64 = params
        .get("timeout_secs")
        .and_then(|v| v.as_u64())
        .unwrap_or(30);

    let root = Path::new(&workspace_root);

    #[cfg(target_os = "windows")]
    let mut child_cmd = {
        let mut c = Command::new("cmd.exe");
        c.args(["/C", &command]);
        c
    };

    #[cfg(not(target_os = "windows"))]
    let mut child_cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", &command]);
        c
    };

    child_cmd.current_dir(root);

    let output = match child_cmd.output() {
        Ok(out) => out,
        Err(e) => {
            return json!({
                "success": false,
                "error": format!("Failed to execute process: {}", e)
            })
            .to_string();
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let mut stdout_str = String::from_utf8_lossy(&output.stdout).to_string();
    let mut stderr_str = String::from_utf8_lossy(&output.stderr).to_string();

    if stdout_str.len() > MAX_OUTPUT_BYTES {
        let start = stdout_str.len() - MAX_OUTPUT_BYTES;
        stdout_str = format!("[... truncated earlier output ...]\n{}", &stdout_str[start..]);
    }
    if stderr_str.len() > MAX_OUTPUT_BYTES {
        let start = stderr_str.len() - MAX_OUTPUT_BYTES;
        stderr_str = format!("[... truncated earlier output ...]\n{}", &stderr_str[start..]);
    }

    let success = exit_code == 0;
    json!({
        "success": success,
        "exit_code": exit_code,
        "stdout": stdout_str.trim(),
        "stderr": stderr_str.trim()
    })
    .to_string()
}
