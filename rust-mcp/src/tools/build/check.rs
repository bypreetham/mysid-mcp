//! `mysid check`: a fast type-check without building.
//!
//! In a TypeScript project (`tsconfig.json`) it runs `tsc --noEmit` and prints only the errors, grouped under one clear
//! first line. Any other project falls through to `mysid lint`.

use serde_json::{json, Value};
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use super::exec::run_with_timeout;
use crate::tools::workspace::resolve_existing_root;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const ERRORS_SHOWN: usize = 30;

fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

/// `src/App.tsx(12,5): error TS2322: ...` lines of tsc output (also the `file:12:5 - error TS...` pretty form), in order.
fn error_lines(output: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    output
        .lines()
        .map(|l| l.trim_end())
        .filter(|l| l.contains("error TS"))
        .filter(|l| seen.insert(l.to_string()))
        .map(String::from)
        .collect()
}

/// A solution-style tsconfig (`"references"`, no files of its own, as in the Vite templates) is checked with `tsc -b`;
/// a plain `tsc --noEmit` on it would check nothing.
fn uses_references(root: &Path) -> bool {
    std::fs::read_to_string(root.join("tsconfig.json")).map_or(false, |t| t.contains("\"references\""))
}

fn label(root: &Path) -> &'static str {
    if uses_references(root) { "tsc -b" } else { "tsc --noEmit" }
}

fn tsc_command(root: &Path) -> Command {
    let mode: &[&str] = if uses_references(root) { &["-b"] } else { &["--noEmit"] };
    let local = if cfg!(windows) { root.join("node_modules/.bin/tsc.cmd") } else { root.join("node_modules/.bin/tsc") };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        if local.exists() {
            c.arg("/C").arg(&local);
        } else {
            c.arg("/C").arg("npx").arg("--no-install").arg("tsc");
        }
        c.args(mode).args(["--pretty", "false"]).creation_flags(0x0800_0000);
        c.current_dir(root);
        c
    }
    #[cfg(not(windows))]
    {
        let mut c = if local.exists() { Command::new(&local) } else { let mut n = Command::new("npx"); n.args(["--no-install", "tsc"]); n };
        c.args(mode).args(["--pretty", "false"]).current_dir(root);
        c
    }
}

pub fn execute_check(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&workspace_root);
    if !root.join("tsconfig.json").exists() {
        // Not a TypeScript project: the older `check` meant lint.
        return super::lint::execute_lint(params, default_root);
    }
    let timeout = ["timeout", "timeout_secs"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_u64())).unwrap_or(DEFAULT_TIMEOUT_SECS);
    let started = Instant::now();
    let run = match run_with_timeout(tsc_command(root), Duration::from_secs(timeout)) {
        Ok(r) => r,
        Err(e) => return out(false, &format!("CHECK FAILED - cannot run tsc: {}", e)),
    };
    let secs = started.elapsed().as_secs_f32();
    if run.timed_out {
        return out(false, &format!("CHECK TIMED OUT after {}s - tsc was killed (raise it with --timeout <secs>).", timeout));
    }
    let all = format!("{}\n{}", run.stdout, run.stderr);
    let errors = error_lines(&all);
    if run.exit_code == 0 && errors.is_empty() {
        return out(true, &format!("CHECK OK ({}, {:.1}s)", label(root), secs));
    }
    if errors.is_empty() {
        // tsc failed without a TS error: not installed, bad config, ...
        let hint: Vec<&str> = all.lines().filter(|l| !l.trim().is_empty()).take(6).collect();
        let note = if all.to_lowercase().contains("not found") || all.contains("npm ERR") || all.contains("could not determine executable") {
            "\nTypeScript is not installed in this project: run `npm install` (or `npm install -D typescript`)."
        } else {
            ""
        };
        return out(false, &format!("CHECK FAILED ({}, {:.1}s) - tsc reported a problem, not a type error:\n{}{}", label(root), secs, hint.join("\n"), note));
    }
    let mut lines = vec![format!("CHECK FAILED ({}, {:.1}s) - {} error(s)", label(root), secs, errors.len())];
    lines.extend(errors.iter().take(ERRORS_SHOWN).map(|e| format!("  {}", e)));
    if errors.len() > ERRORS_SHOWN {
        lines.push(format!("  ... +{} more (mysid exec \"npx tsc --noEmit\" --out tsc.txt for all of them)", errors.len() - ERRORS_SHOWN));
    }
    out(false, &lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_typescript_error_lines_are_kept_once() {
        let raw = "src/App.tsx(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\n  Type detail line\nsrc/A.ts(1,1): error TS2304: Cannot find name 'x'.\nsrc/App.tsx(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\nFound 2 errors.\n";
        let e = error_lines(raw);
        assert_eq!(e.len(), 2, "{:?}", e);
        assert!(e[0].starts_with("src/App.tsx(12,5): error TS2322") && e[1].starts_with("src/A.ts(1,1)"));
        assert!(error_lines("Found 0 errors.").is_empty());
    }

    #[test]
    fn a_project_without_tsconfig_falls_back_to_lint() {
        let dir = std::env::temp_dir().join(format!("mysid_check_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let r = execute_check(&json!({ "workspace_root": dir.to_string_lossy() }), None);
        assert!(!r.contains("CHECK"), "{}", r);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
