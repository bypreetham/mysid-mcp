//! `mysid android scenario steps.json`: a scripted run with checks, one report at the end.
//!
//! Steps launch the app, press keys, `goto` a label, tap, and `expect` text or focus. The report says pass / fail per
//! step (a screenshot only for the first failure), the cold-start time from `am start -W`, the share of janky frames
//! from `dumpsys gfxinfo`, and every crash / ANR / exception of the app's own process, tied to the step that was
//! running when logcat recorded it. The same file can be rerun after each change.

use super::focus;
use super::touch::*;
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_EXPECT_MS: u64 = 5000;
const BOOL_FLAGS: &[&str] = &["cold", "no_ok", "list", "grid", "keep_going", "no_shot", "focus", "fast", "any_app"];

struct Step {
    name: String,
    v: Value,
}

#[derive(Debug, PartialEq, Clone)]
pub(super) struct Event {
    kind: &'static str, // CRASH, ANR, NATIVE CRASH, exception
    text: String,
}

impl Event {
    fn fatal(&self) -> bool {
        self.kind != "exception"
    }
}

// ---------------------------------------------------------------- reading the file

/// Words of a step line; `"quoted words"` and `'single quotes'` stay together.
fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut started = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                started = true;
            }
            None if c.is_whitespace() => {
                if started || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            None => cur.push(c),
        }
    }
    if started || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `expect focus "Play all"`, `goto "USB Drive" "Durandhar"`, `launch --cold`, `key dpad_down --times 3` as a step object.
fn step_from_line(line: &str) -> Result<Value, String> {
    let t = tokenize(line);
    let Some(first) = t.first() else { return Err("an empty step".to_string()) };
    let action = first.to_lowercase();
    let mut positional: Vec<String> = Vec::new();
    let mut fields = serde_json::Map::new();
    let mut i = 1;
    while i < t.len() {
        if let Some(name) = t[i].strip_prefix("--") {
            let name = name.replace('-', "_");
            if BOOL_FLAGS.contains(&name.as_str()) || i + 1 >= t.len() || t[i + 1].starts_with("--") {
                fields.insert(name, json!(true));
            } else {
                fields.insert(name, json!(t[i + 1]));
                i += 1;
            }
        } else {
            positional.push(t[i].clone());
        }
        i += 1;
    }
    let mut obj = fields;
    match action.as_str() {
        "expect" => {
            let Some(kind) = positional.first().map(|k| k.to_lowercase()) else {
                return Err("expect needs text | focus | gone | app, then the value: expect focus \"Play all\"".to_string());
            };
            let kind = match kind.as_str() {
                "text" | "focus" | "gone" | "app" => kind,
                other => return Err(format!("expect {} is not known: use text, focus, gone or app", other)),
            };
            obj.insert(kind, json!(positional[1..].join(" ")));
        }
        "goto" => {
            obj.insert("targets".to_string(), json!(positional));
        }
        "launch" | "launch_app" | "start" => {
            if let Some(p) = positional.first() {
                obj.insert("package".to_string(), json!(p));
            }
        }
        "key" | "keyevent" => {
            if let Some(k) = positional.first() {
                obj.insert("key".to_string(), json!(k));
            }
        }
        "wait" => {
            if let Some(ms) = positional.first() {
                obj.insert("ms".to_string(), json!(ms));
            }
        }
        _ => {
            if !positional.is_empty() {
                obj.insert("args".to_string(), json!(positional));
            }
        }
    }
    obj.insert("action".to_string(), json!(action));
    Ok(Value::Object(obj))
}

fn read_steps(ctx: &Ctx, p: &Value) -> Result<(Vec<Step>, Value, String), String> {
    let source = text(p, &["json", "file"]).or_else(|| args_of(p).first().cloned());
    let root: Value = if let Some(Value::Array(a)) = p.get("steps") {
        json!({ "steps": a })
    } else if let Some(path) = source {
        let inline = matches!(path.trim_start().chars().next(), Some('[') | Some('{'));
        let raw = if inline {
            path.clone()
        } else {
            let full = if Path::new(&path).is_absolute() { PathBuf::from(&path) } else { ctx.ws.join(&path) };
            fs::read_to_string(&full).map_err(|e| format!("cannot read the scenario {}: {}", full.display(), e))?
        };
        serde_json::from_str(raw.trim_start_matches('\u{feff}')).map_err(|e| format!("the scenario is not valid JSON: {}", e))?
    } else {
        return Err("scenario needs a steps file: mysid android scenario steps.json\n{\"app\":\"com.example.tv\",\"steps\":[\"launch --cold\",\"goto \\\"USB Drive\\\"\",\"expect text \\\"Play all\\\"\",\"expect focus \\\"Play all\\\"\"]}".to_string());
    };
    let (steps_v, meta) = match &root {
        Value::Array(a) => (a.clone(), json!({})),
        Value::Object(o) => (o.get("steps").and_then(|s| s.as_array()).cloned().ok_or("the scenario object needs a \"steps\" array")?, root.clone()),
        _ => return Err("the scenario must be an array of steps or {\"steps\": [...]}".to_string()),
    };
    if steps_v.is_empty() {
        return Err("the scenario has no steps".to_string());
    }
    let mut steps = Vec::new();
    for (i, s) in steps_v.iter().enumerate() {
        let (name, v) = match s {
            Value::String(line) => (line.clone(), step_from_line(line).map_err(|e| format!("step {}: {}", i + 1, e))?),
            Value::Object(_) => {
                let v = s.clone();
                let name = text(&v, &["name"]).unwrap_or_else(|| describe_object(&v));
                (name, v)
            }
            _ => return Err(format!("step {} must be a string or an object", i + 1)),
        };
        steps.push(Step { name, v });
    }
    let name = text(&meta, &["name"]).unwrap_or_else(|| text(p, &["json", "file"]).or_else(|| args_of(p).first().cloned()).filter(|s| !s.starts_with('[') && !s.starts_with('{')).unwrap_or_else(|| "scenario".to_string()));
    Ok((steps, meta, name))
}

fn describe_object(v: &Value) -> String {
    let action = text(v, &["action", "do"]).unwrap_or_default();
    let detail = ["text", "focus", "gone", "app", "key", "package", "ms"].iter().find_map(|k| text(v, &[k]).map(|t| format!("{} \"{}\"", k, t)));
    match (action.as_str(), detail) {
        ("expect", Some(d)) => format!("expect {}", d),
        (a, Some(d)) => format!("{} {}", a, d),
        (a, None) => match v.get("targets") {
            Some(Value::Array(t)) => format!("{} {}", a, t.iter().filter_map(|x| x.as_str()).map(|x| format!("\"{}\"", x)).collect::<Vec<_>>().join(" ")),
            _ => a.to_string(),
        },
    }
}

// ---------------------------------------------------------------- device side

fn shell(ctx: &Ctx, args: &[&str], timeout: Duration) -> String {
    let mut full = vec!["shell"];
    full.extend_from_slice(args);
    adb(Some(&ctx.serial), &full, timeout).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// `TotalTime` of `am start -W` in ms, or why the start failed.
pub(super) fn parse_start(out: &str) -> Result<u64, String> {
    if let Some(c) = Regex::new(r"TotalTime:\s*(\d+)").unwrap().captures(out) {
        return Ok(c[1].parse().unwrap_or(0));
    }
    let bad = out.lines().find(|l| l.contains("Error") || l.contains("Warning") || l.contains("Exception")).unwrap_or("no TotalTime in the output");
    Err(format!("am start -W gave no launch time: {}", bad.trim()))
}

/// The activity to start for a package: the launcher activity, or the leanback one on a TV.
fn launcher_component(ctx: &Ctx, pkg: &str) -> Option<String> {
    for cat in [None, Some("android.intent.category.LEANBACK_LAUNCHER")] {
        let mut args = vec!["cmd", "package", "resolve-activity", "--brief"];
        if let Some(c) = cat {
            args.extend(["-c", c]);
        }
        args.push(pkg);
        let out = shell(ctx, &args, ADB_TIMEOUT);
        if let Some(line) = out.lines().rev().map(str::trim).find(|l| l.contains('/') && !l.contains(' ')) {
            return Some(line.to_string());
        }
    }
    None
}

/// Crashes, ANRs and logged exceptions of `pkg` in a `logcat -v threadtime` dump (errors and worse).
pub(super) fn events_from_log(log: &str, pkg: &str, app_pid: Option<&str>) -> Vec<Event> {
    let line_re = Regex::new(r"^\d\d-\d\d\s+[\d:.]+\s+(\d+)\s+\d+\s+([VDIWEF])\s+(.+?)\s*:\s?(.*)$").unwrap();
    let ex_re = Regex::new(r"^[\w.$]+(Exception|Error)\b").unwrap();
    let rows: Vec<(String, String, String)> = log.lines().filter_map(|l| line_re.captures(l).map(|c| (c[1].to_string(), c[3].to_string(), c[4].to_string()))).collect();
    let mut events: Vec<Event> = Vec::new();
    let mut consumed = vec![false; rows.len()];
    let push = |events: &mut Vec<Event>, e: Event| {
        if !events.contains(&e) {
            events.push(e);
        }
    };
    for i in 0..rows.len() {
        let (pid, tag, msg) = &rows[i];
        if tag == "AndroidRuntime" && msg.starts_with("FATAL EXCEPTION") {
            let block: Vec<usize> = (i..rows.len().min(i + 60)).filter(|&j| rows[j].0 == *pid && rows[j].1 == "AndroidRuntime").collect();
            for &j in &block {
                consumed[j] = true;
            }
            let process = block.iter().find_map(|&j| rows[j].2.strip_prefix("Process: ").map(|r| r.split(',').next().unwrap_or("").trim().to_string()));
            if process.as_deref() == Some(pkg) {
                let exc = block.iter().map(|&j| rows[j].2.as_str()).find(|m| ex_re.is_match(m)).unwrap_or("uncaught exception");
                let frame = block.iter().map(|&j| rows[j].2.trim()).find(|m| m.starts_with("at ")).map(|f| format!(" ({})", f)).unwrap_or_default();
                push(&mut events, Event { kind: "CRASH", text: format!("{}{}", clip(exc, 200), frame) });
            }
        } else if msg.starts_with("ANR in ") && msg.contains(pkg) {
            let reason = (i + 1..rows.len().min(i + 8)).map(|j| rows[j].2.trim()).find(|m| m.starts_with("Reason:")).unwrap_or("");
            push(&mut events, Event { kind: "ANR", text: format!("{} {}", clip(msg, 120), reason).trim().to_string() });
        } else if msg.contains(&format!(">>> {} <<<", pkg)) {
            let signal = rows.iter().map(|r| r.2.as_str()).find(|m| m.contains("Fatal signal")).unwrap_or("native crash");
            push(&mut events, Event { kind: "NATIVE CRASH", text: clip(signal, 160) });
        }
    }
    if let Some(ap) = app_pid {
        for (i, (pid, _, msg)) in rows.iter().enumerate() {
            if !consumed[i] && pid == ap && msg.contains("Exception") {
                push(&mut events, Event { kind: "exception", text: clip(msg, 180) });
            }
        }
    }
    events.truncate(8);
    events
}

fn clip(s: &str, n: usize) -> String {
    let s = s.trim().replace('\n', " ");
    if s.chars().count() > n { format!("{}...", s.chars().take(n).collect::<String>()) } else { s }
}

/// What logcat recorded since the last call (and clears it), as events of the app.
fn collect_events(ctx: &Ctx, pkg: &str) -> Vec<Event> {
    let pid = shell(ctx, &["pidof", pkg], ADB_TIMEOUT).split_whitespace().next().map(String::from);
    let log = adb(Some(&ctx.serial), &["logcat", "-d", "-b", "all", "-v", "threadtime", "*:E"], ADB_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let _ = adb(Some(&ctx.serial), &["logcat", "-b", "all", "-c"], ADB_TIMEOUT);
    events_from_log(&log, pkg, pid.as_deref())
}

/// `frames: 340 rendered, 12 janky (3.5%), p90 14ms, p99 40ms` from `dumpsys gfxinfo`, with the janky share.
pub(super) fn parse_gfx(out: &str) -> Option<(u64, u64, f64, String)> {
    let total: u64 = Regex::new(r"Total frames rendered:\s*(\d+)").unwrap().captures(out)?[1].parse().ok()?;
    let janky = Regex::new(r"Janky frames:\s*(\d+)\s*\(([\d.]+)%\)").unwrap().captures(out);
    let (count, pct) = janky.map_or((0, 0.0), |c| (c[1].parse().unwrap_or(0), c[2].parse().unwrap_or(0.0)));
    let pcts: Vec<String> = [("90th", "p90"), ("95th", "p95"), ("99th", "p99")]
        .iter()
        .filter_map(|(k, label)| Regex::new(&format!(r"{} percentile:\s*(\d+)ms", k)).unwrap().captures(out).map(|c| format!("{} {}ms", label, &c[1])))
        .collect();
    Some((total, count, pct, pcts.join(", ")))
}

// ---------------------------------------------------------------- running

struct Outcome {
    ok: bool,
    note: String,
    screenshot: Option<String>,
    events: Vec<Event>,
    ms: u128,
}

/// Polls `check` until it returns Ok(true) or `limit_ms` passes. Err carries the last thing seen.
fn wait_for(limit_ms: u64, mut check: impl FnMut() -> Result<(bool, String), String>) -> Result<String, String> {
    let t0 = Instant::now();
    loop {
        let (held, seen) = check()?;
        if held {
            return Ok(format!("after {:.1}s", t0.elapsed().as_secs_f64()));
        }
        if t0.elapsed() >= Duration::from_millis(limit_ms) {
            return Err(format!("{} after {:.1}s", seen, t0.elapsed().as_secs_f64()));
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn expect(ctx: &Ctx, v: &Value, pkg: &str) -> Result<String, String> {
    let limit = num(v, &["timeout"]).map_or(DEFAULT_EXPECT_MS, |t| t.max(0.0) as u64).min(60_000);
    if let Some(q) = text(v, &["text"]) {
        return wait_for(limit, || {
            let nodes = ui_nodes(ctx)?;
            let held = has_label(&nodes, &q);
            let seen: Vec<String> = nodes.iter().filter(|n| !n.text.is_empty() || !n.desc.is_empty()).take(8).map(|n| if n.text.is_empty() { n.desc.clone() } else { n.text.clone() }).collect();
            Ok((held, format!("text \"{}\" is not on screen (visible: {})", q, seen.join(" | "))))
        })
        .map(|t| format!("text \"{}\" found {}", q, t));
    }
    if let Some(q) = text(v, &["focus"]) {
        return wait_for(limit, || {
            let (held, now) = focus::focus_is(ctx, &q)?;
            Ok((held, format!("focus is {}, not \"{}\"", now, q)))
        })
        .map(|t| format!("focus on \"{}\" {}", q, t));
    }
    if let Some(q) = text(v, &["gone"]) {
        return wait_for(limit, || {
            let nodes = ui_nodes(ctx)?;
            Ok((!has_label(&nodes, &q), format!("\"{}\" is still on screen", q)))
        })
        .map(|t| format!("\"{}\" gone {}", q, t));
    }
    if let Some(want) = text(v, &["app"]).or_else(|| Some(pkg.to_string()).filter(|_| v.get("app").is_some())) {
        return wait_for(limit, || {
            let fg = foreground(ctx).unwrap_or_else(|| "unknown".to_string());
            Ok((fg == want, format!("the foreground app is {}, not {}", fg, want)))
        })
        .map(|t| format!("{} in front {}", want, t));
    }
    Err("expect needs text, focus, gone or app".to_string())
}

fn launch(ctx: &Ctx, v: &Value, pkg: &str, cold_ms: &mut Option<u64>) -> Result<String, String> {
    let pkg = text(v, &["package", "app", "package_name"]).unwrap_or_else(|| pkg.to_string());
    if pkg.is_empty() {
        return Err("launch needs the app: \"app\" at the top of the scenario, or launch <package>".to_string());
    }
    let cold = flag(v, "cold");
    if cold {
        shell(ctx, &["am", "force-stop", &pkg], ADB_TIMEOUT);
        thread::sleep(Duration::from_millis(600));
    }
    let comp = match text(v, &["activity"]) {
        Some(a) if a.contains('/') => a,
        Some(a) => format!("{}/{}", pkg, a),
        None => launcher_component(ctx, &pkg).ok_or_else(|| format!("cannot find a launcher activity for {} (is it installed?). Give \"activity\".", pkg))?,
    };
    let out = shell(ctx, &["am", "start", "-W", "-n", &comp], Duration::from_secs(40));
    let ms = parse_start(&out)?;
    if cold && cold_ms.is_none() {
        *cold_ms = Some(ms);
    }
    thread::sleep(Duration::from_millis(300));
    Ok(format!("{} start {} ms ({})", if cold { "cold" } else { "warm" }, ms, comp))
}

fn run_step(ctx: &Ctx, base: &Value, step: &Step, pkg: &str, cold_ms: &mut Option<u64>) -> Result<String, String> {
    let mut merged = base.clone();
    if let (Some(m), Some(s)) = (merged.as_object_mut(), step.v.as_object()) {
        for k in ["json", "file", "steps", "action", "wait", "text", "x", "y", "args", "targets", "package_name"] {
            m.remove(k);
        }
        for (k, v) in s {
            m.insert(k.clone(), v.clone());
        }
    }
    let action = text(&step.v, &["action", "do"]).unwrap_or_default().to_lowercase();
    match action.as_str() {
        "launch" | "launch_app" | "start" => launch(ctx, &merged, pkg, cold_ms),
        "expect" => expect(ctx, &merged, pkg),
        "" => Err("the step has no action".to_string()),
        "scenario" | "batch" => Err(format!("{} cannot be nested in a scenario", action)),
        a => {
            // Every action is guarded: keys sent while another app is in front (a media key that started SmartTube) would test the
            // wrong app. `"app"` on a step names another one; `--any-app` / `"any_app": true` turns the guard off for the step.
            if !flag(&merged, "any_app") && !pkg.is_empty() && merged.get("app").is_none() {
                if let Some(m) = merged.as_object_mut() {
                    m.insert("app".to_string(), json!(pkg));
                }
            }
            guard(ctx, &merged)?;
            let done = act(ctx, a, &merged)?;
            let note = if matches!(a, "wait" | "focus" | "goto") { None } else { settle_for(ctx, a, &merged).0 };
            // `goto` answers with a summary line and the focus line; the report keeps the summary.
            let desc = done.desc.lines().next().unwrap_or("").to_string();
            Ok(match note {
                Some(n) => format!("{} | {}", desc, n),
                None => desc,
            })
        }
    }
}

fn pad(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    while out.chars().count() < n {
        out.push(' ');
    }
    out
}

pub(super) fn run(ctx: &Ctx, p: &Value) -> Result<String, String> {
    let t_all = Instant::now();
    let (steps, meta, name) = read_steps(ctx, p)?;
    let pkg = text(&meta, &["app", "package"]).or_else(|| text(p, &["app", "package"])).or_else(|| foreground(ctx)).unwrap_or_default();
    let keep_going = flag(&meta, "keep_going") || flag(p, "keep_going") || flag(p, "keep-going");
    // Clean slate: only what happens during the run is reported.
    let _ = adb(Some(&ctx.serial), &["logcat", "-b", "all", "-c"], ADB_TIMEOUT);
    if !pkg.is_empty() {
        shell(ctx, &["dumpsys", "gfxinfo", &pkg, "reset"], ADB_TIMEOUT);
    }
    let mut cold_ms: Option<u64> = None;
    let mut outcomes: Vec<Option<Outcome>> = Vec::new();
    let mut first_fail: Option<usize> = None;
    let base = {
        let mut b = p.clone();
        if let Some(o) = b.as_object_mut() {
            for k in ["json", "file", "steps", "args", "package_name", "app"] {
                o.remove(k);
            }
        }
        b
    };
    for (i, step) in steps.iter().enumerate() {
        if first_fail.is_some() && !keep_going {
            outcomes.push(None);
            continue;
        }
        let t0 = Instant::now();
        let result = run_step(ctx, &base, step, &pkg, &mut cold_ms);
        let events = if pkg.is_empty() { Vec::new() } else { collect_events(ctx, &pkg) };
        let crashed = events.iter().find(|e| e.fatal()).cloned();
        let (mut ok, mut note) = match result {
            Ok(n) => (true, n),
            Err(e) => (false, e),
        };
        if let Some(c) = crashed {
            if !flag(&step.v, "allow_crash") {
                note = if ok { format!("{} CRASHED the app: {}", c.kind, c.text) } else { format!("{} (and the app had a {}: {})", note, c.kind, c.text) };
                ok = false;
            }
        }
        let mut screenshot = None;
        if !ok {
            first_fail.get_or_insert(i);
            if let Ok(s) = capture(ctx, &base, None) {
                screenshot = Some(publish(ctx, &s, &format!("scenario step {} failed", i + 1)));
            }
        }
        log_touch(ctx, &format!("scenario {} step {} {} {}: {}", name, i + 1, step.name, if ok { "PASS" } else { "FAIL" }, note.replace('\n', " ")));
        outcomes.push(Some(Outcome { ok, note, screenshot, events, ms: t0.elapsed().as_millis() }));
    }

    // Frame stats and budgets.
    let gfx = if pkg.is_empty() { None } else { parse_gfx(&shell(ctx, &["dumpsys", "gfxinfo", &pkg], ADB_TIMEOUT)) };
    let mut budget_fail: Vec<String> = Vec::new();
    if let (Some(limit), Some(ms)) = (num(&meta, &["max_cold_start_ms"]), cold_ms) {
        if ms as f64 > limit {
            budget_fail.push(format!("cold start {} ms is over the budget of {} ms", ms, limit));
        }
    }
    if let (Some(limit), Some((_, _, pct, _))) = (num(&meta, &["max_janky_percent"]), gfx.as_ref()) {
        if *pct > limit {
            budget_fail.push(format!("janky frames {}% is over the budget of {}%", pct, limit));
        }
    }

    let total = steps.len();
    let failed = first_fail.is_some() || !budget_fail.is_empty();
    let mut out: Vec<String> = Vec::new();
    out.push(match first_fail {
        Some(i) => format!("scenario {} FAIL - step {} of {}: {}", name, i + 1, total, steps[i].name),
        None if failed => format!("scenario {} FAIL - {} step(s) passed, budget exceeded", name, total),
        None => format!("scenario {} PASS - {} step(s) in {:.1}s", name, total, t_all.elapsed().as_secs_f64()),
    });
    for (i, (step, o)) in steps.iter().zip(outcomes.iter()).enumerate() {
        match o {
            Some(o) => {
                out.push(format!("{:>2} {} {} {}{}", i + 1, if o.ok { "PASS" } else { "FAIL" }, pad(&step.name, 34), clip(&o.note, 220), if o.ms > 1500 { format!(" [{:.1}s]", o.ms as f64 / 1000.0) } else { String::new() }));
                for e in &o.events {
                    out.push(format!("      ! {} {}: {}", e.kind, pkg, e.text));
                }
                if let Some(s) = &o.screenshot {
                    out.push(format!("      {}", s.replace('\n', "\n      ")));
                }
            }
            None => out.push(format!("{:>2} SKIP {}", i + 1, step.name)),
        }
    }
    out.push(String::new());
    out.push(match cold_ms {
        Some(ms) => format!("cold start: {} ms (am start -W TotalTime)", ms),
        None => "cold start: not measured (add a \"launch --cold\" step)".to_string(),
    });
    out.push(match &gfx {
        Some((frames, janky, pct, p)) => format!("frames: {} rendered, {} janky ({}%){}", frames, janky, pct, if p.is_empty() { String::new() } else { format!(", {}", p) }),
        None => "frames: no stats (the app drew no frames, or gfxinfo is unavailable)".to_string(),
    });
    let all_events: usize = outcomes.iter().flatten().map(|o| o.events.len()).sum();
    out.push(if all_events == 0 { "crashes / ANRs / exceptions: none".to_string() } else { format!("crashes / ANRs / exceptions: {} (listed under the step that was running)", all_events) });
    for b in &budget_fail {
        out.push(format!("BUDGET: {}", b));
    }
    let report = out.join("\n");
    if failed { Err(report) } else { Ok(report) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_lines_become_step_objects() {
        let v = step_from_line("goto \"USB Drive\" Durandhar --no-ok --max 12").unwrap();
        assert_eq!(v["action"], "goto");
        assert_eq!(v["targets"], json!(["USB Drive", "Durandhar"]));
        assert_eq!((v["no_ok"].clone(), v["max"].clone()), (json!(true), json!("12")));
        let e = step_from_line("expect focus \"Play all\" --timeout 8000").unwrap();
        assert_eq!((e["action"].clone(), e["focus"].clone(), e["timeout"].clone()), (json!("expect"), json!("Play all"), json!("8000")));
        assert_eq!(step_from_line("expect text Play all").unwrap()["text"], "Play all");
        let l = step_from_line("launch --cold").unwrap();
        assert_eq!((l["action"].clone(), l["cold"].clone()), (json!("launch"), json!(true)));
        let k = step_from_line("key dpad_down --times 3").unwrap();
        assert_eq!((k["key"].clone(), k["times"].clone()), (json!("dpad_down"), json!("3")));
        assert!(step_from_line("expect nonsense x").is_err() && step_from_line("expect").is_err() && step_from_line("").is_err());
    }

    #[test]
    fn launch_time_comes_from_am_start() {
        let out = "Starting: Intent { cmp=com.x/.Main }\nStatus: ok\nLaunchState: COLD\nActivity: com.x/.Main\nTotalTime: 812\nWaitTime: 830\nComplete";
        assert_eq!(parse_start(out), Ok(812));
        assert!(parse_start("Error: Activity class {com.x/.Nope} does not exist.").unwrap_err().contains("does not exist"));
    }

    #[test]
    fn crashes_anrs_and_exceptions_of_the_app_are_found() {
        let log = "\
10-13 10:00:01.100  4000  4000 E AndroidRuntime: FATAL EXCEPTION: main
10-13 10:00:01.100  4000  4000 E AndroidRuntime: Process: com.x.tv, PID: 4000
10-13 10:00:01.100  4000  4000 E AndroidRuntime: java.lang.IllegalStateException: no folder
10-13 10:00:01.100  4000  4000 E AndroidRuntime: \tat com.x.tv.Usb.open(Usb.kt:42)
10-13 10:00:02.000  5000  5000 E AndroidRuntime: FATAL EXCEPTION: main
10-13 10:00:02.000  5000  5000 E AndroidRuntime: Process: com.other, PID: 5000
10-13 10:00:02.000  5000  5000 E AndroidRuntime: java.lang.RuntimeException: not ours
10-13 10:00:03.000   900   950 E ActivityManager: ANR in com.x.tv (com.x.tv/.Main)
10-13 10:00:03.000   900   950 E ActivityManager: Reason: Input dispatching timed out
10-13 10:00:04.000  7000  7000 E UsbScanner: java.io.IOException: read failed
10-13 10:00:04.500  7001  7001 E Other: java.io.IOException: someone else
10-13 10:00:05.000  8000  8000 F DEBUG   : pid: 8000, tid: 8000, name: x  >>> com.x.tv <<<
10-13 10:00:05.000  8000  8000 F libc    : Fatal signal 11 (SIGSEGV), code 1
";
        let ev = events_from_log(log, "com.x.tv", Some("7000"));
        let kinds: Vec<&str> = ev.iter().map(|e| e.kind).collect();
        assert_eq!(kinds, vec!["CRASH", "ANR", "NATIVE CRASH", "exception"], "{:?}", ev);
        assert!(ev[0].text.contains("IllegalStateException: no folder") && ev[0].text.contains("Usb.kt:42"), "{:?}", ev[0]);
        assert!(ev[1].text.contains("Input dispatching timed out"));
        assert!(ev[3].text.contains("read failed"));
        assert!(ev[2].text.contains("SIGSEGV"));
        // Another app's crash alone is nothing.
        assert!(events_from_log(log, "com.nothing", None).is_empty());
    }

    #[test]
    fn frame_stats_give_the_janky_share() {
        let out = "Stats since: 1234ns\nTotal frames rendered: 340\nJanky frames: 12 (3.53%)\n50th percentile: 6ms\n90th percentile: 14ms\n95th percentile: 22ms\n99th percentile: 40ms\n";
        assert_eq!(parse_gfx(out), Some((340, 12, 3.53, "p90 14ms, p95 22ms, p99 40ms".to_string())));
        assert_eq!(parse_gfx("No process found for: com.x"), None);
    }
}
