//! `mysid dev`: start the project's dev server in the background, read its log, stop it.
//!
//! `npm run dev` never exits, so it cannot go through `exec`. This starts it detached with its output in
//! `.mysid/dev.log`, waits until it prints a local URL that accepts connections, and answers with that URL. The agent
//! then opens the URL in a browser to check that the app really renders, reads `mysid dev log` for server errors, and
//! ends it with `mysid dev stop`.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::workspace::resolve_existing_root;
use super::{build_log, build_worker};

const STATE_FILE: &str = "dev.state.json";
const LOG_FILE: &str = "dev.log";
const DEFAULT_WAIT_SECS: u64 = 30;

pub const HELP: &str = "mysid dev: run the project's dev server in the background\n\n\
Usage:\n  \
  mysid dev                    start (npm/pnpm/yarn run dev, else start) and wait until it prints a local URL\n  \
  mysid dev --port 5174        pass a port to the server (vite: --port)\n  \
  mysid dev log                server errors from the log (else its last lines); --lines 40 for the raw tail\n  \
  mysid dev status             running? URL, port reachable, uptime\n  \
  mysid dev stop               stop it and everything it started\n\n\
Needs a package.json with a dev (or start) script. Output goes to .mysid/dev.log. The answer is the URL: open it in a\n\
browser to see the app; `mysid dev log` shows what the server said. Starting twice reuses the running server.\n";

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct State {
    pid: u32,
    started_ms: u64,
    command: String,
    url: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn dir_of(root: &str) -> PathBuf {
    build_log::ensure_dir(Path::new(root)).unwrap_or_else(|_| {
        let d = std::env::temp_dir().join("mysid");
        let _ = fs::create_dir_all(&d);
        d
    })
}

fn read_state(dir: &Path) -> State {
    fs::read_to_string(dir.join(STATE_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn write_state(dir: &Path, s: &State) {
    if let Ok(t) = serde_json::to_string(s) {
        let _ = fs::write(dir.join(STATE_FILE), t);
    }
}

fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
            .creation_flags(0x0800_0000)
            .output()
            .map_or(false, |o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{}\"", pid)))
    }
    #[cfg(not(windows))]
    {
        Command::new("kill").args(["-0", &pid.to_string()]).status().map_or(false, |s| s.success())
    }
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).creation_flags(0x0800_0000).output();
    }
    #[cfg(not(windows))]
    let _ = Command::new("kill").args(["-TERM", &format!("-{}", pid)]).output();
}

fn strip_ansi(s: &str) -> String {
    Regex::new(r"\x1b\[[0-9;?]*[A-Za-z]").unwrap().replace_all(s, "").to_string()
}

fn log_lines(dir: &Path) -> Vec<String> {
    let raw = fs::read(dir.join(LOG_FILE)).unwrap_or_default();
    strip_ansi(&String::from_utf8_lossy(&raw)).lines().map(|l| l.trim_end().to_string()).collect()
}

/// The first local URL the server printed (`Local: http://localhost:5173/`), else any URL with a port.
fn find_url(lines: &[String]) -> Option<String> {
    let re = Regex::new(r"https?://[A-Za-z0-9.\-\[\]:]+:(\d+)[^\s]*").unwrap();
    let all: Vec<String> = lines.iter().filter_map(|l| re.find(l).map(|m| m.as_str().to_string())).collect();
    all.iter().find(|u| u.contains("localhost") || u.contains("127.0.0.1") || u.contains("[::1]")).or_else(|| all.first()).cloned()
}

fn port_of(url: &str) -> Option<u16> {
    Regex::new(r":(\d+)").unwrap().captures_iter(url.trim_start_matches("http://").trim_start_matches("https://")).next().and_then(|c| c[1].parse().ok())
}

fn reachable(port: u16) -> bool {
    ["localhost", "127.0.0.1", "[::1]"].iter().any(|h| {
        format!("{}:{}", h, port).to_socket_addrs().map_or(false, |mut a| a.any(|addr| TcpStream::connect_timeout(&addr, Duration::from_millis(400)).is_ok()))
    })
}

fn error_lines(lines: &[String], max: usize) -> Vec<String> {
    let re = Regex::new(r"(?i)error|failed|cannot find|could not|exception|\bERR!|ENOENT|EADDRINUSE|✘|\[vite\].*(error|failed)").unwrap();
    let hits: Vec<String> = lines.iter().filter(|l| re.is_match(l)).map(|l| l.trim().chars().take(220).collect()).collect();
    let skip = hits.len().saturating_sub(max);
    hits.into_iter().skip(skip).collect()
}

fn fmt_secs(ms: u64) -> String {
    build_worker::fmt_secs(ms / 1000)
}

fn package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if root.join("yarn.lock").exists() {
        "yarn"
    } else {
        "npm"
    }
}

fn text(ok: bool, output: &str) -> String {
    json!({ "success": ok, "output": output }).to_string()
}

fn script_of(root: &Path) -> Result<String, String> {
    let manifest: Value = serde_json::from_str(&fs::read_to_string(root.join("package.json")).map_err(|_| "no package.json here: `mysid dev` runs Node projects".to_string())?)
        .map_err(|e| format!("package.json is not valid JSON: {}", e))?;
    let scripts = manifest.get("scripts").and_then(|s| s.as_object()).cloned().unwrap_or_default();
    for name in ["dev", "start"] {
        if scripts.contains_key(name) {
            return Ok(name.to_string());
        }
    }
    Err(format!("package.json has no \"dev\" or \"start\" script (scripts: {})", scripts.keys().cloned().collect::<Vec<_>>().join(", ")))
}

fn start(root: &str, params: &Value) -> String {
    let dir = dir_of(root);
    let state = read_state(&dir);
    if pid_alive(state.pid) {
        let current = serde_json::from_str::<Value>(&status(root)).ok().and_then(|v| v.get("output").and_then(|o| o.as_str()).map(String::from)).unwrap_or_default();
        return text(true, &format!("(already running)\n{}", current));
    }
    let rootp = Path::new(root);
    let script = match script_of(rootp) {
        Ok(s) => s,
        Err(e) => return text(false, &format!("DEV NOT STARTED: {}", e)),
    };
    let pm = package_manager(rootp);
    let port = params.get("port").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    let mut command = format!("{} run {}", pm, script);
    if let Some(p) = port {
        command.push_str(&format!(" -- --port {}", p));
    }

    let log = match File::create(dir.join(LOG_FILE)) {
        Ok(f) => f,
        Err(e) => return text(false, &format!("DEV NOT STARTED: cannot write .mysid/{}: {}", LOG_FILE, e)),
    };
    let err_log = match log.try_clone() {
        Ok(f) => f,
        Err(e) => return text(false, &format!("DEV NOT STARTED: {}", e)),
    };

    build_worker::no_inherit_std_handles();
    let spawn = |breakaway: bool| {
        #[cfg(windows)]
        let mut cmd = {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd.exe");
            c.raw_arg(format!("/S /C \"{}\"", command));
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", &command]);
            c
        };
        cmd.current_dir(rootp)
            .env("BROWSER", "none")
            .env("NO_COLOR", "1")
            .env("FORCE_COLOR", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(err_log.try_clone().unwrap()));
        build_worker::detach(&mut cmd, breakaway);
        cmd.spawn()
    };
    let child = match spawn(true).or_else(|_| spawn(false)) {
        Ok(c) => c,
        Err(e) => return text(false, &format!("DEV NOT STARTED: cannot run `{}`: {}", command, e)),
    };
    let pid = child.id();
    let started = now_ms();
    write_state(&dir, &State { pid, started_ms: started, command: command.clone(), url: String::new() });

    let wait = params.get("wait").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(DEFAULT_WAIT_SECS);
    let t0 = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(300));
        let lines = log_lines(&dir);
        if let Some(url) = find_url(&lines) {
            if port_of(&url).map_or(false, reachable) {
                write_state(&dir, &State { pid, started_ms: started, command: command.clone(), url: url.clone() });
                let errs = error_lines(&lines, 8);
                let mut out = format!("DEV RUNNING {} ({}, pid {}, ready in {:.1}s)", url, command, pid, t0.elapsed().as_secs_f64());
                if !errs.is_empty() {
                    out.push_str("\nthe server printed errors:");
                    for e in errs {
                        out.push_str(&format!("\n  {}", e));
                    }
                }
                out.push_str(&format!("\nlog: .mysid/{}   stop: mysid dev stop", LOG_FILE));
                return text(true, &out);
            }
        }
        if !pid_alive(pid) {
            let errs = error_lines(&lines, 15);
            let shown: Vec<String> = if errs.is_empty() { lines.iter().rev().take(10).rev().cloned().collect() } else { errs };
            let _ = fs::remove_file(dir.join(STATE_FILE));
            return text(false, &format!("DEV FAILED: `{}` exited\n  {}\nlog: .mysid/{}", command, shown.join("\n  "), LOG_FILE));
        }
        if t0.elapsed() >= Duration::from_secs(wait) {
            let tail: Vec<String> = lines.iter().rev().take(5).rev().cloned().collect();
            return text(
                true,
                &format!(
                    "DEV STARTING: no URL after {}s (still running, pid {}). Check again: mysid dev status\n  {}\nlog: .mysid/{}",
                    wait,
                    pid,
                    tail.join("\n  "),
                    LOG_FILE
                ),
            );
        }
    }
}

fn status(root: &str) -> String {
    let dir = dir_of(root);
    let mut s = read_state(&dir);
    if !pid_alive(s.pid) {
        let errs = error_lines(&log_lines(&dir), 5);
        let mut out = "DEV NOT RUNNING".to_string();
        if s.pid != 0 {
            out.push_str(" (the server has exited)");
            for e in errs {
                out.push_str(&format!("\n  {}", e));
            }
        }
        out.push_str("\nstart: mysid dev");
        return text(s.pid == 0, &out);
    }
    if s.url.is_empty() {
        if let Some(u) = find_url(&log_lines(&dir)) {
            s.url = u;
            write_state(&dir, &s);
        }
    }
    let up = fmt_secs(now_ms().saturating_sub(s.started_ms));
    let line = if s.url.is_empty() {
        format!("DEV STARTING ({}, up {}): no URL printed yet", s.command, up)
    } else {
        let ok = port_of(&s.url).map_or(false, reachable);
        format!("DEV RUNNING {} ({}, pid {}, up {}{})", s.url, s.command, s.pid, up, if ok { "" } else { ", port not answering" })
    };
    text(true, &format!("{}\nlog: .mysid/{}   stop: mysid dev stop", line, LOG_FILE))
}

fn log(root: &str, params: &Value) -> String {
    let dir = dir_of(root);
    let lines = log_lines(&dir);
    if lines.is_empty() {
        return text(true, "dev.log is empty: the server has not been started in this project (mysid dev).");
    }
    if let Some(n) = params.get("lines").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))) {
        let n = n.clamp(1, 400) as usize;
        let tail: Vec<&String> = lines.iter().rev().take(n).rev().collect();
        return text(true, &tail.iter().map(|s| s.as_str()).collect::<Vec<_>>().join("\n"));
    }
    let errs = error_lines(&lines, 20);
    if errs.is_empty() {
        let tail: Vec<String> = lines.iter().rev().take(5).rev().cloned().collect();
        return text(true, &format!("no errors in {} log line(s). Last lines:\n  {}", lines.len(), tail.join("\n  ")));
    }
    text(true, &format!("{} error line(s) of {}:\n  {}", errs.len(), lines.len(), errs.join("\n  ")))
}

fn stop(root: &str) -> String {
    let dir = dir_of(root);
    let s = read_state(&dir);
    if !pid_alive(s.pid) {
        let _ = fs::remove_file(dir.join(STATE_FILE));
        return text(true, "DEV NOT RUNNING (nothing to stop)");
    }
    kill_tree(s.pid);
    for _ in 0..20 {
        if !pid_alive(s.pid) {
            break;
        }
        thread::sleep(Duration::from_millis(150));
    }
    let gone = !pid_alive(s.pid);
    let port_free = s.url.is_empty() || port_of(&s.url).map_or(true, |p| !reachable(p));
    let _ = fs::remove_file(dir.join(STATE_FILE));
    if gone && port_free {
        text(true, &format!("DEV STOPPED ({}, ran {})", if s.url.is_empty() { s.command.clone() } else { s.url.clone() }, fmt_secs(now_ms().saturating_sub(s.started_ms))))
    } else {
        text(false, &format!("DEV STOP INCOMPLETE: pid {} {} and the port is {}. Close the node process by hand.", s.pid, if gone { "is gone" } else { "is still running" }, if port_free { "free" } else { "still answering" }))
    }
}

pub fn execute_dev(params: &Value, default_root: Option<&str>) -> String {
    let root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let q = ["query", "action"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).unwrap_or("").trim().to_lowercase();
    match q.as_str() {
        "" | "start" | "up" | "run" => start(&root, params),
        "status" => status(&root),
        "log" | "logs" => log(&root, params),
        "stop" | "down" | "kill" => stop(&root),
        other => text(false, &format!("unknown dev action '{}': use start (default), log, status, stop", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(String::from).collect()
    }

    #[test]
    fn the_local_url_is_preferred_over_the_network_one() {
        let l = lines("  VITE v5.0  ready in 300 ms\n  Network: http://192.168.1.5:5173/\n  Local:   http://localhost:5173/\n");
        assert_eq!(find_url(&l).as_deref(), Some("http://localhost:5173/"));
        assert_eq!(port_of("http://localhost:5173/"), Some(5173));
        assert_eq!(find_url(&lines("nothing yet")), None);
    }

    #[test]
    fn ansi_codes_are_removed_and_errors_picked() {
        assert_eq!(strip_ansi("\u{1b}[32mLocal\u{1b}[0m: x"), "Local: x");
        let l = lines("ready\n[vite] Internal server error: Failed to resolve import \"x\"\nok\n");
        assert_eq!(error_lines(&l, 5).len(), 1);
        assert!(error_lines(&lines("all fine"), 5).is_empty());
    }

    #[test]
    fn script_choice_and_unknown_action() {
        let d = std::env::temp_dir().join(format!("mysid_dev_{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        fs::write(d.join("package.json"), r#"{"scripts":{"build":"x","start":"y"}}"#).unwrap();
        assert_eq!(script_of(&d).unwrap(), "start");
        fs::write(d.join("package.json"), r#"{"scripts":{"build":"x"}}"#).unwrap();
        assert!(script_of(&d).unwrap_err().contains("no \"dev\""));
        let out = execute_dev(&json!({"workspace_root": d.to_string_lossy(), "query": "bogus"}), None);
        assert!(out.contains("unknown dev action"), "{}", out);
        let _ = fs::remove_dir_all(&d);
    }
}
