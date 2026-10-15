pub mod bump;
pub mod build_rust;
pub mod check;
pub mod debug;
pub mod exec;
pub mod gradle;
pub mod install;
pub mod install_rust;
pub mod lint;
pub mod log;
pub mod preflight;
pub mod release;
pub mod web;
pub mod worker;

pub use install::execute_install;
pub use release::execute_release;

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;

use std::time::Duration;

use worker::{Job, Kind};

const MAX_OUTPUT_BYTES: usize = 32_768;
use super::workspace::resolve_existing_root;

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
        // MCP clients send `action` (clean / status / stop / release); plain "build" is the default.
        .or_else(|| params.get("action").and_then(|v| v.as_str()).filter(|a| !a.eq_ignore_ascii_case("build")))
        .map(|s| s.to_string());

    if let Some(ref extra) = extra_args {
        let trimmed = extra.trim();
        // `release` means an Android AAB, except in a Cargo workspace where it means `--release`.
        let is_cargo = Path::new(&workspace_root).join("Cargo.toml").exists();
        if !is_cargo && (trimmed.eq_ignore_ascii_case("release") || trimmed.eq_ignore_ascii_case("aab")) {
            return release::execute_release(params, default_root);
        }
    }

    let wait = Duration::from_secs(
        params.get("wait").and_then(|v| v.as_u64()).unwrap_or(worker::default_wait_secs()),
    );
    match extra_args.as_deref().map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("status") => {
            return match worker::wait(&workspace_root, wait, false) {
                Some(st) => build_response(&st, false),
                None => json!({ "success": true, "output": "No build has been started in this workspace." }).to_string(),
            };
        }
        Some("stop") | Some("cancel") => {
            return json!({ "success": true, "output": worker::stop(&workspace_root) }).to_string();
        }
        _ => {}
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
    let has_gradlew = root.join("gradlew.bat").exists() || root.join("gradlew").exists();
    // Caches made at another path (the folder was moved) or a missing SDK fail a Gradle build minutes in: say so now.
    if has_gradlew {
        if let Some(answer) = super::stale::guard(root, params) {
            return answer;
        }
    }
    // A Gradle project is built by mysid directly: project scripts usually redirect Gradle's output
    // to a temp file, which hides all progress. (JAVA_HOME is still taken from the script.)
    let custom_run_bat = if has_gradlew {
        None
    } else if tools_run_bat.exists() {
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
        // 3. Rust (cargo): short diagnostics, summarized after the run
        let wants_release = params.get("release").and_then(|v| v.as_bool()).unwrap_or(false)
            || extra_args.as_deref().map(|e| e.trim().eq_ignore_ascii_case("release")).unwrap_or(false);
        let mut a = vec!["build".to_string()];
        if wants_release {
            a.push("--release".to_string());
        }
        if let Some(extra) = &extra_args {
            let e = extra.trim();
            if !e.is_empty() && !e.eq_ignore_ascii_case("release") {
                a.push(e.to_string());
            }
        }
        ("Rust (Cargo)".to_string(), build_rust::find_cargo(), a)
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

    let kind = match stack_name.as_str() {
        "Rust (Cargo)" => Kind::Rust,
        "Android (Gradle)" => Kind::Gradle,
        _ => Kind::Other,
    };
    let job = Job {
        stack: stack_name,
        kind,
        program,
        args,
        cwd: root.to_string_lossy().to_string(),
        env: java_home_env(root),
        bump: false,
    };

    // The build runs in a detached worker (full output streams to .mysid/build.log); wait a short
    // budget for the result and otherwise report progress instead of blocking the caller.
    let attached = match worker::start(&workspace_root, &job) {
        Ok(a) => a,
        Err(e) => return json!({ "success": false, "error": e }).to_string(),
    };
    if !attached {
        eprintln!("BUILD STARTED - {} - in {} - log: .mysid/build.log", job.stack, job.cwd);
    }
    match worker::wait(&workspace_root, wait, !attached) {
        Some(st) => build_response(&st, attached),
        None => json!({ "success": false, "error": "build status unavailable" }).to_string(),
    }
}

pub(crate) fn build_response(st: &worker::Status, attached: bool) -> String {
    let mut text = worker::render(st);
    if attached && st.state == "running" {
        text = format!("(a build is already running)
{}", text);
    }
    json!({
        "success": st.state != "failed",
        "state": st.state,
        "exit_code": st.exit_code,
        "output": text
    })
    .to_string()
}

/// The JDK a project script pins (`set "JAVA_HOME=..."` in run.bat), used when the environment
/// has no usable JAVA_HOME. Gradle is run directly, so the script's setting must be carried over.
pub(crate) fn java_home_env(root: &Path) -> Vec<(String, String)> {
    if std::env::var("JAVA_HOME").map_or(false, |h| Path::new(&h).exists()) {
        return Vec::new();
    }
    const PREFIX: &str = "set \"java_home=";
    for script in [root.join("tools").join("run.bat"), root.join("run.bat")] {
        let Ok(text) = std::fs::read_to_string(&script) else { continue };
        for line in text.lines() {
            let l = line.trim();
            if l.len() > PREFIX.len() && l[..PREFIX.len()].eq_ignore_ascii_case(PREFIX) {
                let value = l[PREFIX.len()..].trim_end_matches('"').trim();
                if !value.is_empty() && Path::new(value).exists() {
                    return vec![("JAVA_HOME".to_string(), value.to_string())];
                }
            }
        }
    }
    Vec::new()
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

