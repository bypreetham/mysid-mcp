//! `mysid test`: run the project tests and print only what matters: one line when everything passes, the failing
//! tests with their messages when not. The full output is in `.mysid/test.log`.

use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::build::log;
use super::build_rust;
use super::build::exec::run_with_timeout;
use super::workspace::resolve_existing_root;

pub const HELP: &str = "mysid test: run the project tests, print failures only\n\n\
Usage:\n  \
  mysid test                 run all tests (stack detected: Cargo, npm/pnpm/yarn test, Gradle, pytest)\n  \
  mysid test <name>          only tests matching the name (cargo test <name>, jest/vitest path, gradle --tests, pytest -k)\n  \
  mysid test --timeout 900   seconds before the run is killed (default 600)\n\n\
Success is one line: TESTS OK (Rust, 104 passed, 9.9s). Failure lists each failing test with its message and the\n\
location. The complete output is .mysid/test.log. Watch modes are switched off (CI=true).\n";

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stack {
    Rust,
    Node,
    Gradle,
    Python,
}

impl Stack {
    fn label(self) -> &'static str {
        match self {
            Stack::Rust => "Rust",
            Stack::Node => "Node",
            Stack::Gradle => "Gradle",
            Stack::Python => "Python",
        }
    }
}

fn node_has_test_script(root: &Path) -> bool {
    fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .map_or(false, |v| v.get("scripts").and_then(|s| s.get("test")).is_some())
}

fn detect(root: &Path) -> Option<Stack> {
    if root.join("Cargo.toml").exists() {
        return Some(Stack::Rust);
    }
    if root.join("package.json").exists() && node_has_test_script(root) {
        return Some(Stack::Node);
    }
    if root.join("gradlew.bat").exists() || root.join("gradlew").exists() {
        return Some(Stack::Gradle);
    }
    let py = root.join("pytest.ini").exists()
        || fs::read_to_string(root.join("pyproject.toml")).map_or(false, |t| t.contains("pytest"))
        || (root.join("tests").is_dir() && root.join("requirements.txt").exists());
    if py {
        return Some(Stack::Python);
    }
    None
}

fn command_for(stack: Stack, root: &Path, name: Option<&str>) -> Command {
    let on_win = cfg!(windows);
    match stack {
        Stack::Rust => {
            let mut c = Command::new(build_rust::find_cargo());
            c.args(["test", "--color", "never"]);
            if let Some(n) = name {
                c.arg(n);
            }
            c
        }
        Stack::Node => {
            let pm = if root.join("pnpm-lock.yaml").exists() {
                "pnpm"
            } else if root.join("yarn.lock").exists() {
                "yarn"
            } else {
                "npm"
            };
            let mut line = format!("{} test", pm);
            if let Some(n) = name {
                line.push_str(&format!(" -- \"{}\"", n.replace('"', "")));
            }
            let mut c;
            if on_win {
                c = Command::new("cmd.exe");
                c.args(["/C", &line]);
            } else {
                c = Command::new("sh");
                c.args(["-c", &line]);
            }
            c.env("CI", "true").env("NO_COLOR", "1").env("FORCE_COLOR", "0");
            c
        }
        Stack::Gradle => {
            let wrapper = if on_win { root.join("gradlew.bat") } else { root.join("gradlew") };
            let mut c = Command::new(wrapper);
            let android = ["app/build.gradle.kts", "app/build.gradle"].iter().any(|f| fs::read_to_string(root.join(f)).map_or(false, |t| t.contains("com.android")));
            c.arg(if android { "testDebugUnitTest" } else { "test" }).arg("--console=plain");
            if let Some(n) = name {
                c.args(["--tests", n]);
            }
            c
        }
        Stack::Python => {
            let mut c = Command::new(if on_win { "python" } else { "python3" });
            c.args(["-m", "pytest", "-q", "--no-header"]);
            if let Some(n) = name {
                c.args(["-k", n]);
            }
            c
        }
    }
}

struct Outcome {
    ok: bool,
    headline: String,
    details: Vec<String>,
}

fn clip(s: &str) -> String {
    s.trim().chars().take(200).collect()
}

fn pick_lines(text: &str, pattern: &str, max: usize) -> Vec<String> {
    let re = Regex::new(pattern).unwrap();
    // npm echoes the command it runs (`> node -e ...`); that is not a result.
    let hits: Vec<String> = text.lines().filter(|l| !l.starts_with("> ") && re.is_match(l)).map(clip).collect();
    let skip = hits.len().saturating_sub(max);
    hits.into_iter().skip(skip).collect()
}

fn tail_lines(text: &str, n: usize) -> Vec<String> {
    let all: Vec<String> = text.lines().filter(|l| !l.trim().is_empty()).map(clip).collect();
    let skip = all.len().saturating_sub(n);
    all.into_iter().skip(skip).collect()
}

fn summarize_rust(out: &str, exit_ok: bool) -> Outcome {
    let result_re = Regex::new(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored").unwrap();
    let (mut passed, mut failed, mut ignored, mut seen) = (0u32, 0u32, 0u32, false);
    for c in result_re.captures_iter(out) {
        seen = true;
        passed += c[2].parse::<u32>().unwrap_or(0);
        failed += c[3].parse::<u32>().unwrap_or(0);
        ignored += c[4].parse::<u32>().unwrap_or(0);
    }
    if !seen {
        // No test binary ran: a compile error.
        let mut d = pick_lines(out, r"^error(\[E\d+\])?:|^\s+--> ", 20);
        if d.is_empty() {
            d = tail_lines(out, 8);
        }
        return Outcome { ok: false, headline: "did not compile".to_string(), details: d };
    }
    if failed == 0 && exit_ok {
        let extra = if ignored > 0 { format!(", {} ignored", ignored) } else { String::new() };
        return Outcome { ok: true, headline: format!("{} passed{}", passed, extra), details: vec![] };
    }
    // `---- name stdout ----` blocks hold each failure's panic message.
    let mut details = Vec::new();
    let lines: Vec<&str> = out.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(name) = lines[i].strip_prefix("---- ").and_then(|l| l.strip_suffix(" stdout ----")) {
            details.push(name.to_string());
            let mut shown = 0;
            i += 1;
            while i < lines.len() && !lines[i].starts_with("---- ") && lines[i].trim() != "failures:" && shown < 4 {
                let l = lines[i].trim();
                if !l.is_empty() && !l.starts_with("note: run with") {
                    details.push(format!("  {}", clip(l)));
                    shown += 1;
                }
                i += 1;
            }
            continue;
        }
        i += 1;
    }
    if details.is_empty() {
        details = pick_lines(out, r"FAILED|panicked", 15);
    }
    Outcome { ok: false, headline: format!("{} failed, {} passed", failed, passed), details }
}

fn summarize_node(out: &str, exit_ok: bool) -> Outcome {
    let summary = pick_lines(out, r"^\s*(Tests?:|Test Files|Test Suites:)|\d+ passing|\d+ passed", 3);
    if exit_ok {
        let head = if summary.is_empty() { "tests passed".to_string() } else { summary.iter().map(|s| s.trim().to_string()).collect::<Vec<_>>().join("; ") };
        return Outcome { ok: true, headline: head, details: vec![] };
    }
    let mut details = pick_lines(out, r"FAIL|✕|×|●|AssertionError|Error:|Expected|Received|expected|\d+ failing|ERR!", 25);
    if details.is_empty() {
        details = tail_lines(out, 10);
    }
    Outcome { ok: false, headline: summary.first().map(|s| s.trim().to_string()).unwrap_or_else(|| "tests failed".to_string()), details }
}

fn summarize_gradle(out: &str, exit_ok: bool) -> Outcome {
    if exit_ok {
        return Outcome { ok: true, headline: "BUILD SUCCESSFUL".to_string(), details: vec![] };
    }
    let mut details = pick_lines(out, r" FAILED$|tests completed|What went wrong|Execution failed|error:|Caused by", 25);
    if details.is_empty() {
        details = tail_lines(out, 10);
    }
    let head = pick_lines(out, r"\d+ tests? completed", 1).first().cloned().unwrap_or_else(|| "tests failed".to_string());
    Outcome { ok: false, headline: head, details }
}

fn summarize_python(out: &str, exit_ok: bool) -> Outcome {
    let last = pick_lines(out, r"\d+ (passed|failed|error)", 1).first().cloned().unwrap_or_default();
    let head = last.trim_matches('=').trim().to_string();
    if exit_ok {
        return Outcome { ok: true, headline: if head.is_empty() { "tests passed".to_string() } else { head }, details: vec![] };
    }
    let mut details = pick_lines(out, r"^FAILED |^ERROR |^E  ", 25);
    if details.is_empty() {
        details = tail_lines(out, 10);
    }
    Outcome { ok: false, headline: head, details }
}

fn summarize(stack: Stack, out: &str, exit_ok: bool) -> Outcome {
    match stack {
        Stack::Rust => summarize_rust(out, exit_ok),
        Stack::Node => summarize_node(out, exit_ok),
        Stack::Gradle => summarize_gradle(out, exit_ok),
        Stack::Python => summarize_python(out, exit_ok),
    }
}

pub fn execute_test(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    let reply = |ok: bool, t: &str| json!({ "success": ok, "output": t }).to_string();
    let Some(stack) = detect(root) else {
        return reply(false, "NO TESTS RUN: no Cargo.toml, package.json with a \"test\" script, Gradle wrapper or pytest setup in this folder.");
    };
    let name = ["query", "name", "filter"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).map(str::trim).filter(|s| !s.is_empty());
    let timeout = params
        .get("timeout")
        .or_else(|| params.get("timeout_secs"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(600);

    let mut cmd = command_for(stack, root, name);
    cmd.current_dir(root);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let started = Instant::now();
    let run = match run_with_timeout(cmd, Duration::from_secs(timeout)) {
        Ok(r) => r,
        Err(e) => return reply(false, &format!("TESTS NOT RUN ({}): cannot start the test command: {}", stack.label(), e)),
    };
    let secs = started.elapsed().as_secs_f64();
    let all = format!("{}\n{}", run.stdout, run.stderr);
    if let Ok(dir) = log::ensure_dir(root) {
        let _ = fs::write(dir.join("test.log"), &all);
    }
    if run.timed_out {
        return reply(false, &format!("TESTS TIMED OUT ({}, killed after {}s; raise it with --timeout)\nlog: .mysid/test.log", stack.label(), timeout));
    }
    let out = summarize(stack, &all, run.exit_code == 0);
    let mut text = if out.ok {
        format!("TESTS OK ({}, {}, {:.1}s)", stack.label(), out.headline, secs)
    } else {
        format!("TESTS FAILED ({}, {}, {:.1}s)", stack.label(), out.headline, secs)
    };
    for d in &out.details {
        text.push_str(&format!("\n  {}", d));
    }
    if !out.ok {
        text.push_str("\nlog: .mysid/test.log");
    }
    reply(out.ok, &text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_pass_fail_and_compile_error() {
        let ok = "running 2 tests\ntest a ... ok\ntest b ... ok\n\ntest result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let o = summarize_rust(ok, true);
        assert!(o.ok && o.headline == "2 passed", "{}", o.headline);

        let bad = "test a ... ok\ntest m::b ... FAILED\n\nfailures:\n\n---- m::b stdout ----\nthread 'm::b' panicked at src/m.rs:9:5:\nassertion `left == right` failed\n  left: 1\n right: 2\n\nfailures:\n    m::b\n\ntest result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n";
        let o = summarize_rust(bad, false);
        assert!(!o.ok && o.headline == "1 failed, 1 passed", "{}", o.headline);
        assert_eq!(o.details[0], "m::b");
        assert!(o.details.iter().any(|d| d.contains("src/m.rs:9:5")), "{:?}", o.details);

        let ce = "error[E0425]: cannot find value `x`\n  --> src/a.rs:3:5\n";
        let o = summarize_rust(ce, false);
        assert!(!o.ok && o.details.len() == 2 && o.headline == "did not compile");
    }

    #[test]
    fn node_gradle_python_summaries() {
        let o = summarize_node("Test Files  2 passed (2)\n     Tests  9 passed (9)\n", true);
        assert!(o.ok && o.headline.contains("9 passed"), "{}", o.headline);
        let o = summarize_node(" FAIL  src/a.test.ts\n AssertionError: expected 1 to be 2\n", false);
        assert!(!o.ok && o.details.len() == 2);
        let o = summarize_gradle("FooTest > bar FAILED\n    java.lang.AssertionError\n3 tests completed, 1 failed\n", false);
        assert!(!o.ok && o.headline.contains("3 tests completed") && o.details[0].contains("FooTest > bar FAILED"));
        let o = summarize_python("FAILED tests/test_a.py::test_x - assert 1 == 2\n=== 1 failed, 4 passed in 0.1s ===\n", false);
        assert!(!o.ok && o.headline == "1 failed, 4 passed in 0.1s");
    }

    #[test]
    fn no_stack_is_reported() {
        let d = std::env::temp_dir().join(format!("mysid_test_none_{}", std::process::id()));
        let _ = fs::create_dir_all(&d);
        let r = execute_test(&json!({ "workspace_root": d.to_string_lossy() }), None);
        assert!(r.contains("NO TESTS RUN"), "{}", r);
        let _ = fs::remove_dir_all(&d);
    }
}
