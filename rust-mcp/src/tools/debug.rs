//! `mysid debug "Buffer, router, usb"`: clears the device log, then records only the log lines that
//! mention those keywords (in the tag or the message) into `.mysid/debug.log`.
//!
//! A detached recorder (`mysid __debug_recorder <workspace>`) streams `adb logcat` and writes the
//! matching lines as they arrive, so nothing the programmer logged with Log.d / LOGD is lost
//! between two questions and the file stays small. `mysid debug` without keywords reads it back.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::android::adb::resolve_adb_binary;
use super::build_log;
use super::build_worker::{detach, fmt_secs, no_inherit_std_handles, now_ms};
use super::workspace::resolve_existing_root;

const LOG_FILE: &str = "debug.log";
const JOB_FILE: &str = "debug.job.json";
const STATE_FILE: &str = "debug.state.json";
const TOUCH_FILE: &str = "debug.touch";
const CURSOR_FILE: &str = "debug.cursor";
/// Start a fresh file when the log gets this big (the old one is kept as `debug.log.old`).
const ROTATE_BYTES: u64 = 32 * 1024 * 1024;
/// A query never reads more than this from the end of the log.
const TAIL_BYTES: u64 = 8 * 1024 * 1024;
/// The recorder exits when no `mysid debug` command has touched it for this long.
const IDLE_EXIT_MS: u64 = 30 * 60 * 1000;
const ALIVE_MS: u64 = 5_000;
const DEFAULT_LINES: usize = 60;
const MAX_MSG: usize = 220;

#[derive(Serialize, Deserialize, Default, Clone)]
#[serde(default)]
struct State {
    pid: u32,
    started_ms: u64,
    updated_ms: u64,
    /// Lines written to the log since the recorder started.
    saved: u64,
    note: String,
    /// The recording window ended and the recorder exited by itself.
    done: bool,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(default)]
struct Job {
    keywords: Vec<String>,
    /// Seconds the recorder runs before stopping on its own (0 = until stopped / idle).
    window_secs: u64,
    /// Epoch ms at which the window ends (0 = none).
    until_ms: u64,
}

const WINDOW_MIN_SECS: u64 = 10;
const WINDOW_MAX_SECS: u64 = 60;
const WINDOW_DEFAULT_SECS: u64 = 20;

/// (asked, used): the recording window from `--seconds N` / `-N` / `--raw N`, default 20, clamped to 10-60.
fn window_secs(params: &Value) -> (u64, u64) {
    let asked = [params.get("seconds"), params.get("secs"), params.get("raw")]
        .into_iter()
        .map(parse_secs)
        .find(|n| *n > 0)
        .unwrap_or(WINDOW_DEFAULT_SECS);
    (asked, asked.clamp(WINDOW_MIN_SECS, WINDOW_MAX_SECS))
}

fn dir_of(ws: &str) -> PathBuf {
    build_log::ensure_dir(Path::new(ws)).unwrap_or_else(|_| {
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
        let tmp = dir.join("debug.state.tmp");
        if fs::write(&tmp, t).is_ok() {
            let _ = fs::rename(&tmp, dir.join(STATE_FILE));
        }
    }
}

fn read_job(dir: &Path) -> Job {
    fs::read_to_string(dir.join(JOB_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn alive(s: &State) -> bool {
    s.pid != 0 && now_ms().saturating_sub(s.updated_ms) < ALIVE_MS
}

fn kill_tree(pid: u32) {
    #[cfg(windows)]
    let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
    #[cfg(unix)]
    let _ = Command::new("kill").args(["-TERM", &format!("-{}", pid)]).output();
}

fn touch(dir: &Path) {
    let _ = fs::write(dir.join(TOUCH_FILE), now_ms().to_string());
}

fn size_of(path: &Path) -> u64 {
    fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

fn human(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

// ── recorder (the detached process) ─────────────────────────────────────────────────────────────

/// Clears the device log, empties the recording and starts a recorder for `keywords`.
fn start_recorder(ws: &str, keywords: &[String], window_secs: u64) -> Result<(), String> {
    let dir = dir_of(ws);
    // A recorder for other keywords (or a stale one) must not keep writing into the new recording.
    let old = read_state(&dir);
    if old.pid != 0 {
        kill_tree(old.pid);
        let _ = fs::remove_file(dir.join(STATE_FILE));
    }
    let _ = Command::new(resolve_adb_binary()).args(["logcat", "-c"]).stdin(Stdio::null()).output();
    let _ = File::create(dir.join(LOG_FILE));
    let _ = fs::remove_file(dir.join("debug.log.old"));
    let _ = fs::remove_file(dir.join(CURSOR_FILE));
    let until_ms = if window_secs > 0 { now_ms() + window_secs * 1000 } else { 0 };
    let job = serde_json::to_string(&Job { keywords: keywords.to_vec(), window_secs, until_ms }).map_err(|e| e.to_string())?;
    fs::write(dir.join(JOB_FILE), job).map_err(|e| e.to_string())?;
    touch(&dir);

    let exe = std::env::current_exe().map_err(|e| format!("cannot locate mysid executable: {}", e))?;
    no_inherit_std_handles();
    let spawn = |breakaway: bool| {
        let mut cmd = Command::new(&exe);
        cmd.arg("__debug_recorder").arg(ws).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
        detach(&mut cmd, breakaway);
        cmd.spawn()
    };
    spawn(true).or_else(|_| spawn(false)).map_err(|e| format!("failed to start the log recorder: {}", e))?;
    // The recorder announces itself in the state file; wait briefly so the caller can say so.
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        if alive(&read_state(&dir)) {
            break;
        }
    }
    Ok(())
}

fn set_note(note: &Arc<Mutex<String>>, text: String) {
    if let Ok(mut n) = note.lock() {
        *n = text;
    }
}

/// Entry point of the detached process (`mysid __debug_recorder <workspace>`).
pub fn run_recorder(ws: &str) {
    let dir = dir_of(ws);
    let log_path = dir.join(LOG_FILE);
    let started = now_ms();
    let job = read_job(&dir);
    let until_ms = job.until_ms;
    let keywords: Vec<String> = job.keywords.iter().map(|k| k.to_lowercase()).collect();
    let note = Arc::new(Mutex::new(String::from("starting")));
    let saved = Arc::new(AtomicU32::new(0));
    let child_pid = Arc::new(AtomicU32::new(0));

    // Heartbeat + idle shutdown, independent of whether logcat is producing anything.
    {
        let (dir, note, saved, child_pid) = (dir.clone(), Arc::clone(&note), Arc::clone(&saved), Arc::clone(&child_pid));
        thread::spawn(move || loop {
            let now = now_ms();
            let finished = until_ms > 0 && now >= until_ms;
            write_state(
                &dir,
                &State {
                    pid: if finished { 0 } else { std::process::id() },
                    started_ms: started,
                    updated_ms: now,
                    saved: saved.load(Ordering::Relaxed) as u64,
                    note: note.lock().map(|n| n.clone()).unwrap_or_default(),
                    done: finished,
                },
            );
            if finished {
                // The window is over: stop logcat and leave; the log file is complete.
                let c = child_pid.load(Ordering::SeqCst);
                if c != 0 {
                    kill_tree(c);
                }
                std::process::exit(0);
            }
            let touched = fs::read_to_string(dir.join(TOUCH_FILE)).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(started);
            if now.saturating_sub(touched) > IDLE_EXIT_MS {
                let c = child_pid.load(Ordering::SeqCst);
                if c != 0 {
                    kill_tree(c);
                }
                let _ = fs::remove_file(dir.join(STATE_FILE));
                std::process::exit(0);
            }
            thread::sleep(Duration::from_millis(if until_ms > 0 { 250 } else { 1000 }));
        });
    }

    let adb = resolve_adb_binary();
    let open_log = || OpenOptions::new().create(true).append(true).open(&log_path);
    let Ok(mut log) = open_log() else { return };
    let mut written = size_of(&log_path);

    loop {
        let mut cmd = Command::new(&adb);
        cmd.args(["logcat", "-v", "threadtime"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                set_note(&note, format!("cannot run adb: {}", e));
                thread::sleep(Duration::from_secs(3));
                continue;
            }
        };
        child_pid.store(child.id(), Ordering::SeqCst);
        set_note(&note, "recording".to_string());
        let stderr = child.stderr.take();
        let stderr_note = Arc::clone(&note);
        let err_thread = thread::spawn(move || {
            if let Some(e) = stderr {
                for line in BufReader::new(e).lines().flatten() {
                    let line = line.trim().to_string();
                    if !line.is_empty() {
                        set_note(&stderr_note, format!("adb: {}", line));
                    }
                }
            }
        });
        if let Some(out) = child.stdout.take() {
            let mut reader = BufReader::new(out);
            let mut raw = Vec::new();
            loop {
                raw.clear();
                match reader.read_until(b'\n', &mut raw) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let line = String::from_utf8_lossy(&raw).trim_end().to_string();
                let Some(e) = parse_line(&line) else { continue };
                if !matches_any(&e, &keywords) {
                    continue;
                }
                let text = format!("{}\n", line);
                if log.write_all(text.as_bytes()).is_err() {
                    break;
                }
                saved.fetch_add(1, Ordering::Relaxed);
                written += text.len() as u64;
                if written > ROTATE_BYTES {
                    drop(log);
                    let _ = fs::rename(&log_path, dir.join("debug.log.old"));
                    match open_log() {
                        Ok(f) => log = f,
                        Err(_) => return,
                    }
                    written = 0;
                }
            }
        }
        let _ = child.wait();
        let _ = err_thread.join();
        child_pid.store(0, Ordering::SeqCst);
        let last = note.lock().map(|n| n.clone()).unwrap_or_default();
        let why = if last.starts_with("adb:") { last } else { "adb logcat ended".to_string() };
        set_note(&note, format!("waiting for a device ({})", why));
        thread::sleep(Duration::from_secs(3));
    }
}

// ── log lines ───────────────────────────────────────────────────────────────────────────────────

struct Entry {
    time: String,
    level: char,
    tag: String,
    msg: String,
}

/// `MM-DD HH:MM:SS.mmm  PID  TID L Tag     : message` (logcat's threadtime format).
fn parse_line(line: &str) -> Option<Entry> {
    let mut fields: Vec<&str> = Vec::with_capacity(5);
    let mut rest = line.trim_start();
    for _ in 0..5 {
        let end = rest.find(char::is_whitespace)?;
        fields.push(&rest[..end]);
        rest = rest[end..].trim_start();
    }
    let level = fields[4].chars().next()?;
    if fields[4].len() != 1 || !"VDIWEFS".contains(level) {
        return None;
    }
    let (tag, msg) = match rest.split_once(": ") {
        Some((t, m)) => (t.trim().to_string(), m.to_string()),
        None => (rest.trim_end_matches(':').trim().to_string(), String::new()),
    };
    Some(Entry { time: fields[1].to_string(), level, tag, msg })
}

/// Case-insensitive substring match of any keyword against the tag or the message. No keywords: everything.
fn matches_any(e: &Entry, lowered: &[String]) -> bool {
    if lowered.is_empty() {
        return true;
    }
    let hay = format!("{} {}", e.tag, e.msg).to_lowercase();
    lowered.iter().any(|k| hay.contains(k.as_str()))
}

fn level_rank(l: char) -> u8 {
    match l {
        'V' => 0,
        'D' => 1,
        'I' => 2,
        'W' => 3,
        'E' => 4,
        _ => 5,
    }
}

/// Lines of the log from byte `from` (never more than `TAIL_BYTES` from the end), and the file length.
fn read_from(path: &Path, from: u64) -> (Vec<String>, u64) {
    let Ok(mut f) = File::open(path) else { return (Vec::new(), 0) };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = from.max(len.saturating_sub(TAIL_BYTES)).min(len);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return (Vec::new(), len);
    }
    let mut bytes = Vec::new();
    let _ = f.take(len - start).read_to_end(&mut bytes);
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    if start > from && !lines.is_empty() {
        lines.remove(0); // began mid-line
    }
    (lines, len)
}

/// Keywords from `"Buffer, router, usb"` (comma-separated, so phrases survive) or `usb router`.
fn split_keywords(q: &str) -> Vec<String> {
    let parts: Vec<&str> = if q.contains(',') { q.split(',').collect() } else { q.split_whitespace().collect() };
    parts.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn same_keywords(a: &[String], b: &[String]) -> bool {
    let norm = |v: &[String]| {
        let mut n: Vec<String> = v.iter().map(|s| s.to_lowercase()).collect();
        n.sort();
        n.dedup();
        n
    };
    norm(a) == norm(b)
}

fn clip(s: &str) -> String {
    if s.chars().count() > MAX_MSG {
        format!("{}...", s.chars().take(MAX_MSG).collect::<String>())
    } else {
        s.to_string()
    }
}

fn parse_secs(v: Option<&Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim_end_matches('s').parse().unwrap_or(0),
        _ => 0,
    }
}

fn flag(params: &Value, key: &str) -> bool {
    params.get(key).map_or(false, |v| v.as_bool().unwrap_or(true))
}

// ── commands ────────────────────────────────────────────────────────────────────────────────────

pub const HELP: &str = "mysid debug: record only the device log lines you care about, then read them back

Usage:
    mysid debug \"Buffer, router, usb\"   clear the device log, record lines whose tag or text contains any keyword
                                        (case-insensitive) into .mysid/debug.log for 20 s, then print
                                        \"20 sec log record done\" and the result - reproduce the issue meanwhile
    mysid debug usb --seconds 40        a different window: 10-60 s (also: -40)
    mysid debug error --raw 20          raw mode: capture the WHOLE device log (all buffers, system dump included)
                                        for 20 s, keep lines containing a term plus 3 lines above and below
    mysid debug                         show the last recording again (last 60 lines)
    mysid debug --level W               show warnings and errors only (V D I W E)
    mysid debug --lines 100             show up to 100 lines
    mysid debug status | stop | clear

The recorder stops by itself when the window ends. Follow the file live with: Get-Content .mysid\\debug.log -Wait
Set ANDROID_SERIAL to pick a device.
";

pub fn execute_debug(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let single = query.split_whitespace().count() == 1;
    let out = match (query.to_ascii_lowercase().as_str(), single) {
        ("help" | "-h" | "--help", _) => HELP.to_string(),
        ("stop", true) => cmd_stop(&ws),
        ("status", true) => status_line(&dir_of(&ws)),
        ("clear", true) => cmd_clear(&ws),
        _ if params.get("raw").map_or(false, |v| v.as_bool().unwrap_or(true)) => cmd_raw(&ws, &query, params),
        _ => cmd_record_or_show(&ws, &query, params),
    };
    json!({ "success": !out.starts_with("ERROR"), "output": out }).to_string()
}

const RAW_CONTEXT: usize = 3;
const RAW_MAX_LINES: usize = 400_000;

/// Streams `adb logcat` for `secs` seconds and returns every line (raw text, system dump included).
fn capture(adb: &str, args: &[&str], secs: u64) -> Result<Vec<String>, String> {
    let mut cmd = Command::new(adb);
    cmd.args(args).args(["-v", "threadtime"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run adb: {}", e))?;
    let out = child.stdout.take().ok_or("no adb output")?;
    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&lines);
    let reader = thread::spawn(move || {
        let mut r = BufReader::new(out);
        let mut raw = Vec::new();
        loop {
            raw.clear();
            match r.read_until(b'\n', &mut raw) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let mut l = sink.lock().unwrap();
                    if l.len() >= RAW_MAX_LINES {
                        break;
                    }
                    l.push(String::from_utf8_lossy(&raw).trim_end().to_string());
                }
            }
        }
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            break; // adb ended on its own (no device, bad option)
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    let taken = std::mem::take(&mut *lines.lock().unwrap());
    Ok(taken)
}

/// `mysid debug <terms> --raw [seconds]`: records the whole device log (all buffers, no tag filtering) for
/// 10-60 s, keeps every line containing a term plus 3 lines above and below, and saves that to debug.log.
fn cmd_raw(ws: &str, query: &str, params: &Value) -> String {
    let keywords = split_keywords(query);
    if keywords.is_empty() {
        return "ERROR: --raw needs a term to filter by, e.g. mysid debug error --raw 20".to_string();
    }
    let (asked, secs) = window_secs(params);
    let dir = dir_of(ws);
    // The keyword recorder writes the same file; raw mode replaces it.
    let old = read_state(&dir);
    if old.pid != 0 {
        kill_tree(old.pid);
        let _ = fs::remove_file(dir.join(STATE_FILE));
    }
    let adb = resolve_adb_binary();
    let _ = Command::new(&adb).args(["logcat", "-c"]).stdin(Stdio::null()).output();
    eprintln!("raw capture {}s, filter [{}] - reproduce the issue now...", secs, keywords.join(", "));

    let started = std::time::Instant::now();
    let mut lines = match capture(&adb, &["logcat", "-b", "all"], secs) {
        Ok(l) => l,
        Err(e) => return format!("ERROR: {}", e),
    };
    if lines.iter().all(|l| parse_line(l).is_none()) && started.elapsed() < Duration::from_secs(3) {
        // `-b all` is not accepted everywhere: retry with the default buffers.
        lines = match capture(&adb, &["logcat"], secs.saturating_sub(started.elapsed().as_secs()).max(WINDOW_MIN_SECS / 2)) {
            Ok(l) => l,
            Err(e) => return format!("ERROR: {}", e),
        };
    }
    let device_lines = lines.iter().filter(|l| parse_line(l).is_some()).count();
    if device_lines == 0 {
        return "ERROR: no log lines received - is a device connected and authorized? (mysid devices)".to_string();
    }

    let lowered: Vec<String> = keywords.iter().map(|k| k.to_lowercase()).collect();
    let mut hits = vec![0usize; lowered.len()];
    let mut is_hit = vec![false; lines.len()];
    for (i, l) in lines.iter().enumerate() {
        let low = l.to_lowercase();
        for (k, kw) in lowered.iter().enumerate() {
            if low.contains(kw.as_str()) {
                hits[k] += 1;
                is_hit[i] = true;
            }
        }
    }
    let blocks = context_blocks(&is_hit, RAW_CONTEXT);

    let mut file = String::new();
    let mut shown: Vec<String> = Vec::new();
    for (n, (a, b)) in blocks.iter().enumerate() {
        if n > 0 {
            file.push_str("# --\n");
            shown.push("--".to_string());
        }
        for i in *a..=*b {
            file.push_str(&lines[i]);
            file.push('\n');
            let mark = if is_hit[i] { '*' } else { ' ' };
            shown.push(match parse_line(&lines[i]) {
                Some(e) => format!("{} {} {} {}: {}", mark, e.time, e.level, e.tag, clip(&e.msg)),
                None => format!("{} {}", mark, clip(&lines[i])),
            });
        }
    }
    let _ = fs::write(dir.join(LOG_FILE), &file);
    let _ = fs::write(dir.join(JOB_FILE), serde_json::to_string(&Job { keywords: keywords.clone(), window_secs: secs, until_ms: 0 }).unwrap_or_default());
    let _ = fs::remove_file(dir.join(CURSOR_FILE));

    let total_hits = is_hit.iter().filter(|h| **h).count();
    let h: Vec<String> = keywords.iter().zip(&hits).map(|(k, n)| format!("{} {}", k, n)).collect();
    let window = if asked != secs { format!(" (asked {}s, allowed {}-{}s)", asked, WINDOW_MIN_SECS, WINDOW_MAX_SECS) } else { String::new() };
    let mut out = format!(
        "{} sec log record done{} (raw): {} device lines captured, {} matching line(s) in {} block(s); saved to {}/{} ({})\nhits: {}\n",
        secs,
        window,
        device_lines,
        total_hits,
        blocks.len(),
        build_log::MYSID_DIR,
        LOG_FILE,
        human(file.len() as u64),
        h.join(", ")
    );
    if total_hits == 0 {
        out.push_str("nothing matched: try another term, or a longer window.\n");
    }
    let max_lines = params.get("lines").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_LINES as u64).clamp(1, 1000) as usize;
    let skip = shown.len().saturating_sub(max_lines);
    if skip > 0 {
        out.push_str(&format!("... {} earlier line(s) in the file (--lines N to show more)\n", skip));
    }
    for l in shown.iter().skip(skip) {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str("(* = line containing a term)   read again: mysid debug");
    out
}

/// Index ranges covering every hit with `context` lines either side; overlapping or adjacent windows merge.
fn context_blocks(is_hit: &[bool], context: usize) -> Vec<(usize, usize)> {
    let mut blocks: Vec<(usize, usize)> = Vec::new();
    for (i, _) in is_hit.iter().enumerate().filter(|(_, h)| **h) {
        let (a, b) = (i.saturating_sub(context), (i + context).min(is_hit.len() - 1));
        match blocks.last_mut() {
            Some(last) if a <= last.1 + 1 => last.1 = last.1.max(b),
            _ => blocks.push((a, b)),
        }
    }
    blocks
}

fn cmd_stop(ws: &str) -> String {
    let dir = dir_of(ws);
    let s = read_state(&dir);
    if s.pid == 0 {
        return "Recorder is not running.".to_string();
    }
    kill_tree(s.pid);
    let _ = fs::remove_file(dir.join(STATE_FILE));
    format!("Recorder stopped. Log kept: {}/{} ({})", build_log::MYSID_DIR, LOG_FILE, human(size_of(&dir.join(LOG_FILE))))
}

fn cmd_clear(ws: &str) -> String {
    let dir = dir_of(ws);
    let _ = File::create(dir.join(LOG_FILE));
    let _ = fs::remove_file(dir.join(CURSOR_FILE));
    let _ = fs::remove_file(dir.join("debug.log.old"));
    "Recorded log cleared (the recorder, if running, keeps recording).".to_string()
}

fn status_line(dir: &Path) -> String {
    let s = read_state(dir);
    let kw = read_job(dir).keywords.join(", ");
    let state = if alive(&s) {
        format!("recording [{}] for {}, {} line(s) saved ({})", kw, fmt_secs(now_ms().saturating_sub(s.started_ms) / 1000), s.saved, s.note)
    } else if s.done {
        format!("{} sec log record done [{}], {} line(s) saved", read_job(dir).window_secs, kw, s.saved)
    } else {
        "recorder NOT running".to_string()
    };
    format!("{}; log {}/{} {}", state, build_log::MYSID_DIR, LOG_FILE, human(size_of(&dir.join(LOG_FILE))))
}

fn cmd_record_or_show(ws: &str, query: &str, params: &Value) -> String {
    let dir = dir_of(ws);
    let log_path = dir.join(LOG_FILE);
    let keywords = split_keywords(query);

    // Keywords start a fixed-length recording (default 20 s, 10-60) and wait for it to finish, so the
    // caller gets the result in one call. Without keywords the saved recording is shown again.
    let mut started_now = false;
    let mut window: Option<(u64, u64)> = None;
    if !keywords.is_empty() {
        let running = alive(&read_state(&dir));
        let (asked, secs) = window_secs(params);
        if !running || !same_keywords(&read_job(&dir).keywords, &keywords) {
            if let Err(e) = start_recorder(ws, &keywords, secs) {
                return format!("ERROR: {}", e);
            }
            started_now = true;
        }
        window = Some((asked, secs));
        eprintln!("recording {}s, filter [{}] - reproduce the issue now...", secs, keywords.join(", "));
        let limit = std::time::Instant::now() + Duration::from_secs(secs + 5);
        while std::time::Instant::now() < limit && alive(&read_state(&dir)) {
            thread::sleep(Duration::from_millis(200));
        }
    }
    touch(&dir);

    let max_lines = params.get("lines").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_LINES as u64).clamp(1, 1000) as usize;
    let min_level = params
        .get("level")
        .and_then(|v| v.as_str())
        .and_then(|s| s.chars().next())
        .map(|c| level_rank(c.to_ascii_uppercase()))
        .unwrap_or(0);
    let wait = parse_secs(params.get("wait"));

    // Where to read from: this call's start (--wait), the previous call's end (--new), or the tail.
    let cursor = fs::read_to_string(dir.join(CURSOR_FILE)).ok().and_then(|t| t.trim().parse::<u64>().ok()).unwrap_or(0);
    let mut from = 0;
    if wait > 0 && window.is_none() {
        from = size_of(&log_path);
        eprintln!("recording {}s - reproduce the issue now...", wait.min(120));
        thread::sleep(Duration::from_secs(wait.min(120)));
    } else if flag(params, "new") {
        from = if cursor <= size_of(&log_path) { cursor } else { 0 };
    }
    let (raw, end) = read_from(&log_path, from);
    let _ = fs::write(dir.join(CURSOR_FILE), end.to_string());

    let job = read_job(&dir);
    let lowered: Vec<String> = job.keywords.iter().map(|k| k.to_lowercase()).collect();
    let mut hits = vec![0usize; lowered.len()];
    let (mut warn, mut err) = (0, 0);
    let mut shown: Vec<(Entry, usize)> = Vec::new();
    for line in &raw {
        let Some(e) = parse_line(line) else { continue };
        let hay = format!("{} {}", e.tag, e.msg).to_lowercase();
        for (i, k) in lowered.iter().enumerate() {
            if hay.contains(k.as_str()) {
                hits[i] += 1;
            }
        }
        match e.level {
            'W' => warn += 1,
            'E' | 'F' => err += 1,
            _ => {}
        }
        if level_rank(e.level) < min_level {
            continue;
        }
        match shown.last_mut() {
            Some((p, n)) if p.tag == e.tag && p.msg == e.msg && p.level == e.level => *n += 1,
            _ => shown.push((e, 1)),
        }
    }
    let total = shown.len();
    let skip = total.saturating_sub(max_lines);

    let state = read_state(&dir);
    let mut out = if let (true, Some((asked, secs))) = (state.done, window) {
        let note = if asked != secs { format!(" (asked {}s, allowed {}-{}s)", asked, WINDOW_MIN_SECS, WINDOW_MAX_SECS) } else { String::new() };
        format!("{} sec log record done{} - {} line(s) saved to {}/{}\n", secs, note, state.saved, build_log::MYSID_DIR, LOG_FILE)
    } else {
        format!("DEBUG {}\n", status_line(&dir))
    };
    if started_now && state.saved == 0 && !state.note.is_empty() && state.note != "recording" {
        out.push_str(&format!("recorder: {}\n", state.note));
    }
    if window.is_none() && !state.done && !alive(&state) {
        out.push_str("no recording is running - showing what was recorded. Record again with: mysid debug \"keyword, keyword\"\n");
    }
    if !lowered.is_empty() && !raw.is_empty() {
        let h: Vec<String> = job.keywords.iter().zip(&hits).map(|(k, n)| format!("{} {}", k, n)).collect();
        out.push_str(&format!("hits: {} | {} error, {} warning\n", h.join(", "), err, warn));
    }
    if total == 0 && !started_now {
        out.push_str("no lines yet.\n");
    }
    if skip > 0 {
        out.push_str(&format!("... {} earlier line(s) not shown (--lines N, --level W, or the file)\n", skip));
    }
    for (e, n) in shown.iter().skip(skip) {
        out.push_str(&format!("{} {} {}: {}", e.time, e.level, e.tag, clip(&e.msg)));
        if *n > 1 {
            out.push_str(&format!("  (x{})", n));
        }
        out.push('\n');
    }
    out.push_str(&format!("log: {}/{}   again: mysid debug (show) | mysid debug \"kw, kw\" (record a new window)", build_log::MYSID_DIR, LOG_FILE));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_threadtime_lines() {
        let e = parse_line("10-08 12:34:56.789  1234  5678 D UsbBulkEngine: [USB] opened fd 7").unwrap();
        assert_eq!((e.time.as_str(), e.level, e.tag.as_str(), e.msg.as_str()), ("12:34:56.789", 'D', "UsbBulkEngine", "[USB] opened fd 7"));
        assert!(parse_line("--------- beginning of main").is_none());
    }

    #[test]
    fn hits_get_three_lines_of_context_and_close_windows_merge() {
        let mut hit = vec![false; 30];
        hit[10] = true;
        hit[15] = true; // windows 7..13 and 12..18 overlap
        hit[28] = true; // window 25..29 (clamped at the end)
        hit[1] = true; // window 0..4 (clamped at the start)
        assert_eq!(context_blocks(&hit, 3), vec![(0, 4), (7, 18), (25, 29)]);
        assert!(context_blocks(&[false; 5], 3).is_empty());
    }

    #[test]
    fn single_dash_spellings_and_seconds_are_clamped() {
        assert_eq!(parse_secs(Some(&json!(20))), 20);
        assert_eq!(parse_secs(Some(&json!("20s"))), 20);
        assert_eq!(window_secs(&json!({})), (20, 20), "default is 20 s");
        assert_eq!(window_secs(&json!({ "seconds": 5 })), (5, 10));
        assert_eq!(window_secs(&json!({ "raw": 300 })), (300, 60));
        assert_eq!(window_secs(&json!({ "raw": true })), (20, 20));
    }

    #[test]
    fn keywords_split_on_commas_or_spaces_and_match_tag_or_text() {
        assert_eq!(split_keywords("Buffer, router, usb, BT, decoder"), vec!["Buffer", "router", "usb", "BT", "decoder"]);
        assert_eq!(split_keywords("audio track, usb"), vec!["audio track", "usb"]);
        assert_eq!(split_keywords("usb router"), vec!["usb", "router"]);
        assert!(same_keywords(&["USB".into(), "bt".into()], &["bt".into(), "usb".into()]));
        let kw = vec!["usb".to_string(), "buffer".to_string()];
        let tag_hit = parse_line("10-08 12:00:00.001  10  11 D UsbEngine: opened").unwrap();
        let text_hit = parse_line("10-08 12:00:00.001  10  11 D Audio: Buffer underrun").unwrap();
        let miss = parse_line("10-08 12:00:00.001  10  11 D Audio: hello").unwrap();
        assert!(matches_any(&tag_hit, &kw) && matches_any(&text_hit, &kw) && !matches_any(&miss, &kw));
        assert!(matches_any(&miss, &[]));
    }

    #[test]
    fn shows_the_recorded_file_without_a_device() {
        let dir = std::env::temp_dir().join(format!("mysid_debug_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".mysid")).unwrap();
        let log = [
            "10-08 12:00:00.001  10  11 D AudioRouter: route changed to usb",
            "10-08 12:00:00.002  10  11 D AudioRouter: route changed to usb",
            "10-08 12:00:00.003  10  11 E UsbEngine: Buffer underrun at 44100",
        ];
        let d = dir.join(".mysid");
        fs::write(d.join(LOG_FILE), log.join("\n") + "\n").unwrap();
        fs::write(d.join(JOB_FILE), r#"{"keywords":["usb","buffer"]}"#).unwrap();
        write_state(&d, &State { pid: 1, started_ms: now_ms(), updated_ms: now_ms(), saved: 3, note: "recording".into(), done: false });
        let out = execute_debug(&json!({ "workspace_root": dir.to_string_lossy() }), None);
        let text = serde_json::from_str::<Value>(&out).unwrap()["output"].as_str().unwrap().to_string();
        let warn = execute_debug(&json!({ "workspace_root": dir.to_string_lossy(), "level": "W" }), None);
        let _ = fs::remove_dir_all(&dir);
        assert!(text.contains("(x2)") && text.contains("E UsbEngine: Buffer underrun"), "{}", text);
        assert!(text.contains("hits: usb 3, buffer 1 | 1 error, 0 warning"), "{}", text);
        assert!(!warn.contains("AudioRouter"), "level filter: {}", warn);
    }
}
