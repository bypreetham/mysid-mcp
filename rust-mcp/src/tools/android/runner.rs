use serde_json::json;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::adb::run_adb_cmd;
use super::AndroidParams;

/// Discover custom project runner script, checking:
/// 1. <workspace>/tools/run.bat
/// 2. <workspace>/run.bat
pub fn detect_custom_run_bat(ws: &Path) -> Option<PathBuf> {
    let tools_run_bat = ws.join("tools").join("run.bat");
    let root_run_bat = ws.join("run.bat");

    if tools_run_bat.exists() {
        Some(tools_run_bat)
    } else if root_run_bat.exists() {
        Some(root_run_bat)
    } else {
        None
    }
}

/// Execute run.bat with specified arguments
pub fn run_custom_bat(bat: &Path, args: &[&str], cwd: &Path) -> Result<(i32, String, String), String> {
    let mut cmd = Command::new("cmd.exe");
    let bat_str = bat.to_string_lossy();
    let mut cmd_args = vec!["/C", &bat_str];
    cmd_args.extend_from_slice(args);
    cmd.args(&cmd_args);
    cmd.current_dir(cwd);

    match cmd.output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1);
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            Ok((code, stdout, stderr))
        }
        Err(e) => Err(format!("Failed to execute script '{}': {}", bat.display(), e)),
    }
}

pub fn handle_install(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    let ws = match workspace_path {
        Some(w) => w,
        None => {
            return json!({
                "success": false,
                "error": "workspace_root is required for install action"
            }).to_string();
        }
    };

    // 1. Check custom run.bat (e.g. tools/run.bat or run.bat)
    if let Some(bat) = detect_custom_run_bat(ws) {
        return match run_custom_bat(&bat, &["install"], ws) {
            Ok((code, stdout, stderr)) => json!({
                "success": code == 0,
                "method": format!("{} install", bat.display()),
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim()
            }).to_string(),
            Err(e) => json!({
                "success": false,
                "error": e
            }).to_string(),
        };
    }

    // 2. Default APK location: app/build/outputs/apk/debug/app-debug.apk
    let apk_rel = ws.join("app").join("build").join("outputs").join("apk").join("debug").join("app-debug.apk");
    if !apk_rel.exists() {
        return json!({
            "success": false,
            "error": format!("APK not found at '{}'. Run 'build' first.", apk_rel.display())
        }).to_string();
    }

    let apk_str = apk_rel.to_string_lossy().to_string();
    let res = run_adb_cmd(&["install", "-r", "-t", "-d", "-g", &apk_str], Some(ws));
    match res {
        Ok((code, stdout, stderr)) => {
            let combined = format!("{}\n{}", stdout, stderr);
            if combined.contains("INSTALL_FAILED_UPDATE_INCOMPATIBLE") || combined.to_lowercase().contains("signatures do not match") {
                // Uninstall and retry
                if let Some(pkg) = &p.package_name {
                    let _ = run_adb_cmd(&["uninstall", pkg], Some(ws));
                    let retry_res = run_adb_cmd(&["install", "-r", "-t", "-g", &apk_str], Some(ws));
                    if let Ok((rc2, out2, err2)) = retry_res {
                        return json!({
                            "success": rc2 == 0,
                            "retried_after_uninstall": true,
                            "exit_code": rc2,
                            "stdout": out2.trim(),
                            "stderr": err2.trim(),
                        }).to_string();
                    }
                }
            }

            json!({
                "success": code == 0,
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim(),
            }).to_string()
        }
        Err(e) => json!({ "success": false, "error": e }).to_string(),
    }
}

pub fn handle_run_process(workspace_path: Option<&Path>) -> String {
    let ws = match workspace_path {
        Some(w) => w,
        None => {
            return json!({
                "success": false,
                "error": "workspace_root is required for run action"
            }).to_string();
        }
    };

    if let Some(bat) = detect_custom_run_bat(ws) {
        match run_custom_bat(&bat, &[], ws) {
            Ok((code, stdout, stderr)) => json!({
                "success": code == 0,
                "action": "run",
                "method": bat.display().to_string(),
                "exit_code": code,
                "stdout": stdout.trim(),
                "stderr": stderr.trim()
            }).to_string(),
            Err(e) => json!({
                "success": false,
                "error": e
            }).to_string(),
        }
    } else {
        json!({
            "success": false,
            "error": "No tools/run.bat or run.bat found in workspace."
        }).to_string()
    }
}

pub fn handle_launch_app(p: &AndroidParams, workspace_path: Option<&Path>) -> String {
    match &p.package_name {
        Some(pkg) => {
            let mut args = vec!["shell"];
            let target_comp;
            if let Some(act) = &p.activity_name {
                target_comp = format!("{}/{}", pkg, act);
                args.extend(&["am", "start", "-n", &target_comp]);
            } else {
                args.extend(&["monkey", "-p", pkg, "-c", "android.intent.category.LAUNCHER", "1"]);
            }

            match run_adb_cmd(&args, workspace_path) {
                Ok((code, stdout, stderr)) => json!({
                    "success": code == 0,
                    "action": "launch_app",
                    "package": pkg,
                    "stdout": stdout.trim(),
                    "stderr": stderr.trim(),
                }).to_string(),
                Err(e) => json!({ "success": false, "error": e }).to_string(),
            }
        }
        None => json!({ "success": false, "error": "package_name is required for launch_app" }).to_string(),
    }
}
