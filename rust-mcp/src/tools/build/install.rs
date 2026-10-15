//! `mysid install`: deploy the built Android app, install the built Rust binary, or run the package manager.

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use crate::tools::workspace::resolve_existing_root;

const MAX_OUTPUT_BYTES: usize = 32_768;

/// Run `npm install` (or yarn/pnpm) and return stdout + stderr + extracted warnings.
/// Always returns output even on success so the LLM can inspect peer-dep warnings.
pub fn execute_install(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let root = Path::new(&workspace_root);

    if root.join("gradlew.bat").exists() || root.join("gradlew").exists() || root.join("app").exists() {
        let mut p = params.clone();
        if let Some(obj) = p.as_object_mut() {
            obj.insert("action".to_string(), json!("install"));
        }
        return crate::tools::android::execute_android(&p, default_root);
    }

    if root.join("Cargo.toml").exists() && !root.join("package.json").exists() {
        return super::install_rust::execute(root, params);
    }

    if !root.join("package.json").exists() {
        return json!({
            "success": false,
            "error": "No package.json found — install only supports Node/npm or Android projects."
        })
        .to_string();
    }

    // Detect package manager by lockfile
    let (prog, install_args): (&str, Vec<&str>) = if root.join("pnpm-lock.yaml").exists() {
        ("pnpm", vec!["install"])
    } else if root.join("yarn.lock").exists() {
        ("yarn", vec![])
    } else {
        ("npm", vec!["install"])
    };

    let stack = prog.to_string();

    #[cfg(target_os = "windows")]
    let raw_output = {
        let mut full_args = vec!["/C", prog];
        full_args.extend_from_slice(&install_args);
        Command::new("cmd.exe")
            .args(&full_args)
            .current_dir(root)
            .output()
    };

    #[cfg(not(target_os = "windows"))]
    let raw_output = Command::new(prog)
        .args(&install_args)
        .current_dir(root)
        .output();

    match raw_output {
        Err(e) => json!({
            "success": false,
            "stack": stack,
            "error": format!("Failed to spawn '{}': {}", prog, e)
        })
        .to_string(),
        Ok(out) => {
            let exit_code = out.status.code().unwrap_or(-1);
            let mut stdout_str = String::from_utf8_lossy(&out.stdout).to_string();
            let mut stderr_str = String::from_utf8_lossy(&out.stderr).to_string();

            if stdout_str.len() > MAX_OUTPUT_BYTES {
                let start = stdout_str.len() - MAX_OUTPUT_BYTES;
                stdout_str = format!("[... truncated ...]\n{}", &stdout_str[start..]);
            }
            if stderr_str.len() > MAX_OUTPUT_BYTES {
                let start = stderr_str.len() - MAX_OUTPUT_BYTES;
                stderr_str = format!("[... truncated ...]\n{}", &stderr_str[start..]);
            }

            // Surface npm warnings / deprecation notices for the LLM
            let warnings: Vec<&str> = stderr_str
                .lines()
                .filter(|l| {
                    let lower = l.to_ascii_lowercase();
                    lower.starts_with("npm warn")
                        || lower.starts_with("warn ")
                        || lower.contains("deprecated")
                        || lower.contains("peer dep")
                })
                .collect();

            json!({
                "success": exit_code == 0,
                "stack": stack,
                "exit_code": exit_code,
                "stdout": stdout_str.trim(),
                "stderr": stderr_str.trim(),
                "warnings": warnings
            })
            .to_string()
        }
    }
}
