use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::tools::workspace::resolve_existing_root;

const MAX_OUTPUT_BYTES: usize = 32_768;
const DEFAULT_TIMEOUT_SECS: u64 = 30;

/// The shell command line for this platform.
///
/// On Windows the command goes to `cmd.exe /S /C "..."` as a raw argument. Passing it as a normal
/// argument makes Rust escape quotes with backslashes, which cmd does not understand, so any
/// command containing quotes, pipes or flags with quoted values arrived mangled.
fn shell(command: &str) -> Command {
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd.exe");
        c.raw_arg(format!("/S /C \"{}\"", command));
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// True when `command` has a `;` outside quotes (cmd.exe has no such separator; PowerShell and bash do).
fn has_unquoted_semicolon(command: &str) -> bool {
    let mut quote: Option<char> = None;
    for c in command.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (None, '"') | (None, '\'') => quote = Some(c),
            (None, ';') => return true,
            _ => {}
        }
    }
    false
}

fn on_path(program: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    Command::new(finder).arg(program).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map_or(false, |s| s.success())
}

/// The shell to run `command` in: the one asked for (`--shell cmd|powershell|bash`), else the platform's, except that on
/// Windows a command with `;` between commands goes to PowerShell. The note says when mysid chose for you.
fn shell_for(command: &str, wanted: Option<&str>) -> Result<(Command, Option<String>), String> {
    let wanted = wanted.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
    let auto_ps = cfg!(windows) && wanted.is_none() && has_unquoted_semicolon(command);
    match wanted.as_deref() {
        None if !auto_ps => Ok((shell(command), None)),
        Some("cmd") | Some("sh") if !cfg!(windows) || wanted.as_deref() == Some("cmd") => Ok((shell(command), None)),
        Some("bash") | Some("sh") => {
            // `bash` on a Windows PATH is often the WSL launcher: prefer Git for Windows' own.
            let git_bash = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
                .iter()
                .filter_map(|v| std::env::var(v).ok())
                .flat_map(|p| [Path::new(&p).join("Git").join("bin").join("bash.exe"), Path::new(&p).join("Programs").join("Git").join("bin").join("bash.exe")])
                .find(|p| p.exists());
            if git_bash.is_none() && !on_path("bash") {
                return Err("bash was not found (Git for Windows installs it). Use --shell powershell or cmd.".to_string());
            }
            let mut c = Command::new(git_bash.map(|p| p.to_string_lossy().to_string()).unwrap_or_else(|| "bash".to_string()));
            c.args(["-c", command]);
            Ok((c, None))
        }
        None | Some("powershell") | Some("pwsh") | Some("ps") => {
            let exe = if wanted.as_deref() != Some("powershell") && on_path("pwsh") { "pwsh" } else { "powershell" };
            let mut c = Command::new(exe);
            c.args(["-NoProfile", "-NonInteractive", "-Command", command]);
            let note = auto_ps.then(|| "[ran in PowerShell because the command has ';' (cmd.exe has no such separator). Choose with --shell cmd|powershell|bash]".to_string());
            Ok((c, note))
        }
        Some(other) => Err(format!("unknown shell '{}': use cmd, powershell or bash", other)),
    }
}

/// The `emsdk_env` script of the Emscripten SDK: `EMSDK`, then the usual install folders.
fn find_emsdk_env() -> Option<std::path::PathBuf> {
    let script = if cfg!(windows) { "emsdk_env.bat" } else { "emsdk_env.sh" };
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Ok(e) = std::env::var("EMSDK") {
        dirs.push(e.into());
    }
    if let Ok(h) = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
        dirs.push(Path::new(&h).join("emsdk"));
    }
    dirs.extend(["C:\\emsdk", "D:\\emsdk", "E:\\emsdk", "F:\\emsdk", "/opt/emsdk", "/usr/local/emsdk"].iter().map(|s| std::path::PathBuf::from(*s)));
    dirs.into_iter().map(|d| d.join(script)).find(|p| p.is_file())
}

/// `vcvars64.bat` of the newest Visual Studio that has the C++ tools: found with vswhere, else the usual paths.
#[cfg(windows)]
fn find_vcvars() -> Option<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| "C:\\Program Files (x86)".to_string());
    let vswhere = Path::new(&pf86).join("Microsoft Visual Studio").join("Installer").join("vswhere.exe");
    if vswhere.is_file() {
        let out = Command::new(&vswhere)
            .args(["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"])
            .creation_flags(0x0800_0000)
            .output()
            .ok()?;
        let base = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or("").trim().to_string();
        if !base.is_empty() {
            let bat = Path::new(&base).join("VC").join("Auxiliary").join("Build").join("vcvars64.bat");
            if bat.is_file() {
                return Some(bat);
            }
        }
    }
    for root in ["C:\\Program Files", "C:\\Program Files (x86)"] {
        for year in ["2022", "2019", "2017"] {
            for edition in ["Enterprise", "Professional", "Community", "BuildTools"] {
                let bat = Path::new(root).join("Microsoft Visual Studio").join(year).join(edition).join("VC").join("Auxiliary").join("Build").join("vcvars64.bat");
                if bat.is_file() {
                    return Some(bat);
                }
            }
        }
    }
    None
}

/// `command` with a toolchain's environment set up first: `msvc` (Visual Studio C++) or `emsdk` (Emscripten).
fn wrap_toolchain(env: &str, command: &str) -> Result<String, String> {
    match env {
        "msvc" | "cl" => {
            #[cfg(windows)]
            {
                let bat = find_vcvars().ok_or("Visual Studio with the C++ tools was not found (vswhere, then the usual folders). Install the 'Desktop development with C++' workload.")?;
                Ok(format!("call \"{}\" >nul 2>&1 && {}", bat.display(), command))
            }
            #[cfg(not(windows))]
            {
                let _ = command;
                Err("msvc is Windows only".to_string())
            }
        }
        "emsdk" | "wasm" | "emcc" => {
            let script = find_emsdk_env().ok_or("the Emscripten SDK was not found: set EMSDK, or install it as %USERPROFILE%\\emsdk or C:\\emsdk (https://emscripten.org/docs/getting_started/downloads.html)")?;
            if cfg!(windows) {
                Ok(format!("call \"{}\" >nul 2>&1 && {}", script.display(), command))
            } else {
                Ok(format!(". \"{}\" >/dev/null 2>&1 && {}", script.display(), command))
            }
        }
        other => Err(format!("unknown --env '{}': use msvc or emsdk", other)),
    }
}

/// Whether `full` is inside `root` (lexically: `..` is resolved, symlinks are not followed).
pub(crate) fn inside(root: &Path, full: &Path) -> bool {
    use std::path::Component;
    let norm = |p: &Path| {
        let mut out: Vec<std::ffi::OsString> = Vec::new();
        for c in p.components() {
            match c {
                Component::ParentDir => {
                    out.pop();
                }
                Component::CurDir => {}
                other => out.push(other.as_os_str().to_ascii_lowercase()),
            }
        }
        out
    };
    let (r, f) = (norm(root), norm(full));
    f.len() >= r.len() && f[..r.len()] == r[..]
}

fn read_all(pipe: Option<impl Read>) -> String {
    let mut buf = Vec::new();
    if let Some(mut p) = pipe {
        let _ = p.read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).to_string()
}

fn kill_tree(pid: u32) {
    #[cfg(target_os = "windows")]
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    #[cfg(not(target_os = "windows"))]
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).output();
}

pub(crate) struct Finished {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Runs the command, killing it (and its children) if it outlives `timeout`.
pub(crate) fn run_with_timeout(mut cmd: Command, timeout: Duration) -> std::io::Result<Finished> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    let t_out = thread::spawn(move || read_all(out));
    let t_err = thread::spawn(move || read_all(err));

    let started = Instant::now();
    let mut timed_out = false;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            kill_tree(child.id());
            let _ = child.kill();
            break child.wait().ok();
        }
        thread::sleep(Duration::from_millis(25));
    };

    Ok(Finished {
        exit_code: status.and_then(|s| s.code()).unwrap_or(-1),
        stdout: t_out.join().unwrap_or_default(),
        stderr: t_err.join().unwrap_or_default(),
        timed_out,
    })
}

/// Keeps the last `MAX_OUTPUT_BYTES` of `s`, cutting on a character boundary.
fn tail(s: String) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s;
    }
    let mut start = s.len() - MAX_OUTPUT_BYTES;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("[... truncated earlier output ...]\n{}", &s[start..])
}

pub fn execute_exec(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let command = match params.get("command").and_then(|v| v.as_str()) {
        Some(c) if !c.trim().is_empty() => c.trim().to_string(),
        _ => return json!({ "success": false, "error": "Missing or empty `command` parameter." }).to_string(),
    };
    let timeout_secs = ["timeout_secs", "timeout"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_u64()))
        .unwrap_or(DEFAULT_TIMEOUT_SECS);

    // `--out` must stay inside the workspace: a path like ../../x.log would leave a stray file behind.
    if let Some(f) = params.get("out").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
        let p = Path::new(f);
        let full = if p.is_absolute() { p.to_path_buf() } else { Path::new(&workspace_root).join(p) };
        if !inside(Path::new(&workspace_root), &full) {
            return json!({
                "success": false,
                "output": format!("NOT RUN: --out {} is outside the workspace ({}). Use a path inside it, for example .mysid/out.log; the command was not run.", f, workspace_root)
            })
            .to_string();
        }
    }
    // `--env msvc|emsdk`: the toolchain's environment first (cmd.exe on Windows, so `call x.bat` works).
    let toolchain = params.get("env").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    let to_run = match toolchain {
        Some(e) => match wrap_toolchain(e, &command) {
            Ok(c) => c,
            Err(msg) => return json!({ "success": false, "output": format!("NOT RUN: {}", msg) }).to_string(),
        },
        None => command.clone(),
    };
    let shell_choice = params.get("shell").and_then(|v| v.as_str()).or(if toolchain.is_some() && cfg!(windows) { Some("cmd") } else { None });
    // `gradlew clean` on a moved project fails minutes in on caches that name the old folder: refuse with the fix.
    if command.contains("gradlew") && !command.contains("--stop") {
        if let Some(answer) = crate::tools::stale::guard(Path::new(&workspace_root), params) {
            let msg = serde_json::from_str::<Value>(&answer).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(String::from)).unwrap_or(answer);
            return json!({ "success": false, "output": msg }).to_string();
        }
    }
    let (mut cmd, shell_note) = match shell_for(&to_run, shell_choice) {
        Ok(x) => x,
        Err(e) => return json!({ "success": false, "output": format!("NOT RUN: {}", e) }).to_string(),
    };
    cmd.current_dir(Path::new(&workspace_root));
    // Rust installed through rustup keeps cargo in the toolchain folder; when `cargo` is not on PATH (nothing in ~/.cargo/bin),
    // add that folder at the END of PATH so `cargo test` / `rustc` work like `mysid build` does.
    if !on_path("cargo") {
        let found = crate::tools::build_rust::find_cargo();
        if let Some(dir) = Path::new(&found).parent().filter(|d| d.is_dir() && !d.as_os_str().is_empty()) {
            let sep = if cfg!(windows) { ";" } else { ":" };
            let path = std::env::var("PATH").unwrap_or_default();
            cmd.env("PATH", format!("{}{}{}", path, sep, dir.display()));
        }
    }
    // emsdk_env.bat prints bash `export` lines instead of setting variables when it sees Git Bash's MSYSTEM / SHELL.
    if toolchain.is_some() && cfg!(windows) {
        for v in ["MSYSTEM", "SHELL", "MSYSCON", "MSYS", "MSYS2_PATH_TYPE", "MINGW_PREFIX", "MINGW_CHOST"] {
            cmd.env_remove(v);
        }
    }

    let run = match run_with_timeout(cmd, Duration::from_secs(timeout_secs)) {
        Ok(r) => r,
        Err(e) => return json!({ "success": false, "error": format!("Failed to execute process: {}", e) }).to_string(),
    };

    // `--out <file>`: everything the command printed goes to a file, even when it was killed at the timeout.
    let out_file = params.get("out").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()).map(|f| {
        let p = Path::new(f.trim());
        let full = if p.is_absolute() { p.to_path_buf() } else { Path::new(&workspace_root).join(p) };
        let body = format!(
            "$ {}\n# {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            command,
            if run.timed_out { format!("timed out after {}s", timeout_secs) } else { format!("exit {}", run.exit_code) },
            run.stdout,
            run.stderr
        );
        if let Some(dir) = full.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&full, &body) {
            Ok(_) => format!("[full output: {} ({} bytes)]", f.trim(), body.len()),
            Err(e) => format!("[could not write {}: {}]", f.trim(), e),
        }
    });

    let stdout = tail(run.stdout).trim().to_string();
    let stderr = tail(run.stderr).trim().to_string();
    let success = run.exit_code == 0 && !run.timed_out;

    // Plain text for the CLI: the output itself, then only what needs explaining.
    let mut text = stdout.clone();
    if !stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("[stderr]\n{}", stderr));
    }
    if run.timed_out {
        text.push_str(&format!("\n[timed out after {}s and was killed; raise it with --timeout <secs>]", timeout_secs));
    } else if run.exit_code != 0 {
        let note = if run.exit_code == 1 && stdout.is_empty() && stderr.is_empty() {
            " - no output; for grep/findstr/diff this means no match or no difference"
        } else {
            ""
        };
        text.push_str(&format!("\n[exit {}{}]", run.exit_code, note));
    }

    if let Some(note) = out_file {
        text.push('\n');
        text.push_str(&note);
    }
    if let Some(note) = shell_note {
        text.push('\n');
        text.push_str(&note);
    }

    json!({
        "success": success,
        "exit_code": run.exit_code,
        "timed_out": run.timed_out,
        "stdout": stdout,
        "stderr": stderr,
        "output": text.trim()
    })
    .to_string()
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn semicolons_are_found_outside_quotes_only() {
        assert!(has_unquoted_semicolon("ls a; where b"));
        assert!(!has_unquoted_semicolon("echo \"a;b\" && echo 'c;d'"));
        assert!(!has_unquoted_semicolon("cd x && y"));
    }

    #[test]
    fn out_paths_must_stay_inside_the_workspace() {
        let root = Path::new("C:/proj/app");
        assert!(inside(root, &root.join(".mysid").join("out.log")));
        assert!(inside(root, &root.join("a").join("..").join("b.log")));
        assert!(!inside(root, &root.join("..").join("..").join("build.log")));
        assert!(!inside(root, Path::new("D:/other/x.log")));
    }
}

// These drive cmd.exe (findstr, ping), so they only run on Windows.
#[cfg(all(test, windows))]
mod tests {
    use super::*;

    fn run(command: &str, extra: Value) -> Value {
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let mut p = json!({ "workspace_root": dir, "command": command });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        serde_json::from_str(&execute_exec(&p, Some(&dir))).unwrap()
    }

    #[test]
    fn quotes_and_pipes_reach_the_shell_intact() {
        let r = run("echo hello \"two words\" | findstr two", json!({}));
        assert_eq!(r["success"], true, "{}", r);
        assert!(r["output"].as_str().unwrap().contains("two words"), "{}", r);
    }

    #[test]
    fn exit_code_one_without_output_is_explained() {
        let r = run("echo abc | findstr zzz", json!({}));
        assert_eq!(r["success"], false);
        assert!(r["output"].as_str().unwrap().contains("no output"), "{}", r);
    }

    #[test]
    fn stderr_and_exit_code_are_reported() {
        let r = run("echo oops 1>&2 && exit /b 3", json!({}));
        let out = r["output"].as_str().unwrap();
        assert!(out.contains("[stderr]") && out.contains("oops") && out.contains("[exit 3]"), "{}", r);
    }

    #[test]
    fn a_command_that_outlives_the_timeout_is_killed() {
        let started = Instant::now();
        let r = run("ping -n 20 127.0.0.1", json!({ "timeout_secs": 1 }));
        assert_eq!(r["timed_out"], true, "{}", r);
        assert!(started.elapsed() < Duration::from_secs(8), "should return near the timeout");
        assert!(r["output"].as_str().unwrap().contains("timed out after 1s"), "{}", r);
    }
}
