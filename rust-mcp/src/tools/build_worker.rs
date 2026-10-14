//! Detached build worker.
//!
//! `mysid build` writes a job file and spawns `mysid __build_worker <workspace>` detached, so the
//! build survives the caller (an LLM tool call has a time limit; a build may not). The worker runs
//! the build with its full output streaming to `.mysid/build.log`, and rewrites
//! `.mysid/build.status.json` every second. The caller waits a short budget for the result and,
//! if the build is still going, returns a one-line progress report instead of hanging.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{build_gradle, build_log, build_rust, release_bump, release_check, release_web};

const JOB_FILE: &str = "build.job.json";
const STATUS_FILE: &str = "build.status.json";
const STATS_FILE: &str = "build.stats.json";
/// A `running` status not refreshed for this long means the worker died.
const STALE_MS: u64 = 20_000;
pub const DEFAULT_WAIT_SECS: u64 = 20;

/// How long a command waits for the result before it returns a progress line. A person at a terminal gets the
/// end of the build (up to 4 hours); an LLM tool call (stdout is a pipe) gets a short budget and checks back.
pub fn default_wait_secs() -> u64 {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() { 4 * 3600 } else { DEFAULT_WAIT_SECS }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Rust,
    Gradle,
    /// `bundleRelease`: reports the AAB and R8 mapping instead of the APK.
    GradleRelease,
    /// `mysid release` in a web project: build, then `netlify deploy --prod` (log: release.log).
    WebRelease,
    Other,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Job {
    pub stack: String,
    pub kind: Kind,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
    /// Raise versionCode by one before building (`mysid release`).
    #[serde(default)]
    pub bump: bool,
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct Status {
    /// `running`, `ok` or `failed`.
    pub state: String,
    pub stack: String,
    pub pid: u32,
    pub started_ms: u64,
    pub updated_ms: u64,
    pub tasks_done: usize,
    pub tasks_executed: usize,
    /// Task count of the last successful build of this stack, for a percentage.
    pub tasks_total_est: usize,
    pub current: String,
    pub warnings: usize,
    pub errors: usize,
    pub exit_code: i32,
    /// Final human-readable report (empty while running).
    pub summary: String,
    /// versionCode bump applied by this build (0 = none); each edited file with its old value,
    /// so a cancelled or failed build can put it back.
    pub bump_from: u64,
    pub bump_to: u64,
    pub bump_files: Vec<(String, u64)>,
    /// Current pipeline stage (`Kotlin compile`, `native C++`, ...) and the time spent in each so far.
    pub stage: String,
    pub stage_started_ms: u64,
    pub stage_ms: Vec<(String, u64)>,
    /// One entry per stage change: (seconds since start, stage, first task of that stage).
    pub stage_log: Vec<(u64, String, String)>,
}

pub fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Records a step mysid itself performs (preflight, version bump, verification...) as part of the
/// stage trail and the status line, so nothing mysid does is invisible while Gradle is quiet.
pub fn note(p: &mut Status, stage: &str, detail: &str) {
    if p.stage != stage {
        build_gradle::close_stage(p, now_ms());
        p.stage = stage.to_string();
        p.stage_started_ms = now_ms();
    }
    p.current = detail.to_string();
    p.stage_log.push((now_ms().saturating_sub(p.started_ms) / 1000, stage.to_string(), detail.to_string()));
}

pub fn fmt_secs(secs: u64) -> String {
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}

fn state_dir(workspace_root: &str) -> PathBuf {
    build_log::ensure_dir(Path::new(workspace_root)).unwrap_or_else(|_| {
        let dir = std::env::temp_dir().join("mysid");
        let _ = fs::create_dir_all(&dir);
        dir
    })
}

fn write_atomic(path: &Path, text: &str) {
    let tmp = path.with_extension("tmp");
    if fs::write(&tmp, text).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

fn write_status(dir: &Path, s: &Status) {
    if let Ok(text) = serde_json::to_string(s) {
        write_atomic(&dir.join(STATUS_FILE), &text);
    }
}

fn load_estimate(dir: &Path, stack: &str) -> usize {
    fs::read_to_string(dir.join(STATS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v.get(stack).and_then(|s| s.get("tasks")).and_then(|t| t.as_u64()))
        .unwrap_or(0) as usize
}

fn save_estimate(dir: &Path, stack: &str, tasks: usize, secs: u64) {
    let mut all = fs::read_to_string(dir.join(STATS_FILE))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    all[stack] = serde_json::json!({ "tasks": tasks, "secs": secs });
    write_atomic(&dir.join(STATS_FILE), &all.to_string());
}

/// Latest status, with a `running` status whose worker stopped updating reported as failed.
pub fn read_status(workspace_root: &str) -> Option<Status> {
    let text = fs::read_to_string(state_dir(workspace_root).join(STATUS_FILE)).ok()?;
    let mut s: Status = serde_json::from_str(&text).ok()?;
    if s.state == "running" && now_ms().saturating_sub(s.updated_ms) > STALE_MS {
        s.state = "failed".to_string();
        s.summary = format!(
            "BUILD FAILED - the build worker stopped responding after {} (see {}/{})",
            fmt_secs(s.updated_ms.saturating_sub(s.started_ms) / 1000),
            build_log::MYSID_DIR,
            build_log::BUILD_LOG
        );
    }
    Some(s)
}

/// Starts a build, or attaches to the one already running. Ok(true) means attached.
pub fn start(workspace_root: &str, job: &Job) -> Result<bool, String> {
    if read_status(workspace_root).map_or(false, |s| s.state == "running") {
        return Ok(true);
    }
    let dir = state_dir(workspace_root);
    let job_text = serde_json::to_string(job).map_err(|e| e.to_string())?;
    write_atomic(&dir.join(JOB_FILE), &job_text);

    let now = now_ms();
    write_status(
        &dir,
        &Status {
            state: "running".to_string(),
            stack: job.stack.clone(),
            started_ms: now,
            updated_ms: now,
            tasks_total_est: load_estimate(&dir, &job.stack),
            current: "starting".to_string(),
            ..Default::default()
        },
    );

    let exe = std::env::current_exe().map_err(|e| format!("cannot locate mysid executable: {}", e))?;
    // Without this the worker keeps a copy of our stdout/stderr pipe open, so `mysid build | tail`
    // would not finish until the whole build does.
    no_inherit_std_handles();
    let spawn = |breakaway: bool| {
        let mut cmd = Command::new(&exe);
        cmd.arg("__build_worker").arg(workspace_root).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut cmd, breakaway);
        cmd.spawn()
    };
    // Prefer leaving the caller's job object (so the build outlives a killed terminal); fall back
    // if the job forbids breakaway.
    spawn(true).or_else(|_| spawn(false)).map_err(|e| format!("failed to start build worker: {}", e))?;
    Ok(false)
}

/// Windows hands every inheritable handle to a child process, not just the ones named in its stdio.
/// Marking our own stdio handles non-inheritable keeps the detached worker from holding a caller's
/// pipe open after we exit.
#[cfg(windows)]
pub(super) fn no_inherit_std_handles() {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetStdHandle(n_std_handle: u32) -> *mut c_void;
        fn SetHandleInformation(handle: *mut c_void, mask: u32, flags: u32) -> i32;
    }
    const HANDLE_FLAG_INHERIT: u32 = 1;
    for id in [-10i32, -11, -12] {
        // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE
        unsafe {
            let handle = GetStdHandle(id as u32);
            if !handle.is_null() && handle as isize != -1 {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
            }
        }
    }
}

#[cfg(not(windows))]
pub(super) fn no_inherit_std_handles() {}

#[cfg(windows)]
pub(super) fn detach(cmd: &mut Command, breakaway: bool) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;
    let mut flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP;
    if breakaway {
        flags |= CREATE_BREAKAWAY_FROM_JOB;
    }
    cmd.creation_flags(flags);
}

#[cfg(unix)]
pub(super) fn detach(cmd: &mut Command, _breakaway: bool) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

/// `[0:21] :app:kspDebugKotlin (annotation processing)` for a Gradle task, and
/// `[0:00] preflight: JDK: JAVA_HOME ok` for a step mysid performs itself.
fn trail_line(entry: &(u64, String, String)) -> String {
    let (secs, stage, task) = entry;
    let at = format!("[{}:{:02}]", secs / 60, secs % 60);
    if task.is_empty() || task == stage {
        format!("{} {}", at, stage)
    } else if task.starts_with(':') {
        format!("{} {} ({})", at, task, stage)
    } else {
        format!("{} {}: {}", at, stage, task)
    }
}

/// Waits until the build finishes or `budget` elapses, whichever comes first. With `trail`, each
/// stage change is printed to stderr as it happens (stderr, so MCP's stdout stays clean), which
/// gives a start-to-finish picture even for builds that end inside the budget.
pub fn wait(workspace_root: &str, budget: Duration, trail: bool) -> Option<Status> {
    let t0 = Instant::now();
    let mut printed = 0usize;
    loop {
        let s = read_status(workspace_root)?;
        if trail {
            for entry in s.stage_log.iter().skip(printed).filter(|e| !(e.1.is_empty() && e.2.is_empty())) {
                eprintln!("{}", trail_line(entry));
            }
            printed = s.stage_log.len();
        }
        if s.state != "running" || t0.elapsed() >= budget {
            return Some(s);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

/// One short text block for an LLM: the final report, or a progress line while running.
pub fn render(s: &Status) -> String {
    if s.state != "running" {
        return s.summary.clone();
    }
    let elapsed = fmt_secs(now_ms().saturating_sub(s.started_ms) / 1000);
    let in_stage = if s.stage_started_ms > 0 { now_ms().saturating_sub(s.stage_started_ms) / 1000 } else { 0 };
    let stage_label = if in_stage >= 10 { format!("{}, {} in stage", s.stage, fmt_secs(in_stage)) } else { s.stage.clone() };
    let at = match (s.current.is_empty(), s.stage.is_empty()) {
        (false, false) if s.current.starts_with(':') => format!("{} ({})", s.current, stage_label),
        (false, false) if s.current != s.stage => format!("{}: {}", stage_label, s.current),
        (false, _) => s.current.clone(),
        (true, false) => stage_label,
        (true, true) => "starting".to_string(),
    };
    let progress = if s.tasks_total_est > 0 && s.tasks_done > 0 {
        format!(" - {}% (~{}/{} tasks)", (s.tasks_done * 100 / s.tasks_total_est).min(99), s.tasks_done, s.tasks_total_est)
    } else if s.tasks_done > 0 {
        format!(" - {} tasks done", s.tasks_done)
    } else {
        String::new()
    };
    let mut line = format!("BUILD RUNNING {} - at {}{}", elapsed, at, progress);
    if s.errors > 0 {
        line.push_str(&format!(" - {} error(s) so far", s.errors));
    } else if s.warnings > 0 {
        line.push_str(&format!(" - {} warning(s)", s.warnings));
    }
    let (word, log) = if s.stack == release_web::STACK { ("RELEASE", release_web::RELEASE_LOG) } else { ("BUILD", build_log::BUILD_LOG) };
    format!(
        "{}\nlog: {}/{}\ncheck: mysid build status   cancel: mysid build stop",
        line.replacen("BUILD", word, 1),
        build_log::MYSID_DIR,
        log
    )
}

/// Cancels the running build (worker and everything it started).
pub fn stop(workspace_root: &str) -> String {
    let dir = state_dir(workspace_root);
    let Some(mut s) = read_status(workspace_root).filter(|s| s.state == "running") else {
        return "No build is running.".to_string();
    };
    if s.pid != 0 {
        #[cfg(windows)]
        let _ = Command::new("taskkill").args(["/PID", &s.pid.to_string(), "/T", "/F"]).output();
        #[cfg(unix)]
        let _ = Command::new("kill").args(["-TERM", &format!("-{}", s.pid)]).output();
    }
    s.state = "failed".to_string();
    s.updated_ms = now_ms();
    s.summary = format!("BUILD CANCELLED after {}", fmt_secs(s.updated_ms.saturating_sub(s.started_ms) / 1000));
    if s.bump_to > 0 {
        restore_bump(&s);
        s.summary.push_str(&format!("; versionCode restored to {}", s.bump_from));
        s.bump_to = 0;
    }
    write_status(&dir, &s);
    s.summary
}

/// Entry point of the detached process (`mysid __build_worker <workspace>`).
pub fn run_worker(workspace_root: &str) {
    let dir = state_dir(workspace_root);
    let Some(job) = fs::read_to_string(dir.join(JOB_FILE)).ok().and_then(|t| serde_json::from_str::<Job>(&t).ok()) else {
        return;
    };
    let log_path = dir.join(build_log::BUILD_LOG);
    let started = Instant::now();
    let started_ms = now_ms();

    let status = Arc::new(Mutex::new(Status {
        state: "running".to_string(),
        stack: job.stack.clone(),
        pid: std::process::id(),
        started_ms,
        updated_ms: started_ms,
        tasks_total_est: load_estimate(&dir, &job.stack),
        current: "starting".to_string(),
        ..Default::default()
    }));

    // `mysid release`: report what it depends on before anything is changed.
    if job.kind == Kind::GradleRelease {
        let lines = release_check::preflight(Path::new(&job.cwd), &job.env);
        let mut s = status.lock().unwrap();
        for line in &lines {
            note(&mut s, "preflight", line);
        }
    }

    // `mysid release`: raise versionCode first. A failed or cancelled build puts it back.
    let mut bump_note = String::new();
    if job.bump {
        match release_bump::bump_version_code(Path::new(&job.cwd)) {
            Ok(Some(b)) => {
                let mut s = status.lock().unwrap();
                s.bump_from = b.from;
                s.bump_to = b.to;
                s.bump_files = b.files.iter().map(|(p, v)| (p.to_string_lossy().to_string(), *v)).collect();
                note(&mut s, "version bump", &format!("versionCode {} -> {}", b.from, b.to));
            }
            Ok(None) => bump_note = "versionCode not bumped: no literal versionCode found".to_string(),
            Err(e) => {
                let mut s = status.lock().unwrap().clone();
                s.state = "failed".to_string();
                s.updated_ms = now_ms();
                s.summary = format!("RELEASE ABORTED - could not bump versionCode: {}", e);
                write_status(&dir, &s);
                return;
            }
        }
    }

    // Heartbeat: rewrite the status file every second, even when the build prints nothing.
    let finished = Arc::new(AtomicBool::new(false));
    let heartbeat = {
        let (status, finished, dir) = (Arc::clone(&status), Arc::clone(&finished), dir.clone());
        thread::spawn(move || {
            while !finished.load(Ordering::Relaxed) {
                let snapshot = {
                    let mut s = status.lock().unwrap();
                    s.updated_ms = now_ms();
                    s.clone()
                };
                write_status(&dir, &snapshot);
                thread::sleep(Duration::from_secs(1));
            }
        })
    };

    if matches!(job.kind, Kind::Gradle | Kind::GradleRelease) {
        note(
            &mut status.lock().unwrap(),
            "gradle startup",
            "starting Gradle (daemon start and configuration print nothing until tasks begin)",
        );
    }

    // `mysid release` in a web project: preflight, build, netlify deploy; its own log (release.log).
    if job.kind == Kind::WebRelease {
        let (ok, exit_code, report) = release_web::release(Path::new(&job.cwd), &status);
        finished.store(true, Ordering::Relaxed);
        let _ = heartbeat.join();
        let mut fin = status.lock().unwrap().clone();
        build_gradle::close_stage(&mut fin, now_ms());
        fin.state = if ok { "ok" } else { "failed" }.to_string();
        fin.updated_ms = now_ms();
        fin.exit_code = exit_code;
        fin.summary = format!("{}\nlog: {}/{}", report, build_log::MYSID_DIR, release_web::RELEASE_LOG);
        write_status(&dir, &fin);
        return;
    }

    let on_line: Arc<dyn Fn(&str) + Send + Sync> = {
        let status = Arc::clone(&status);
        let kind = job.kind;
        Arc::new(move |line: &str| {
            let mut s = status.lock().unwrap();
            match kind {
                Kind::Gradle | Kind::GradleRelease => build_gradle::track(&mut s, line),
                Kind::Rust => build_rust::track(&mut s, line),
                Kind::WebRelease | Kind::Other => {
                    let t = line.trim();
                    if !t.is_empty() {
                        s.current = t.chars().take(80).collect();
                    }
                }
            }
        })
    };

    let mut cmd = Command::new(&job.program);
    cmd.args(&job.args).current_dir(&job.cwd);
    for (k, v) in &job.env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashing for the build tools
    }
    let bump_title = {
        let s = status.lock().unwrap();
        if s.bump_to > 0 { format!(" (versionCode {} -> {})", s.bump_from, s.bump_to) } else { String::new() }
    };
    let title = format!("mysid build [{}]{} {} {}", job.stack, bump_title, job.program, job.args.join(" "));
    let result = build_log::run_logged(cmd, &log_path, &title, on_line);

    // After a successful release build: check the bundle itself (still under the heartbeat, so the
    // status stays fresh while jarsigner runs).
    let mut signature_line: Option<String> = None;
    if job.kind == Kind::GradleRelease {
        if let Ok(run) = &result {
            if run.status.success() {
                let v = release_check::verify_release(Path::new(workspace_root), &job.env, started_ms);
                let mut s = status.lock().unwrap();
                for n in &v.notes {
                    note(&mut s, "verifying", n);
                }
                signature_line = Some(v.signature);
            }
        }
    }

    finished.store(true, Ordering::Relaxed);
    let _ = heartbeat.join();

    let elapsed = started.elapsed();
    let log_line = format!("log: {}/{}", build_log::MYSID_DIR, build_log::BUILD_LOG);
    let mut final_status = status.lock().unwrap().clone();
    build_gradle::close_stage(&mut final_status, now_ms());
    let (ok, exit_code, report) = match result {
        Ok(run) => {
            let ok = run.status.success();
            let report = match job.kind {
                Kind::Rust => build_rust::summarize(&run.stdout, &run.stderr, ok, elapsed, true),
                Kind::Gradle | Kind::GradleRelease => {
                    let combined = format!("{}\n{}", run.stdout, run.stderr);
                    let release = job.kind == Kind::GradleRelease;
                    build_gradle::summarize(&combined, ok, elapsed, &final_status, workspace_root, started_ms, release)
                }
                Kind::WebRelease | Kind::Other => summarize_other(&job.stack, &run.stdout, &run.stderr, ok, elapsed),
            };
            (ok, run.status.code().unwrap_or(-1), report)
        }
        Err(e) => (false, -1, format!("BUILD FAILED - could not start `{}`: {}", job.program, e)),
    };

    let mut report = report;
    if !bump_note.is_empty() {
        report.push_str(&format!("\n{}", bump_note));
    }
    if let Some(sig) = &signature_line {
        report.push_str(&format!("\nSignature: {}", sig));
    }
    if !ok && final_status.bump_to > 0 {
        restore_bump(&final_status);
        report.push_str(&format!("\nversionCode restored to {}", final_status.bump_from));
        final_status.bump_to = 0;
    }
    if ok && matches!(job.kind, Kind::Gradle | Kind::GradleRelease) && final_status.tasks_done > 0 {
        save_estimate(&dir, &job.stack, final_status.tasks_done, elapsed.as_secs());
    }
    final_status.state = if ok { "ok" } else { "failed" }.to_string();
    final_status.updated_ms = now_ms();
    final_status.exit_code = exit_code;
    final_status.summary = format!("{}\n{}", report, log_line);
    write_status(&dir, &final_status);
}

/// Generic stacks (npm, pip, custom scripts): status line plus, on failure, the last error-looking lines.
fn summarize_other(stack: &str, stdout: &str, stderr: &str, ok: bool, elapsed: Duration) -> String {
    if ok {
        return format!("BUILD OK ({}, {})", stack, fmt_secs(elapsed.as_secs()));
    }
    let combined = format!("{}\n{}", stdout, stderr);
    let mut hits: Vec<&str> = combined
        .lines()
        .filter(|l| {
            let l = l.to_lowercase();
            ["error", "failed", "exception", "fatal", "cannot", "not found"].iter().any(|m| l.contains(m))
        })
        .collect();
    if hits.is_empty() {
        hits = combined.lines().rev().take(15).collect::<Vec<_>>().into_iter().rev().collect();
    }
    let tail: Vec<&str> = hits.iter().rev().take(25).rev().copied().collect();
    format!("BUILD FAILED ({}, {})\n  {}", stack, fmt_secs(elapsed.as_secs()), tail.join("\n  "))
}

fn restore_bump(s: &Status) {
    let files: Vec<(PathBuf, u64)> = s.bump_files.iter().map(|(p, v)| (PathBuf::from(p), *v)).collect();
    release_bump::restore(&files);
}
