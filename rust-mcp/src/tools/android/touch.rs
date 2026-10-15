//! Touch automation: every action on the device ends with a screenshot, so an agent drives the UI in one call per step.
//!
//! `tap swipe longpress type keyevent screenshot` perform an action, wait for the screen to settle, and return the
//! screen as a PNG (image content block over MCP, `.mysid/shots/<ms>.png` on the command line) plus one line that maps
//! image pixels back to device pixels. `ui_dump` lists the visible elements; `tap --text/--desc/--id` finds an element
//! itself. `batch` runs a list of steps in one call. Every adb call has a 10 s timeout.

use regex::Regex;
use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::adb::resolve_adb_binary;
use super::shot::{self, Image};

use super::{focus, scenario};

pub(super) const ADB_TIMEOUT: Duration = Duration::from_secs(10);
pub(super) const DEFAULT_WAIT_MS: u64 = 600;
pub(super) const MAX_SIDE: f64 = 2000.0;
pub(super) const KEEP_SHOTS: usize = 20;

pub fn handles(action: &str) -> bool {
    matches!(
        action,
        "tap" | "swipe" | "longpress" | "long_press" | "scroll" | "type" | "keyevent" | "key" | "screenshot" | "shot" | "ui_dump" | "uidump" | "batch" | "wait" | "focus" | "goto" | "scenario"
    )
}

// ---------------------------------------------------------------- parameters

pub(super) fn get<'a>(p: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| p.get(*k)).filter(|v| !v.is_null())
}

pub(super) fn num(p: &Value, keys: &[&str]) -> Option<f64> {
    match get(p, keys)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub(super) fn text(p: &Value, keys: &[&str]) -> Option<String> {
    match get(p, keys)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

pub(super) fn flag(p: &Value, key: &str) -> bool {
    match p.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"),
        Some(Value::Number(n)) => n.as_f64().map_or(false, |n| n != 0.0),
        _ => false,
    }
}

/// Bare words after the action (`tap 540 1200`, `type hello`, `keyevent back`).
pub(super) fn args_of(p: &Value) -> Vec<String> {
    p.get("args").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()
}

pub(super) fn err(msg: impl AsRef<str>) -> String {
    format!("ERROR: {}", msg.as_ref())
}

// ---------------------------------------------------------------- adb

pub(super) struct AdbOut {
    pub(super) code: i32,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: String,
}

pub(super) fn adb(serial: Option<&str>, args: &[&str], timeout: Duration) -> Result<AdbOut, String> {
    let bin = resolve_adb_binary();
    let mut cmd = Command::new(&bin);
    if let Some(s) = serial {
        cmd.args(["-s", s]);
    }
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run {}: {}", bin, e))?;
    let (mut so, mut se) = (child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let t_out = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = thread::spawn(move || {
        let mut b = Vec::new();
        let _ = se.read_to_end(&mut b);
        b
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "adb {} timed out after {}s (device asleep, locked, or the USB/Wi-Fi link dropped?)",
                    args.join(" "),
                    timeout.as_secs()
                ));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok(AdbOut {
        code: status.code().unwrap_or(-1),
        stdout: t_out.join().unwrap_or_default(),
        stderr: String::from_utf8_lossy(&t_err.join().unwrap_or_default()).trim().to_string(),
    })
}

pub(super) fn pick_device(p: &Value) -> Result<String, String> {
    let o = adb(None, &["devices", "-l"], ADB_TIMEOUT)?;
    let listing = String::from_utf8_lossy(&o.stdout).to_string();
    let mut ready: Vec<(String, String)> = Vec::new();
    let mut other: Vec<String> = Vec::new();
    for line in listing.lines().skip(1).map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('*')) {
        let mut it = line.split_whitespace();
        let (serial, state) = (it.next().unwrap_or(""), it.next().unwrap_or(""));
        let model = line.split_whitespace().find_map(|w| w.strip_prefix("model:")).unwrap_or("").to_string();
        if state == "device" {
            ready.push((serial.to_string(), model));
        } else {
            other.push(format!("{} ({})", serial, state));
        }
    }
    let show = |v: &[(String, String)]| v.iter().map(|(s, m)| format!("  {}  {}", s, m)).collect::<Vec<_>>().join("\n");
    if let Some(want) = text(p, &["device", "serial"]) {
        return ready
            .iter()
            .find(|(s, _)| *s == want)
            .map(|(s, _)| s.clone())
            .ok_or_else(|| format!("device '{}' is not connected and ready. Ready devices:\n{}", want, if ready.is_empty() { "  (none)".to_string() } else { show(&ready) }));
    }
    match ready.len() {
        1 => Ok(ready[0].0.clone()),
        0 => Err(format!(
            "no Android device connected{}. Check the cable / `mysid wifi`, and accept the USB debugging prompt.",
            if other.is_empty() { String::new() } else { format!(" (seen: {})", other.join(", ")) }
        )),
        _ => Err(format!("{} devices are connected; pass --device <serial>:\n{}", ready.len(), show(&ready))),
    }
}

// ---------------------------------------------------------------- context

pub(super) struct Ctx {
    pub(super) serial: String,
    pub(super) ws: PathBuf,
    /// Screen size in the current orientation, device pixels (what `input tap` uses).
    pub(super) dev_w: i64,
    pub(super) dev_h: i64,
}

pub(super) fn screen_size(serial: &str) -> Result<(i64, i64), String> {
    let o = adb(Some(serial), &["shell", "wm", "size"], ADB_TIMEOUT)?;
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    let re = Regex::new(r"(Override|Physical) size:\s*(\d+)x(\d+)").unwrap();
    let mut best: Option<(i64, i64)> = None;
    for c in re.captures_iter(&t) {
        let wh = (c[2].parse().unwrap_or(0), c[3].parse().unwrap_or(0));
        if &c[1] == "Override" || best.is_none() {
            best = Some(wh);
        }
    }
    let (mut w, mut h) = best.ok_or_else(|| format!("cannot read the screen size (`wm size` said: {})", t.trim()))?;
    // `wm size` is the natural orientation; a rotated screen swaps the axes.
    if let Ok(o) = adb(Some(serial), &["shell", "dumpsys", "window", "displays"], ADB_TIMEOUT) {
        let d = String::from_utf8_lossy(&o.stdout).to_string();
        if let Some(c) = Regex::new(r"mDisplayRotation=ROTATION_(\d+)").unwrap().captures(&d) {
            if matches!(&c[1], "90" | "270") {
                std::mem::swap(&mut w, &mut h);
            }
        }
    }
    Ok((w, h))
}

pub(super) fn context(p: &Value, ws: Option<&Path>) -> Result<Ctx, String> {
    let serial = pick_device(p)?;
    let (dev_w, dev_h) = screen_size(&serial)?;
    let ws = ws.map(Path::to_path_buf).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    Ok(Ctx { serial, ws, dev_w, dev_h })
}

/// Image pixels per device pixel for the returned image.
pub(super) fn image_scale(ctx_w: i64, ctx_h: i64, p: &Value) -> f64 {
    if let Some(s) = num(p, &["scale"]) {
        return s.clamp(0.1, 1.0);
    }
    (MAX_SIDE / ctx_w.max(ctx_h).max(1) as f64).min(1.0)
}

// ---------------------------------------------------------------- screenshot

pub(super) struct Shot {
    pub(super) png: Vec<u8>,
    pub(super) dev_w: usize,
    pub(super) dev_h: usize,
    pub(super) img_w: usize,
    pub(super) img_h: usize,
    /// Device pixels per image pixel.
    pub(super) factor: f64,
    /// What is on screen (see `shot::fingerprint`) and the foreground app.
    pub(super) hash: u64,
    pub(super) app: String,
}

/// The package that has the input focus, from `dumpsys window`.
pub(super) fn foreground(ctx: &Ctx) -> Option<String> {
    let o = adb(Some(&ctx.serial), &["shell", "dumpsys", "window"], ADB_TIMEOUT).ok()?;
    let t = String::from_utf8_lossy(&o.stdout).to_string();
    Regex::new(r"mCurrentFocus=Window\{[^}]*? ([\w.]+)/[\w.$]+\}")
        .unwrap()
        .captures(&t)
        .or_else(|| Regex::new(r"mFocusedApp=.*? ([\w.]+)/").unwrap().captures(&t))
        .map(|c| c[1].to_string())
}

/// `--app <package>`: refuse to act when another app is in front (a mistaken tap lands in whatever is there).
pub(super) fn guard(ctx: &Ctx, p: &Value) -> Result<(), String> {
    let Some(want) = text(p, &["app"]) else { return Ok(()) };
    match foreground(ctx) {
        Some(fg) if fg == want => Ok(()),
        Some(fg) => Err(format!("the foreground app is {}, not {}; nothing was done. Start it: mysid android launch_app {}", fg, want, want)),
        None => Err(format!("cannot tell which app is in the foreground (wanted {}); nothing was done", want)),
    }
}

/// Why the screen is black: off, locked, or a protected window.
pub(super) fn why_black(ctx: &Ctx) -> String {
    let power = adb(Some(&ctx.serial), &["shell", "dumpsys", "power"], ADB_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    if let Some(c) = Regex::new(r"mWakefulness=(\w+)").unwrap().captures(&power) {
        if &c[1] != "Awake" {
            return format!("the screen is off ({}). Wake it: mysid android keyevent wakeup", &c[1]);
        }
    }
    let win = adb(Some(&ctx.serial), &["shell", "dumpsys", "window"], ADB_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    if win.contains("mShowingLockscreen=true") || win.contains("isKeyguardShowing=true") || win.contains("mKeyguardShowing=true") {
        return "the phone is locked: unlock it by hand, then retry".to_string();
    }
    "the window is protected (FLAG_SECURE): the screenshot is black".to_string()
}

/// Fingerprint of the screen right now (None when it cannot be captured).
pub(super) fn screen_hash(ctx: &Ctx) -> Option<u64> {
    let o = adb(Some(&ctx.serial), &["exec-out", "screencap", "-p"], ADB_TIMEOUT).ok()?;
    shot::is_png(&o.stdout).then(|| shot::decode(&o.stdout).ok()).flatten().map(|i| shot::fingerprint(&i))
}

pub(super) fn capture(ctx: &Ctx, p: &Value, mark: Option<(i64, i64)>) -> Result<Shot, String> {
    let mut image: Option<Image> = None;
    for attempt in 0..2 {
        let o = adb(Some(&ctx.serial), &["exec-out", "screencap", "-p"], ADB_TIMEOUT)?;
        if !shot::is_png(&o.stdout) {
            return Err(format!("screencap returned no PNG ({} bytes){}", o.stdout.len(), if o.stderr.is_empty() { String::new() } else { format!(": {}", o.stderr) }));
        }
        let img = shot::decode(&o.stdout)?;
        if shot::is_blank(&img) {
            if attempt == 0 {
                thread::sleep(Duration::from_millis(400));
                continue;
            }
            return Err(why_black(ctx));
        }
        image = Some(img);
        break;
    }
    let img = image.ok_or("screenshot failed")?;
    let hash = shot::fingerprint(&img);
    let sc = image_scale(img.w as i64, img.h as i64, p);
    let (nw, nh) = (((img.w as f64 * sc).round() as usize).max(1), ((img.h as f64 * sc).round() as usize).max(1));
    let mut small = shot::resize(&img, nw, nh);
    let factor = img.w as f64 / small.w as f64;
    if flag(p, "grid") {
        shot::overlay(&mut small, img.w as i64, img.h as i64, factor, mark);
    }
    let app = foreground(ctx).unwrap_or_else(|| "?".to_string());
    Ok(Shot { png: shot::encode(&small)?, dev_w: img.w, dev_h: img.h, img_w: small.w, img_h: small.h, factor, hash, app })
}

pub(super) fn now_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)
}

pub(super) fn log_touch(ctx: &Ctx, line: &str) {
    if let Ok(dir) = crate::tools::build::log::ensure_dir(&ctx.ws) {
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("touch.log")) {
            let _ = writeln!(f, "{} {}", now_ms(), line);
        }
    }
}

/// Saves the shot (last 20 kept), queues it for MCP, logs it, and returns the one-line mapping.
pub(super) fn publish(ctx: &Ctx, shot: &Shot, head: &str) -> String {
    let mut saved = String::new();
    if let Ok(dir) = crate::tools::build::log::ensure_dir(&ctx.ws) {
        let shots = dir.join("shots");
        let _ = fs::create_dir_all(&shots);
        let path = shots.join(format!("{}.png", now_ms()));
        if fs::write(&path, &shot.png).is_ok() {
            saved = path.to_string_lossy().to_string();
            let mut all: Vec<PathBuf> = fs::read_dir(&shots).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.extension().map_or(false, |x| x == "png")).collect()).unwrap_or_default();
            all.sort();
            let extra = all.len().saturating_sub(KEEP_SHOTS);
            for old in all.into_iter().take(extra) {
                let _ = fs::remove_file(old);
            }
        }
    }
    if shot::is_mcp() {
        shot::queue_image(shot.png.clone());
    }
    log_touch(ctx, &format!("{} -> {}", head, if saved.is_empty() { "(not saved)" } else { &saved }));
    let mut line = format!(
        "{} | device {} | app {} | screen {}x{} | image {}x{} | scale {:.3} (image px x {:.3} = device px)",
        head, ctx.serial, shot.app, shot.dev_w, shot.dev_h, shot.img_w, shot.img_h, shot.factor, shot.factor
    );
    if !saved.is_empty() {
        line.push_str(&format!("\nsaved: {}", saved));
    }
    line.push_str(&format!("\nworkspace: {}", ctx.ws.display()));
    line
}

// ---------------------------------------------------------------- elements

#[derive(Clone)]
pub(super) struct Node {
    pub(super) text: String,
    pub(super) desc: String,
    pub(super) id: String,
    pub(super) clickable: bool,
    pub(super) b: (i64, i64, i64, i64),
}

impl Node {
    fn cx(&self) -> i64 {
        (self.b.0 + self.b.2) / 2
    }
    fn cy(&self) -> i64 {
        (self.b.1 + self.b.3) / 2
    }
    fn short_id(&self) -> &str {
        self.id.rsplit('/').next().unwrap_or("")
    }
    fn line(&self, i: usize) -> String {
        let cut = |s: &str| -> String {
            let s = s.replace('\n', " ");
            if s.is_empty() { "-".to_string() } else { s.chars().take(40).collect() }
        };
        format!("[{}] {} | {} | {} | {} | ({},{})", i, cut(&self.text), cut(&self.desc), cut(self.short_id()), if self.clickable { "clickable" } else { "-" }, self.cx(), self.cy())
    }
}

pub(super) fn unescape(s: &str) -> String {
    s.replace("&#10;", "\n").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

pub(super) fn dump_xml(ctx: &Ctx) -> Result<String, String> {
    let grab = |o: &AdbOut| {
        let s = String::from_utf8_lossy(&o.stdout).to_string();
        s.find("<?xml").or_else(|| s.find("<hierarchy")).map(|i| s[i..].to_string()).filter(|x| x.contains("<node"))
    };
    // Two uiautomator runs at once fail with "UiAutomation already registered": one at a time in this process,
    // and one retry when another process (an earlier dump still in flight) holds it.
    static DUMP_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DUMP_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for attempt in 0..2 {
        if let Ok(o) = adb(Some(&ctx.serial), &["exec-out", "uiautomator", "dump", "/dev/tty"], ADB_TIMEOUT) {
            if let Some(x) = grab(&o) {
                return Ok(x);
            }
            let said = format!("{}{}", String::from_utf8_lossy(&o.stdout), o.stderr);
            if attempt == 0 && said.contains("already registered") {
                thread::sleep(Duration::from_millis(1200));
                continue;
            }
        }
        break;
    }
    // Some devices cannot dump to the terminal: dump to a file on the device and read it back.
    let o = adb(Some(&ctx.serial), &["shell", "uiautomator", "dump", "/sdcard/mysid_ui.xml"], ADB_TIMEOUT)?;
    let r = adb(Some(&ctx.serial), &["exec-out", "cat", "/sdcard/mysid_ui.xml"], ADB_TIMEOUT)?;
    let _ = adb(Some(&ctx.serial), &["shell", "rm", "-f", "/sdcard/mysid_ui.xml"], ADB_TIMEOUT);
    grab(&r).ok_or_else(|| {
        format!(
            "uiautomator returned no hierarchy ({}). A screen that blocks accessibility (secure window, lock screen) cannot be read.",
            String::from_utf8_lossy(&o.stdout).trim()
        )
    })
}

/// Visible nodes that are clickable or carry text / a content description, in screen order.
pub(super) fn ui_nodes(ctx: &Ctx) -> Result<Vec<Node>, String> {
    let xml = dump_xml(ctx)?;
    let node_re = Regex::new(r#"<node\s((?:[^>"]|"[^"]*")*?)/?>"#).unwrap();
    let attr_re = Regex::new(r#"([\w:-]+)="([^"]*)""#).unwrap();
    let bounds_re = Regex::new(r"\[(-?\d+),(-?\d+)\]\[(-?\d+),(-?\d+)\]").unwrap();
    let mut out = Vec::new();
    for c in node_re.captures_iter(&xml) {
        let attrs: std::collections::HashMap<String, String> = attr_re.captures_iter(&c[1]).map(|a| (a[1].to_string(), unescape(&a[2]))).collect();
        let a = |k: &str| attrs.get(k).cloned().unwrap_or_default();
        let bounds = a("bounds");
        let Some(b) = bounds_re.captures(&bounds) else { continue };
        let b = (b[1].parse().unwrap_or(0), b[2].parse().unwrap_or(0), b[3].parse().unwrap_or(0), b[4].parse().unwrap_or(0));
        if b.2 <= b.0 || b.3 <= b.1 || a("visible-to-user") == "false" {
            continue;
        }
        let n = Node { text: a("text"), desc: a("content-desc"), id: a("resource-id"), clickable: a("clickable") == "true", b };
        if n.clickable || !n.text.is_empty() || !n.desc.is_empty() {
            out.push(n);
        }
    }
    Ok(out)
}

pub(super) fn eq_or_contains(nodes: &[Node], q: &str, field: impl Fn(&Node) -> String) -> Vec<Node> {
    let q = q.to_lowercase();
    let exact: Vec<Node> = nodes.iter().filter(|n| field(n).to_lowercase() == q).cloned().collect();
    if !exact.is_empty() {
        return exact;
    }
    nodes.iter().filter(|n| field(n).to_lowercase().contains(&q)).cloned().collect()
}

/// The one element a text / desc / id target names. Several candidates: none is chosen unless `--index` says which.
pub(super) fn find_element(ctx: &Ctx, p: &Value) -> Result<(Node, String), String> {
    let (t, d, id) = (text(p, &["text"]), text(p, &["desc", "description", "content_desc"]), text(p, &["id", "resource_id"]));
    let nodes = ui_nodes(ctx)?;
    let mut found: Vec<Node> = nodes.clone();
    let mut what = Vec::new();
    if let Some(q) = &t {
        found = eq_or_contains(&found, q, |n| n.text.clone());
        what.push(format!("text \"{}\"", q));
    }
    if let Some(q) = &d {
        found = eq_or_contains(&found, q, |n| n.desc.clone());
        what.push(format!("desc \"{}\"", q));
    }
    if let Some(q) = &id {
        found.retain(|n| n.id == *q || n.id.ends_with(&format!("/{}", q)));
        what.push(format!("id \"{}\"", q));
    }
    // The same element often appears twice (a clickable parent and its text child): keep the first of each cluster.
    let mut unique: Vec<Node> = Vec::new();
    for n in found {
        if !unique.iter().any(|u| (u.cx() - n.cx()).abs() <= 12 && (u.cy() - n.cy()).abs() <= 12) {
            unique.push(n);
        }
    }
    let what = what.join(" + ");
    if unique.is_empty() {
        let seen: Vec<String> = nodes.iter().filter(|n| !n.text.is_empty() || !n.desc.is_empty()).take(12).map(|n| if n.text.is_empty() { n.desc.clone() } else { n.text.clone() }).collect();
        return Err(format!("no element matches {}. On screen: {}. (`mysid android ui_dump` lists everything.)", what, seen.join(" | ")));
    }
    let idx = num(p, &["index"]).map(|i| i as usize);
    match (unique.len(), idx) {
        (_, Some(i)) if i < unique.len() => Ok((unique[i].clone(), what)),
        (_, Some(i)) => Err(format!("--index {} but only {} element(s) match {}", i, unique.len(), what)),
        (1, None) => Ok((unique[0].clone(), what)),
        (n, None) => Err(format!(
            "{} elements match {}; tapped none. Re-run with --index <n>:\n{}",
            n,
            what,
            unique.iter().take(12).enumerate().map(|(i, n)| n.line(i)).collect::<Vec<_>>().join("\n")
        )),
    }
}

// ---------------------------------------------------------------- actions

pub(super) struct Done {
    pub(super) desc: String,
    pub(super) mark: Option<(i64, i64)>,
}

pub(super) fn has_target(p: &Value) -> bool {
    get(p, &["text", "desc", "description", "content_desc", "id", "resource_id"]).is_some()
}

pub(super) fn check_range(ctx: &Ctx, what: &str, x: i64, y: i64) -> Result<(), String> {
    if x < 0 || y < 0 || x >= ctx.dev_w || y >= ctx.dev_h {
        return Err(format!(
            "{}({},{}) is outside the screen {}x{} (device pixels); nothing was tapped. Coordinates are device pixels; use --space image for the returned image's pixels.",
            what, x, y, ctx.dev_w, ctx.dev_h
        ));
    }
    Ok(())
}

/// A device-pixel coordinate from a value in device or image pixels (`--space image`).
pub(super) fn to_device(ctx: &Ctx, p: &Value, v: f64) -> i64 {
    if text(p, &["space"]).map_or(false, |s| s.eq_ignore_ascii_case("image")) {
        (v / image_scale(ctx.dev_w, ctx.dev_h, p)).round() as i64
    } else {
        v.round() as i64
    }
}

pub(super) fn point(ctx: &Ctx, p: &Value, what: &str) -> Result<(i64, i64, String), String> {
    if has_target(p) && num(p, &["x"]).is_none() {
        let (n, how) = find_element(ctx, p)?;
        return Ok((n.cx(), n.cy(), format!(" on {}", how)));
    }
    let bare: Vec<f64> = args_of(p).iter().filter_map(|a| a.parse().ok()).collect();
    let (x, y) = match (num(p, &["x"]), num(p, &["y"])) {
        (Some(x), Some(y)) => (x, y),
        _ if bare.len() >= 2 => (bare[0], bare[1]),
        _ => return Err(format!("{} needs --x and --y (device pixels), or --text / --desc / --id to find an element", what)),
    };
    let (x, y) = (to_device(ctx, p, x), to_device(ctx, p, y));
    check_range(ctx, what, x, y)?;
    Ok((x, y, String::new()))
}

pub(super) fn shell_input(ctx: &Ctx, args: &[&str]) -> Result<(), String> {
    let mut full = vec!["shell", "input"];
    full.extend_from_slice(args);
    let o = adb(Some(&ctx.serial), &full, ADB_TIMEOUT)?;
    if o.code != 0 || o.stderr.to_lowercase().contains("error") {
        return Err(format!("input {} failed: {}", args.join(" "), if o.stderr.is_empty() { String::from_utf8_lossy(&o.stdout).trim().to_string() } else { o.stderr }));
    }
    Ok(())
}

pub(super) fn keycode(raw: &str) -> String {
    let r = raw.trim();
    if r.chars().all(|c| c.is_ascii_digit()) && !r.is_empty() {
        return r.to_string();
    }
    let up = r.to_uppercase().replace(['-', ' '], "_");
    let up = match up.as_str() {
        "RECENT" | "RECENTS" | "OVERVIEW" => "APP_SWITCH".to_string(),
        "ENTER" | "RETURN" => "ENTER".to_string(),
        other => other.to_string(),
    };
    if up.starts_with("KEYCODE_") { up } else { format!("KEYCODE_{}", up) }
}

/// `input text` goes through the device shell: spaces become %s and shell characters are escaped.
pub(super) fn escape_text(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            ' ' => out.push_str("%s"),
            '\\' | '\'' | '"' | '`' | '&' | '|' | ';' | '<' | '>' | '(' | ')' | '$' | '*' | '?' | '!' | '~' | '#' | '{' | '}' | '[' | ']' | '^' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

pub(super) fn act(ctx: &Ctx, action: &str, p: &Value) -> Result<Done, String> {
    match action {
        "tap" => {
            let (x, y, how) = point(ctx, p, "tap")?;
            shell_input(ctx, &["tap", &x.to_string(), &y.to_string()])?;
            Ok(Done { desc: format!("tap({},{}){}", x, y, how), mark: Some((x, y)) })
        }
        "longpress" | "long_press" => {
            let (x, y, how) = point(ctx, p, "longpress")?;
            let ms = num(p, &["duration"]).unwrap_or(800.0).max(100.0) as u64;
            shell_input(ctx, &["swipe", &x.to_string(), &y.to_string(), &x.to_string(), &y.to_string(), &ms.to_string()])?;
            Ok(Done { desc: format!("longpress({},{},{}ms){}", x, y, ms, how), mark: Some((x, y)) })
        }
        "swipe" => {
            let bare: Vec<f64> = args_of(p).iter().filter_map(|a| a.parse().ok()).collect();
            let pick = |k: &str, i: usize| num(p, &[k]).or_else(|| bare.get(i).copied());
            let (Some(x1), Some(y1), Some(x2), Some(y2)) = (pick("x1", 0), pick("y1", 1), pick("x2", 2), pick("y2", 3)) else {
                return Err("swipe needs --x1 --y1 --x2 --y2 (device pixels; --space image for image pixels)".to_string());
            };
            let (x1, y1, x2, y2) = (to_device(ctx, p, x1), to_device(ctx, p, y1), to_device(ctx, p, x2), to_device(ctx, p, y2));
            check_range(ctx, "swipe start", x1, y1)?;
            check_range(ctx, "swipe end", x2, y2)?;
            let ms = num(p, &["duration"]).unwrap_or(300.0).max(50.0) as u64;
            shell_input(ctx, &["swipe", &x1.to_string(), &y1.to_string(), &x2.to_string(), &y2.to_string(), &ms.to_string()])?;
            Ok(Done { desc: format!("swipe({},{}->{},{},{}ms)", x1, y1, x2, y2, ms), mark: Some((x2, y2)) })
        }
        "type" => {
            let t = text(p, &["text"]).or_else(|| Some(args_of(p).join(" ")).filter(|s| !s.is_empty())).ok_or("type needs --text <text>")?;
            if !t.is_ascii() {
                return Err("`input text` types ASCII only; for other characters use keyevents or paste through the app".to_string());
            }
            for (i, line) in t.split('\n').enumerate() {
                if i > 0 {
                    shell_input(ctx, &["keyevent", "KEYCODE_ENTER"])?;
                }
                if !line.is_empty() {
                    shell_input(ctx, &["text", &escape_text(line)])?;
                }
            }
            Ok(Done { desc: format!("type({} chars)", t.chars().count()), mark: None })
        }
        "keyevent" | "key" => {
            // `--fast` (and `--gap ms`): every press in ONE adb call. A call costs ~1.7 s on a TV, so 20 presses one by one take half a minute.
            // `--keys "dpad_right dpad_right enter"` is a sequence; `--times n` repeats the whole sequence.
            let sequence: Option<Vec<String>> = text(p, &["keys", "sequence"]).map(|s| s.split(|c: char| c.is_whitespace() || c == ',').filter(|w| !w.is_empty()).map(keycode).collect());
            if sequence.is_some() || flag(p, "fast") || num(p, &["gap"]).is_some() {
                let one: Vec<String> = sequence.unwrap_or_else(|| vec![keycode(&text(p, &["keycode", "key"]).or_else(|| args_of(p).first().cloned()).unwrap_or_else(|| "KEYCODE_BACK".to_string()))]);
                let asked = num(p, &["times", "count", "repeat"]).map_or(1, |n| n.max(1.0) as usize);
                let times = asked.min((200 / one.len().max(1)).max(1));
                let all: Vec<String> = (0..times).flat_map(|_| one.clone()).collect();
                if let Some(gap) = num(p, &["gap"]) {
                    let gap = gap.max(0.0) as u64;
                    let script = format!("for k in {}; do input keyevent $k; sleep {}.{:03}; done", all.join(" "), gap / 1000, gap % 1000);
                    let o = adb(Some(&ctx.serial), &["shell", &script], ADB_TIMEOUT + Duration::from_millis(gap * all.len() as u64 + 400 * all.len() as u64))?;
                    if o.code != 0 {
                        return Err(format!("keyevents failed: {}", o.stderr));
                    }
                } else {
                    let mut args: Vec<&str> = vec!["keyevent"];
                    args.extend(all.iter().map(|s| s.as_str()));
                    shell_input(ctx, &args)?;
                }
                let names: Vec<String> = one.iter().map(|c| c.trim_start_matches("KEYCODE_").to_lowercase()).collect();
                let capped = if asked > times { format!(", capped from {}: at most 200 presses per call", asked) } else { String::new() };
                return Ok(Done { desc: format!("keys({}{} = {} presses, fast{})", names.join(" "), if times > 1 { format!(" x{}", times) } else { String::new() }, all.len(), capped), mark: None });
            }
            let k = text(p, &["keycode", "key"]).or_else(|| args_of(p).first().cloned()).unwrap_or_else(|| "KEYCODE_BACK".to_string());
            let code = keycode(&k);
            // `--times 2`: one back often only closes the keyboard or a menu.
            let asked = num(p, &["times", "count", "repeat"]).map_or(1, |n| n.max(1.0) as usize);
            let times = asked.min(10);
            for i in 0..times {
                if i > 0 {
                    thread::sleep(Duration::from_millis(250));
                }
                shell_input(ctx, &["keyevent", &code])?;
            }
            let capped = if asked > times { format!(", capped from {}: at most 10 presses per call", asked) } else { String::new() };
            Ok(Done { desc: if times > 1 { format!("keyevent({} x{}{})", code, times, capped) } else { format!("keyevent({})", code) }, mark: None })
        }
        "scroll" => scroll(ctx, p),
        "screenshot" | "shot" => Ok(Done { desc: "screenshot".to_string(), mark: None }),
        // Text-only steps (also usable inside a batch): the answer is the step's description.
        "focus" => focus::report(ctx, p).map(|t| Done { desc: t, mark: None }),
        "goto" => focus::goto(ctx, p).map(|t| Done { desc: t, mark: None }),
        "wait" => {
            let ms = num(p, &["ms", "wait", "duration"]).unwrap_or(500.0).max(0.0) as u64;
            thread::sleep(Duration::from_millis(ms.min(30_000)));
            Ok(Done { desc: format!("wait({}ms)", ms), mark: None })
        }
        other => Err(format!("unknown touch action '{}'", other)),
    }
}

// ---------------------------------------------------------------- entry points

pub(super) fn has_label(nodes: &[Node], q: &str) -> bool {
    let q = q.to_lowercase();
    nodes.iter().any(|n| n.text.to_lowercase().contains(&q) || n.desc.to_lowercase().contains(&q))
}

/// `scroll down|up|left|right` (the direction the content moves into view) or `scroll [down] to "text"`.
pub(super) fn scroll(ctx: &Ctx, p: &Value) -> Result<Done, String> {
    let a = args_of(p);
    let to = text(p, &["to"]).or_else(|| {
        let i = a.iter().position(|w| w.eq_ignore_ascii_case("to"))?;
        Some(a[i + 1..].join(" ")).filter(|s| !s.is_empty())
    });
    let first = a.first().cloned().filter(|s| !s.eq_ignore_ascii_case("to"));
    let dir = text(p, &["direction", "dir"]).or(first).unwrap_or_else(|| "down".to_string()).to_lowercase();
    let (w, h) = (ctx.dev_w, ctx.dev_h);
    // A finger moves the opposite way to the content: "down" drags upwards.
    let (x1, y1, x2, y2) = match dir.as_str() {
        "down" => (w / 2, h * 72 / 100, w / 2, h * 28 / 100),
        "up" => (w / 2, h * 28 / 100, w / 2, h * 72 / 100),
        "right" => (w * 78 / 100, h / 2, w * 22 / 100, h / 2),
        "left" => (w * 22 / 100, h / 2, w * 78 / 100, h / 2),
        other => return Err(format!("scroll direction must be up, down, left or right, not '{}'", other)),
    };
    let one = |ctx: &Ctx| shell_input(ctx, &["swipe", &x1.to_string(), &y1.to_string(), &x2.to_string(), &y2.to_string(), "400"]);
    let Some(target) = to else {
        one(ctx)?;
        return Ok(Done { desc: format!("scroll({})", dir), mark: None });
    };
    let max = num(p, &["max"]).map_or(8, |m| (m as usize).clamp(1, 30));
    for n in 0..=max {
        if ui_nodes(ctx).map(|nodes| has_label(&nodes, &target)).unwrap_or(false) {
            return Ok(Done { desc: format!("scroll({} to \"{}\", {} scroll(s))", dir, target, n), mark: None });
        }
        if n < max {
            one(ctx)?;
            thread::sleep(Duration::from_millis(500));
        }
    }
    Err(format!("\"{}\" did not appear after {} scroll(s) {}; it may be in another direction or behind a different screen", target, max, dir))
}

/// After the action: wait for `--expect "text"` to appear (up to --timeout ms, default 5000), else the fixed --wait.
/// Returns a note for the result line and whether the expectation held.
pub(super) fn settle(ctx: &Ctx, p: &Value) -> (Option<String>, bool) {
    if let Some(q) = text(p, &["expect"]) {
        let limit = num(p, &["timeout"]).map_or(5000, |t| t.max(0.0) as u64).min(30_000);
        let t0 = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(350));
            if ui_nodes(ctx).map(|n| has_label(&n, &q)).unwrap_or(false) {
                return (Some(format!("expected \"{}\" found after {:.1}s", q, t0.elapsed().as_secs_f64())), true);
            }
            if t0.elapsed() >= Duration::from_millis(limit) {
                return (Some(format!("expected \"{}\" NOT found after {}s", q, limit / 1000)), false);
            }
        }
    }
    let wait = num(p, &["wait"]).map_or(DEFAULT_WAIT_MS, |w| w.max(0.0) as u64).min(30_000);
    thread::sleep(Duration::from_millis(wait));
    (None, true)
}

/// A key press waits until the screen stops changing (two equal captures in a row) instead of a fixed time, unless
/// `--wait` or `--expect` says otherwise. Other actions keep the fixed wait.
pub(super) fn settle_for(ctx: &Ctx, action: &str, p: &Value) -> (Option<String>, bool) {
    if matches!(action, "keyevent" | "key") && get(p, &["wait", "expect"]).is_none() {
        let cap = num(p, &["settle_timeout", "settle-timeout"]).map_or(focus::SETTLE_CAP_MS, |t| t.max(0.0) as u64).min(30_000);
        let (ms, settled) = focus::wait_settled(ctx, cap);
        return (Some(format!("{} in {}ms", if settled { "settled" } else { "still changing" }, ms)), true);
    }
    settle(ctx, p)
}

pub(super) fn diff_enabled(action: &str, p: &Value) -> bool {
    !matches!(action, "screenshot" | "shot" | "wait") && !flag(p, "no_diff") && !flag(p, "no-diff")
}

/// One action, then the screenshot.
pub(super) fn run_one(ctx: &Ctx, action: &str, p: &Value) -> Result<String, String> {
    if !matches!(action, "screenshot" | "shot") {
        guard(ctx, p)?;
    }
    let before = if diff_enabled(action, p) { screen_hash(ctx) } else { None };
    let done = act(ctx, action, p)?;
    let (note, _) = settle_for(ctx, action, p);
    // `--no-shot`: text only. The answer is the result line and where focus is (a few tokens instead of an image).
    if flag(p, "no_shot") || flag(p, "no-shot") {
        let mut head = format!("{} ok", done.desc);
        if let Some(b) = before {
            head.push_str(if screen_hash(ctx) == Some(b) { " | screen UNCHANGED" } else { " | screen changed" });
        }
        if let Some(n) = note {
            head.push_str(&format!(" | {}", n));
        }
        log_touch(ctx, &head);
        return Ok(format!("{}\n{}", head, focus::focus_line(ctx)));
    }
    let shot = match capture(ctx, p, done.mark) {
        Ok(s) => s,
        Err(e) if matches!(action, "screenshot" | "shot") => return Err(e),
        Err(e) => return Err(format!("{} ok, but no screenshot: {}", done.desc, e)),
    };
    let mut head = format!("{} ok", done.desc);
    if let Some(b) = before {
        head.push_str(if b == shot.hash { " | screen UNCHANGED" } else { " | screen changed" });
    }
    if let Some(n) = note {
        head.push_str(&format!(" | {}", n));
    }
    let mut text_out = publish(ctx, &shot, &head);
    if flag(p, "focus") {
        text_out.push_str(&format!("\n{}", focus::focus_line(ctx)));
    }
    Ok(text_out)
}

pub(super) fn batch(ctx: &Ctx, p: &Value) -> Result<String, String> {
    let steps: Vec<Value> = if let Some(Value::Array(a)) = p.get("steps") {
        a.clone()
    } else if let Some(path) = text(p, &["json", "file"]) {
        // The steps themselves (`--json '[...]'`) or the path of a file that holds them.
        let inline = matches!(path.trim_start().chars().next(), Some('[') | Some('{'));
        let (raw, label) = if inline {
            (path.clone(), "the inline JSON".to_string())
        } else {
            let full = if Path::new(&path).is_absolute() { PathBuf::from(&path) } else { ctx.ws.join(&path) };
            let raw = fs::read_to_string(&full).map_err(|e| format!("cannot read the steps file {}: {}", full.display(), e))?;
            (raw, path.clone())
        };
        let v: Value = serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| format!("{} is not valid JSON: {}", label, e))?;
        match v {
            Value::Array(a) => a,
            Value::Object(o) => o.get("steps").and_then(|s| s.as_array()).cloned().ok_or("the JSON must be an array of steps (or {\"steps\": [...]})")?,
            _ => return Err("the JSON must be an array of steps".to_string()),
        }
    } else {
        return Err("batch needs --json <steps.json>: [{\"action\":\"tap\",\"x\":540,\"y\":1200,\"shot\":true}, {\"action\":\"wait\",\"ms\":500}, ...]".to_string());
    };
    if steps.is_empty() {
        return Err("batch has no steps".to_string());
    }
    let mut lines = Vec::new();
    let mut last_mark = None;
    let total = steps.len();
    for (i, step) in steps.iter().enumerate() {
        let mut merged = p.clone();
        if let (Some(m), Some(s)) = (merged.as_object_mut(), step.as_object()) {
            for k in ["json", "file", "steps", "action", "wait", "text", "x", "y", "args"] {
                m.remove(k);
            }
            for (k, v) in s {
                m.insert(k.clone(), v.clone());
            }
        }
        let action = text(step, &["action", "do"]).unwrap_or_default();
        let before = if flag(step, "require_change") { screen_hash(ctx) } else { None };
        let result = guard(ctx, &merged).and_then(|_| act(ctx, &action, &merged));
        match result.and_then(|done| {
            // Every step waits (or waits for `expect`); a failed expectation or an unchanged screen stops the batch.
            let (note, held) = if matches!(action.as_str(), "wait" | "focus" | "goto") { (None, true) } else { settle_for(ctx, &action, &merged) };
            if !held {
                return Err(format!("{}: {}", done.desc, note.unwrap_or_default()));
            }
            if let Some(b) = before {
                if screen_hash(ctx) == Some(b) {
                    return Err(format!("{}: the screen did not change (require_change)", done.desc));
                }
            }
            Ok(done)
        }) {
            Ok(done) => {
                last_mark = done.mark.or(last_mark);
                if flag(step, "shot") && i + 1 < total {
                    match capture(ctx, &merged, done.mark) {
                        Ok(s) => lines.push(format!("{}. {}", i + 1, publish(ctx, &s, &format!("{} ok", done.desc)).replace('\n', "\n   "))),
                        Err(e) => lines.push(format!("{}. {} ok, no screenshot: {}", i + 1, done.desc, e)),
                    }
                } else {
                    log_touch(ctx, &format!("{} ok", done.desc));
                    lines.push(format!("{}. {} ok", i + 1, done.desc));
                }
            }
            Err(e) => {
                lines.push(format!("{}. FAILED: {}", i + 1, e));
                lines.push(format!("stopped at step {} of {}", i + 1, total));
                if let Ok(s) = capture(ctx, p, last_mark) {
                    lines.push(publish(ctx, &s, "batch stopped"));
                }
                return Err(lines.join("\n"));
            }
        }
    }
    let s = capture(ctx, p, last_mark).map_err(|e| format!("{}\nfinal screenshot failed: {}", lines.join("\n"), e))?;
    lines.push(publish(ctx, &s, &format!("batch ok ({} steps)", total)));
    Ok(lines.join("\n"))
}

pub(super) fn ui_dump(ctx: &Ctx) -> Result<String, String> {
    let nodes = ui_nodes(ctx)?;
    if nodes.is_empty() {
        return Err("no visible elements (blank or protected screen)".to_string());
    }
    let mut out = vec![format!("{} elements on screen {}x{}: [index] text | content-desc | id | clickable | center(x,y)", nodes.len(), ctx.dev_w, ctx.dev_h)];
    out.extend(nodes.iter().enumerate().map(|(i, n)| n.line(i)));
    Ok(out.join("\n"))
}

pub fn execute(action: &str, params: &Value, ws: Option<&Path>) -> String {
    let result = (|| -> Result<String, String> {
        let ctx = context(params, ws)?;
        match action {
            "ui_dump" | "uidump" => ui_dump(&ctx),
            "batch" => batch(&ctx, params),
            "focus" => focus::report(&ctx, params),
            "goto" => focus::goto(&ctx, params),
            "scenario" => scenario::run(&ctx, params),
            a => run_one(&ctx, a, params),
        }
    })();
    match result {
        Ok(t) => t,
        Err(e) => err(e),
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keycodes_and_text_are_normalised() {
        assert_eq!(keycode("back"), "KEYCODE_BACK");
        assert_eq!(keycode("KEYCODE_HOME"), "KEYCODE_HOME");
        assert_eq!(keycode("66"), "66");
        assert_eq!(keycode("recent"), "KEYCODE_APP_SWITCH");
        assert_eq!(escape_text("a b&c"), "a%sb\\&c");
    }

    #[test]
    fn element_lines_and_unescape() {
        let n = Node { text: "Apply".into(), desc: String::new(), id: "com.x:id/apply_btn".into(), clickable: true, b: (100, 200, 300, 260) };
        assert_eq!(n.line(2), "[2] Apply | - | apply_btn | clickable | (200,230)");
        assert_eq!(unescape("a &amp; b &lt;c&gt; &quot;d&quot;"), "a & b <c> \"d\"");
    }

    #[test]
    fn image_space_maps_back_to_device_pixels() {
        let ctx = Ctx { serial: "s".into(), ws: PathBuf::new(), dev_w: 1080, dev_h: 2412 };
        let dev = json!({});
        let img = json!({"space": "image"});
        assert_eq!(to_device(&ctx, &dev, 540.0), 540);
        // 2412 px tall -> 2000 px image: scale 0.829, so image (448, 1000) is device (540, 1206)
        assert_eq!(to_device(&ctx, &img, 448.0), 540);
        assert_eq!(to_device(&ctx, &img, 1000.0), 1206);
        assert_eq!(to_device(&ctx, &json!({"space": "image", "scale": 0.5}), 270.0), 540);
        assert!(check_range(&ctx, "tap", 1080, 10).is_err());
        assert!(check_range(&ctx, "tap", 1079, 2411).is_ok());
    }

    #[test]
    fn params_accept_numbers_and_strings() {
        let p = json!({"x": "540", "y": 1200.0, "grid": "", "args": ["a", "b"]});
        assert_eq!(num(&p, &["x"]), Some(540.0));
        assert_eq!(num(&p, &["y"]), Some(1200.0));
        assert!(flag(&p, "grid"));
        assert!(!flag(&p, "missing"));
        assert_eq!(args_of(&p), vec!["a", "b"]);
    }
}
