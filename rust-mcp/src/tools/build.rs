use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use super::workspace::resolve_existing_root;

const MAX_OUTPUT_BYTES: usize = 32_768;

pub fn execute_build(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let extra_args: Option<String> = params
        .get("extra_args")
        .and_then(|v| v.as_str())
        .or_else(|| params.get("path").and_then(|v| v.as_str()))
        .or_else(|| {
            params
                .get("args")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
        })
        .map(|s| s.to_string());

    if let Some(ref extra) = extra_args {
        let trimmed = extra.trim();
        if trimmed.eq_ignore_ascii_case("release") || trimmed.eq_ignore_ascii_case("aab") {
            return execute_release(params, default_root);
        }
    }

    let root = Path::new(&workspace_root);
    if !root.exists() {
        return json!({
            "success": false,
            "error": format!("Workspace root does not exist: {}", root.display())
        })
        .to_string();
    }

    // If no recognised project file at workspace root, probe one level of subdirectories.
    // Handles monorepo layout: workspace = E:\MyApp, package.json = E:\MyApp\frontend\package.json
    let probed;
    let root = if !root.join("package.json").exists()
        && !root.join("Cargo.toml").exists()
        && !root.join("gradlew").exists()
        && !root.join("gradlew.bat").exists()
        && !root.join("run.bat").exists()
        && !root.join("tools").join("run.bat").exists()
    {
        let sub = std::fs::read_dir(root).ok().and_then(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().is_dir())
                .find(|e| {
                    let p = e.path();
                    p.join("package.json").exists()
                        || p.join("Cargo.toml").exists()
                        || p.join("gradlew").exists()
                        || p.join("gradlew.bat").exists()
                })
                .map(|e| e.path())
        });
        probed = sub.unwrap_or_else(|| root.to_path_buf());
        probed.as_path()
    } else {
        root
    };

    // 0. Detect custom project script (tools/run.bat or run.bat)
    let tools_run_bat = root.join("tools").join("run.bat");
    let root_run_bat = root.join("run.bat");
    let custom_run_bat = if tools_run_bat.exists() {
        Some(tools_run_bat)
    } else if root_run_bat.exists() {
        Some(root_run_bat)
    } else {
        None
    };

    let (stack_name, program, args): (String, String, Vec<String>) = if let Some(bat) = custom_run_bat {
        let bat_str = bat.to_string_lossy().to_string();
        let mut a = vec!["/C".to_string(), bat_str, "build".to_string()];
        if let Some(extra) = &extra_args {
            if !extra.trim().is_empty() {
                a.push(extra.trim().to_string());
            }
        }
        ("Project run.bat (build)".to_string(), "cmd.exe".to_string(), a)
    } else if root.join("gradlew.bat").exists() || root.join("gradlew").exists() {
        // 1. Android (gradlew)
        #[cfg(target_os = "windows")]
        let gradlew = if root.join("gradlew.bat").exists() {
            root.join("gradlew.bat").to_string_lossy().to_string()
        } else {
            "gradlew.bat".to_string()
        };
        #[cfg(not(target_os = "windows"))]
        let gradlew = if root.join("gradlew").exists() {
            "./gradlew".to_string()
        } else {
            "gradlew".to_string()
        };

        let mut a = vec![];
        if let Some(extra) = &extra_args {
            if extra.trim().eq_ignore_ascii_case("clean") {
                a.push("clean".to_string());
            }
        }
        a.push("assembleDebug".to_string());
        a.push("--console=plain".to_string());
        if let Some(extra) = &extra_args {
            if !extra.trim().eq_ignore_ascii_case("clean") && !extra.trim().is_empty() {
                a.push(extra.trim().to_string());
            }
        }
        ("Android (Gradle)".to_string(), gradlew, a)
    } else if root.join("package.json").exists() {
        // 2. React / Node — first type-check, then bundle
        let tsc_result = {
            #[cfg(target_os = "windows")]
            {
                Command::new("cmd.exe")
                    .args(["/C", "npx", "tsc", "--noEmit"])
                    .current_dir(root)
                    .output()
            }
            #[cfg(not(target_os = "windows"))]
            {
                Command::new("npx")
                    .args(["tsc", "--noEmit"])
                    .current_dir(root)
                    .output()
            }
        };

        if let Ok(tsc_out) = tsc_result {
            if !tsc_out.status.success() {
                let mut tsc_err = String::from_utf8_lossy(&tsc_out.stderr).to_string()
                    + &String::from_utf8_lossy(&tsc_out.stdout);
                if tsc_err.len() > MAX_OUTPUT_BYTES {
                    let start = tsc_err.len() - MAX_OUTPUT_BYTES;
                    tsc_err = format!("[... truncated ...]\n{}", &tsc_err[start..]);
                }
                return json!({
                    "success": false,
                    "stack": "React/Node (tsc type-check)",
                    "exit_code": tsc_out.status.code().unwrap_or(-1),
                    "stdout": "",
                    "stderr": tsc_err.trim()
                })
                .to_string();
            }
        }

        #[cfg(target_os = "windows")]
        let (prog, a) = {
            let mut cmd_args = vec!["/C".to_string(), "npm".to_string(), "run".to_string(), "build".to_string()];
            if let Some(extra) = &extra_args {
                if !extra.trim().is_empty() {
                    cmd_args.push(extra.trim().to_string());
                }
            }
            ("cmd.exe".to_string(), cmd_args)
        };
        #[cfg(not(target_os = "windows"))]
        let (prog, a) = {
            let mut cmd_args = vec!["run".to_string(), "build".to_string()];
            if let Some(extra) = &extra_args {
                if !extra.trim().is_empty() {
                    cmd_args.push(extra.trim().to_string());
                }
            }
            ("npm".to_string(), cmd_args)
        };
        ("React/Node (npm run build)".to_string(), prog, a)
    } else if root.join("Cargo.toml").exists() {
        // 3. Rust (cargo)
        let cargo_bin = if let Ok(user_dir) = std::env::var("USERPROFILE") {
            let user_cargo = Path::new(&user_dir).join(".cargo").join("bin").join("cargo.exe");
            if user_cargo.exists() {
                user_cargo.to_string_lossy().to_string()
            } else {
                "cargo".to_string()
            }
        } else {
            "cargo".to_string()
        };
        let mut a = vec!["build".to_string()];
        if let Some(extra) = &extra_args {
            if !extra.trim().is_empty() {
                a.push(extra.trim().to_string());
            }
        }
        ("Rust (Cargo)".to_string(), cargo_bin, a)
    } else if root.join("pyproject.toml").exists() || root.join("setup.py").exists() {
        // 4. Python
        let python_bin = if root.join("venv").join("Scripts").join("python.exe").exists() {
            root.join("venv").join("Scripts").join("python.exe").to_string_lossy().to_string()
        } else {
            "python".to_string()
        };
        ("Python (setup.py/pip)".to_string(), python_bin, vec!["-m".to_string(), "pip".to_string(), "install".to_string(), "-e".to_string(), ".".to_string()])
    } else {
        return json!({
            "success": false,
            "error": "No supported project build configuration detected (run.bat, gradlew, package.json, Cargo.toml, setup.py not found)"
        })
        .to_string();
    };

    use std::io::Write;
    println!("build started...");
    let _ = std::io::stdout().flush();

    let mut cmd = Command::new(&program);
    cmd.args(&args);
    cmd.current_dir(root);

    let output = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            return json!({
                "success": false,
                "stack": stack_name,
                "error": format!("Failed to spawn build command '{}': {}", program, e)
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
    let (final_stdout, final_stderr) = if success {
        ("success".to_string(), "".to_string())
    } else {
        let combined = format!("{}\n{}", stdout_str, stderr_str);
        let error_lines: Vec<&str> = combined
            .lines()
            .filter(|line| {
                let l = line.to_lowercase();
                l.contains("error")
                    || l.contains("failed")
                    || l.contains("exception")
                    || l.starts_with("(!)")
                    || l.contains("ts")
            })
            .collect();
        let filtered = if !error_lines.is_empty() {
            error_lines.join("\n")
        } else {
            combined
        };
        ("".to_string(), filtered)
    };

    json!({
        "success": success,
        "stack": stack_name,
        "exit_code": exit_code,
        "stdout": final_stdout,
        "stderr": final_stderr
    })
    .to_string()
}

/// Generate Android release bundle (.aab) locally with R8 mapping verification.
pub fn execute_release(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let root = Path::new(&workspace_root);
    if !root.exists() {
        return json!({
            "success": false,
            "error": format!("Workspace root does not exist: {}", root.display())
        })
        .to_string();
    }

    #[cfg(target_os = "windows")]
    let gradlew = if root.join("gradlew.bat").exists() {
        root.join("gradlew.bat").to_string_lossy().to_string()
    } else {
        "gradlew.bat".to_string()
    };
    #[cfg(not(target_os = "windows"))]
    let gradlew = if root.join("gradlew").exists() {
        "./gradlew".to_string()
    } else {
        "gradlew".to_string()
    };

    use std::io::Write;
    println!("release AAB build started...");
    let _ = std::io::stdout().flush();

    let mut cmd = Command::new(&gradlew);
    cmd.args(["bundleRelease", "-x", "test", "--console=plain"]);
    cmd.current_dir(root);

    let output = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            return json!({
                "success": false,
                "error": format!("Failed to spawn '{}': {}", gradlew, e)
            })
            .to_string();
        }
    };

    let exit_code = output.status.code().unwrap_or(-1);
    let stdout_str = String::from_utf8_lossy(&output.stdout);
    let stderr_str = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{}\n{}", stdout_str, stderr_str);

    if exit_code != 0 {
        let error_lines: Vec<&str> = combined
            .lines()
            .filter(|line| {
                let l = line.trim();
                l.starts_with("e: ")
                    || l.contains("error:")
                    || l.contains("FAILED")
                    || l.contains("Exception")
                    || l.contains("Caused by:")
                    || l.contains("ld.lld:")
            })
            .collect();

        let filtered_error = if !error_lines.is_empty() {
            error_lines.join("\n")
        } else {
            combined.trim().to_string()
        };

        return json!({
            "success": false,
            "exit_code": exit_code,
            "error": filtered_error
        })
        .to_string();
    }

    let aab_rel = root.join("app").join("build").join("outputs").join("bundle").join("release").join("app-release.aab");
    let mapping_rel = root.join("app").join("build").join("outputs").join("mapping").join("release").join("mapping.txt");

    if aab_rel.exists() {
        let size_bytes = std::fs::metadata(&aab_rel).map(|m| m.len()).unwrap_or(0);
        let size_mb = (size_bytes as f64) / (1024.0 * 1024.0);
        let r8_present = mapping_rel.exists();

        json!({
            "success": true,
            "action": "release",
            "aab_path": aab_rel.to_string_lossy().to_string(),
            "size_mb": (size_mb * 100.0).round() / 100.0,
            "r8_mapping_present": r8_present,
            "output": format!(
                "SUCCESS: Release AAB generated at {} ({:.2} MB){}",
                aab_rel.display(),
                size_mb,
                if r8_present { " | R8 mapping present" } else { "" }
            )
        })
        .to_string()
    } else {
        json!({
            "success": false,
            "error": format!("AAB file not found at expected path: {}", aab_rel.display())
        })
        .to_string()
    }
}

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
        return super::android::execute_android(&p, default_root);
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

/// Run dev server (`npm run dev`) or test-launch it.
/// If `spawn_background` is true (or when started in dev mode), it launches the process
/// and verifies it starts up within a few seconds, capturing initial stdout/stderr.
pub fn execute_dev(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&workspace_root);

    if !root.join("package.json").exists() {
        return json!({
            "success": false,
            "error": "No package.json found — dev only supports Node/npm projects."
        })
        .to_string();
    }

    let extra_args = params.get("extra_args").and_then(|v| v.as_str());

    // Detect package manager by lockfile
    let prog = if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    };

    #[cfg(target_os = "windows")]
    let (program, cmd_args) = {
        let mut a = vec!["/C".to_string(), prog.to_string(), "run".to_string(), "dev".to_string()];
        if let Some(extra) = extra_args {
            if !extra.trim().is_empty() {
                a.push(extra.trim().to_string());
            }
        }
        ("cmd.exe", a)
    };

    #[cfg(not(target_os = "windows"))]
    let (program, cmd_args) = {
        let mut a = vec!["run".to_string(), "dev".to_string()];
        if let Some(extra) = extra_args {
            if !extra.trim().is_empty() {
                a.push(extra.trim().to_string());
            }
        }
        (prog, a)
    };

    // Check if background run requested
    let is_background = params.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

    if is_background {
        #[cfg(target_os = "windows")]
        let spawn_res = Command::new(program)
            .args(&cmd_args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(not(target_os = "windows"))]
        let spawn_res = Command::new(program)
            .args(&cmd_args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        match spawn_res {
            Ok(child) => json!({
                "success": true,
                "stack": format!("{} run dev (background)", prog),
                "pid": child.id(),
                "message": format!("Dev server spawned in background (PID: {}).", child.id())
            })
            .to_string(),
            Err(e) => json!({
                "success": false,
                "error": format!("Failed to spawn dev server: {}", e)
            })
            .to_string(),
        }
    } else {
        // Launch detached from stdio so the parent RPC session does not inherit child pipe handles
        #[cfg(target_os = "windows")]
        let spawn_res = Command::new(program)
            .args(&cmd_args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        #[cfg(not(target_os = "windows"))]
        let spawn_res = Command::new(program)
            .args(&cmd_args)
            .current_dir(root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();

        let mut child = match spawn_res {
            Ok(c) => c,
            Err(e) => {
                return json!({
                    "success": false,
                    "error": format!("Failed to start dev server: {}", e)
                })
                .to_string();
            }
        };

        // Sleep 2 seconds to check if process exits immediately or stays running
        std::thread::sleep(std::time::Duration::from_secs(2));

        match child.try_wait() {
            Ok(Some(status)) => json!({
                "success": false,
                "stack": format!("{} run dev", prog),
                "exit_code": status.code().unwrap_or(-1),
                "message": "Dev server exited immediately."
            })
            .to_string(),
            Ok(None) => {
                let pid = child.id();
                json!({
                    "success": true,
                    "stack": format!("{} run dev", prog),
                    "pid": pid,
                    "status": "running",
                    "port": 5173,
                    "url": "http://localhost:5173",
                    "message": format!("Dev server is up and running on PID {} (http://localhost:5173).", pid)
                })
                .to_string()
            }
            Err(e) => json!({
                "success": false,
                "error": format!("Error monitoring dev server: {}", e)
            })
            .to_string(),
        }
    }
}

