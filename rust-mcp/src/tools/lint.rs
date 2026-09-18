use regex::Regex;
use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use super::workspace::resolve_existing_root;

pub fn execute_lint(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let target_path_opt: Option<String> = params
        .get("path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let root = Path::new(&workspace_root);

    let target = target_path_opt.as_deref().unwrap_or(".");
    let target_path = root.join(target);

    let is_python = target.ends_with(".py") || (!target_path.is_file() && root.join("requirements.txt").exists());
    let is_rust = target.ends_with(".rs") || (!target_path.is_file() && root.join("Cargo.toml").exists());

    let mut diagnostics: Vec<serde_json::Value> = Vec::new();

    if is_python {
        let venv_py_win = root.join("venv").join("Scripts").join("python.exe");
        let venv_py_nix = root.join("venv").join("bin").join("python");
        let py_bin = if venv_py_win.exists() {
            venv_py_win.to_string_lossy().to_string()
        } else if venv_py_nix.exists() {
            venv_py_nix.to_string_lossy().to_string()
        } else {
            "python".to_string()
        };

        let mut cmd = Command::new(&py_bin);
        if target.ends_with(".py") {
            cmd.args(["-m", "py_compile", target]);
        } else {
            cmd.args(["-m", "compileall", "-q", "-f", target]);
        }
        cmd.current_dir(root);

        if let Ok(output) = cmd.output() {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let stdout_str = String::from_utf8_lossy(&output.stdout);
            let combined = format!("{}\n{}", stdout_str, stderr_str);

            let re_file_line = Regex::new(r#"(?s)File "([^"]+)", line (\d+).*?(SyntaxError|IndentationError):\s*([^\r\n]+)"#).unwrap();
            for cap in re_file_line.captures_iter(&combined) {
                let file = cap.get(1).map_or("", |m| m.as_str()).to_string();
                let line: usize = cap.get(2).map_or("0", |m| m.as_str()).parse().unwrap_or(0);
                let msg = format!("{}: {}", cap.get(3).map_or("", |m| m.as_str()), cap.get(4).map_or("", |m| m.as_str()).trim());
                diagnostics.push(json!({ "file": file, "line": line, "message": msg }));
            }

            let re_sorry = Regex::new(r#"Sorry:\s+(SyntaxError|IndentationError):\s+(.+?)\s+\((.+?),\s+line\s+(\d+)\)"#).unwrap();
            for cap in re_sorry.captures_iter(&combined) {
                let msg = format!("{}: {}", cap.get(1).map_or("", |m| m.as_str()), cap.get(2).map_or("", |m| m.as_str()));
                let file = cap.get(3).map_or("", |m| m.as_str()).to_string();
                let line: usize = cap.get(4).map_or("0", |m| m.as_str()).parse().unwrap_or(0);
                diagnostics.push(json!({ "file": file, "line": line, "message": msg }));
            }
        }
    } else if is_rust {
        let mut cmd = Command::new("cargo");
        cmd.args(["check", "--message-format=short"]);
        cmd.current_dir(root);

        if let Ok(output) = cmd.output() {
            let stderr_str = String::from_utf8_lossy(&output.stderr);
            let re_rust = Regex::new(r#"([^:\n]+):(\d+):(\d+):\s+error(?:\[\w+\])?:\s+(.+)"#).unwrap();
            for cap in re_rust.captures_iter(&stderr_str) {
                let file = cap.get(1).map_or("", |m| m.as_str()).to_string();
                let line: usize = cap.get(2).map_or("0", |m| m.as_str()).parse().unwrap_or(0);
                let message = cap.get(4).map_or("", |m| m.as_str()).to_string();
                diagnostics.push(json!({ "file": file, "line": line, "message": message }));
            }
        }
    }

    let success = diagnostics.is_empty();
    let summary = if success {
        "No compiler or syntax errors detected.".to_string()
    } else {
        format!("{} error(s) detected", diagnostics.len())
    };

    json!({
        "success": success,
        "diagnostics": diagnostics,
        "summary": summary
    })
    .to_string()
}
