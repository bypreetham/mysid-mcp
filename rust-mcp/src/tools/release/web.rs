//! `mysid release` for a web project (package.json): build, then `netlify deploy --prod`.
//!
//! Runs inside the detached build worker. Progress is a few one-line crumbs (stage trail on stderr while the
//! caller waits); the caller's final answer is one line plus the live URL, or, on failure, only the error lines.
//! Everything, crumbs and the full output of every command, goes to `.mysid/release.log` so a person can follow
//! the deployment live (`Get-Content .mysid\release.log -Wait`).

use regex::Regex;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use crate::tools::build::worker::{self, Status};

pub const RELEASE_LOG: &str = "release.log";
pub const STACK: &str = "React release (Netlify)";

pub fn is_web_project(root: &Path) -> bool {
    root.join("package.json").exists()
}

struct Log {
    file: Mutex<File>,
    t0: Instant,
}

impl Log {
    fn line(&self, text: &str) {
        if let Ok(mut f) = self.file.lock() {
            let _ = writeln!(f, "{}", text);
            let _ = f.flush();
        }
    }
    fn crumb(&self, text: &str) {
        let s = self.t0.elapsed().as_secs();
        self.line(&format!("[{}:{:02}] {}", s / 60, s % 60, text));
    }
}

fn crumb(log: &Log, status: &Mutex<Status>, stage: &str, detail: &str) {
    log.crumb(&format!("{}: {}", stage, detail));
    worker::note(&mut status.lock().unwrap(), stage, detail);
}

/// A command (`npm`, `netlify` are .cmd shims on Windows) with the project as working directory.
fn tool(program: &str, args: &[&str], cwd: &Path) -> Command {
    #[cfg(windows)]
    let mut c = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd.exe");
        c.arg("/C").arg(program).args(args);
        c.creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = Command::new(program);
        c.args(args);
        c
    };
    c.current_dir(cwd).env("CI", "true").env("NO_COLOR", "1").env("FORCE_COLOR", "0").stdin(Stdio::null());
    c
}

/// Whether a program is on PATH (instant; `netlify --version` can take 20 s to start).
fn on_path(program: &str) -> bool {
    let finder = if cfg!(windows) { "where" } else { "which" };
    let mut c = Command::new(finder);
    c.arg(program).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    c.status().map_or(false, |s| s.success())
}

struct Run {
    ok: bool,
    code: i32,
    out: String,
}

/// Runs a command, appending its output to the log (indented) and returning it.
fn run(log: &Arc<Log>, mut cmd: Command) -> Result<Run, String> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| e.to_string())?;
    let pump = |r: Box<dyn std::io::Read + Send>, log: Arc<Log>| {
        thread::spawn(move || {
            let mut text = String::new();
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                log.line(&format!("    {}", line));
                text.push_str(&line);
                text.push('\n');
            }
            text
        })
    };
    let a = pump(Box::new(child.stdout.take().unwrap()), Arc::clone(log));
    let b = pump(Box::new(child.stderr.take().unwrap()), Arc::clone(log));
    let st = child.wait().map_err(|e| e.to_string())?;
    let (o, e) = (a.join().unwrap_or_default(), b.join().unwrap_or_default());
    Ok(Run { ok: st.success(), code: st.code().unwrap_or(-1), out: format!("{}{}", o, e) })
}

/// Only the lines that say what went wrong (at most `max`), else the tail.
fn error_lines(text: &str, max: usize) -> Vec<String> {
    let marks = ["error", "err!", "failed", "cannot", "not found", "unauthorized", "not logged", "denied", "exception"];
    let all: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('>')).collect();
    let hits: Vec<&str> = all.iter().copied().filter(|l| marks.iter().any(|m| l.to_lowercase().contains(m))).collect();
    let pick = if hits.is_empty() { all } else { hits };
    let skip = pick.len().saturating_sub(max);
    pick.into_iter().skip(skip).map(|l| l.chars().take(200).collect()).collect()
}

/// `publish = "dist"` from netlify.toml, if any.
fn publish_dir(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("netlify.toml")).ok()?;
    let re = Regex::new(r#"(?m)^\s*publish\s*=\s*"([^"]+)""#).unwrap();
    re.captures(&text).map(|c| c[1].to_string())
}

fn output_dir(root: &Path) -> Option<String> {
    if let Some(p) = publish_dir(root) {
        if root.join(&p).is_dir() {
            return Some(p);
        }
    }
    ["dist", "build", "out"].iter().find(|d| root.join(d).join("index.html").exists()).map(|d| d.to_string())
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

/// The URLs of the finished deploy, from `netlify deploy --json` (or the plain-text lines).
fn deploy_urls(out: &str) -> (Option<String>, Option<String>) {
    let json_start = out.find('{');
    if let Some(v) = json_start.and_then(|i| serde_json::from_str::<serde_json::Value>(&out[i..]).ok()) {
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(String::from);
        return (get("url"), get("deploy_url"));
    }
    let re = |label: &str| {
        Regex::new(&format!(r"(?i){}[^\n]*?(https?://\S+)", label)).unwrap().captures(out).map(|c| c[1].to_string())
    };
    (re("(?:website|production) url"), re("unique deploy url"))
}

fn fail(log: &Log, step: &str, secs: u64, lines: &[String]) -> (bool, i32, String) {
    log.crumb(&format!("FAILED at {}", step));
    let mut r = format!("RELEASE FAILED at {} (React, Netlify, {})", step, worker::fmt_secs(secs));
    for l in lines {
        r.push_str(&format!("\n  {}", l));
    }
    (false, 1, r)
}

/// The whole release. Returns (ok, exit code, report without the trailing `log:` line).
pub fn release(root: &Path, status: &Mutex<Status>) -> (bool, i32, String) {
    let dir = root.join(".mysid");
    let _ = fs::create_dir_all(&dir);
    let file = match OpenOptions::new().create(true).write(true).truncate(true).open(dir.join(RELEASE_LOG)) {
        Ok(f) => f,
        Err(e) => return (false, 1, format!("RELEASE FAILED: cannot write .mysid/{}: {}", RELEASE_LOG, e)),
    };
    let log = Arc::new(Log { file: Mutex::new(file), t0: Instant::now() });
    log.line(&format!("# mysid release [{}] {}", STACK, root.display()));
    let secs = || log.t0.elapsed().as_secs();

    // 1. preflight
    crumb(&log, status, "preflight", "package.json, netlify CLI, site link");
    let manifest = fs::read_to_string(root.join("package.json")).unwrap_or_default();
    if !manifest.contains("\"build\"") {
        return fail(&log, "preflight", secs(), &["package.json has no \"build\" script".to_string()]);
    }
    let pm = package_manager(root);
    let netlify_ok = on_path("netlify");
    if !netlify_ok {
        return fail(&log, "preflight", secs(), &["netlify CLI not found. Install: npm install -g netlify-cli, then: netlify login".to_string()]);
    }
    let linked = root.join(".netlify").join("state.json").exists() || std::env::var("NETLIFY_SITE_ID").is_ok();
    if !linked {
        return fail(&log, "preflight", secs(), &["no Netlify site linked. Run once in the project: netlify link (or netlify init)".to_string()]);
    }

    // 2. build
    crumb(&log, status, "build", &format!("{} run build", pm));
    let build = match run(&log, tool(pm, &["run", "build"], root)) {
        Ok(r) => r,
        Err(e) => return fail(&log, "build", secs(), &[e]),
    };
    if !build.ok {
        return fail(&log, "build", secs(), &error_lines(&build.out, 15));
    }
    let Some(out_dir) = output_dir(root) else {
        return fail(&log, "build", secs(), &["build finished but no output folder with index.html (dist, build, out, or netlify.toml publish)".to_string()]);
    };
    crumb(&log, status, "build", &format!("ok, output {}/", out_dir));

    // 3. deploy
    crumb(&log, status, "deploy", &format!("netlify deploy --prod --dir {}", out_dir));
    let deploy = match run(&log, tool("netlify", &["deploy", "--prod", "--dir", &out_dir, "--json"], root)) {
        Ok(r) => r,
        Err(e) => return fail(&log, "deploy", secs(), &[e]),
    };
    if !deploy.ok {
        return fail(&log, "deploy", secs(), &error_lines(&deploy.out, 15));
    }
    let (url, unique) = deploy_urls(&deploy.out);
    let Some(url) = url.or_else(|| unique.clone()) else {
        return fail(&log, "deploy", secs(), &[format!("deploy exited {} but printed no site URL (see .mysid/{})", deploy.code, RELEASE_LOG)]);
    };
    crumb(&log, status, "done", &url);
    let mut report = format!("RELEASE OK (React, Netlify, {})\nURL: {}", worker::fmt_secs(secs()), url);
    if let Some(u) = unique.filter(|u| *u != url) {
        report.push_str(&format!("\ndeploy: {}", u));
    }
    (true, 0, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_from_json_and_text() {
        let j = "Deploying...\n{\"url\":\"https://a.netlify.app\",\"deploy_url\":\"https://x--a.netlify.app\"}";
        assert_eq!(deploy_urls(j), (Some("https://a.netlify.app".into()), Some("https://x--a.netlify.app".into())));
        let t = "Unique deploy URL: https://u--a.netlify.app\nWebsite URL: https://a.netlify.app\n";
        assert_eq!(deploy_urls(t), (Some("https://a.netlify.app".into()), Some("https://u--a.netlify.app".into())));
    }

    #[test]
    fn error_lines_prefers_errors() {
        let t = "ok\nsrc/App.tsx(3,1): error TS2304: Cannot find name 'x'.\nmore\n";
        assert_eq!(error_lines(t, 5), vec!["src/App.tsx(3,1): error TS2304: Cannot find name 'x'.".to_string()]);
        assert_eq!(error_lines("a\nb\n", 1), vec!["b".to_string()]);
    }
}
