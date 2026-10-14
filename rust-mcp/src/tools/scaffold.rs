//! `mysid new <stack> <name>`: create a new project.
//!
//! - `react`: the official Vite generator (`npm create vite@latest <name> -- --template react-ts`), then `npm install`.
//! - `android`: not available yet.
//!
//! A project is only ever created in a folder that does not exist or is empty; nothing is overwritten.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::exec::run_with_timeout;

const CREATE_TIMEOUT: Duration = Duration::from_secs(300);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);
const STACKS: &[&str] = &["react", "android"];

pub fn execute_new(params: &Value, _default_root: Option<&str>) -> String {
    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let words: Vec<&str> = query.split_whitespace().collect();
    let Some(&stack) = words.first() else {
        return out(false, &format!("ERROR: usage: mysid new <stack> <name> [--dir <folder>]. Stacks: {}", STACKS.join(", ")));
    };
    let stack = stack.to_ascii_lowercase();
    if !STACKS.contains(&stack.as_str()) {
        return out(false, &format!("ERROR: unknown stack '{}'. Available: {}", stack, STACKS.join(", ")));
    }
    let joined = words[1..].join(" ");
    let name = joined.as_str();
    if name.is_empty() {
        return out(false, &format!("ERROR: a project name is required. Example: mysid new {} my-app", stack));
    }
    let base = match base_dir(params) {
        Ok(b) => b,
        Err(e) => return out(false, &e),
    };
    match stack.as_str() {
        "react" => new_react(name, &base, params),
        _ => out(false, "ERROR: `mysid new android` is not available yet. `mysid new react <name>` is."),
    }
}

/// `--dir`, else the current folder. (Never the saved workspace: that is usually a different project.)
fn base_dir(params: &Value) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("ERROR: cannot read the current folder: {}", e))?;
    let dir = match params.get("dir").and_then(|v| v.as_str()) {
        Some(d) if !d.trim().is_empty() => {
            let p = PathBuf::from(d.trim());
            if p.is_absolute() { p } else { cwd.join(p) }
        }
        _ => cwd,
    };
    fs::create_dir_all(&dir).map_err(|e| format!("ERROR: cannot create {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// npm package-name rules, trimmed to what a folder name needs: lower case, digits, `-`, `_`, `.`, starting with a letter or digit.
fn check_package_name(name: &str) -> Result<(), String> {
    let kebab: String = {
        let mut s = String::new();
        for c in name.chars() {
            if c.is_ascii_uppercase() {
                if !s.is_empty() && !s.ends_with('-') {
                    s.push('-');
                }
                s.push(c.to_ascii_lowercase());
            } else if c.is_whitespace() {
                if !s.ends_with('-') {
                    s.push('-');
                }
            } else {
                s.push(c);
            }
        }
        s
    };
    let ok = !name.is_empty()
        && name.len() <= 214
        && name.chars().next().map_or(false, |c| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "ERROR: '{}' is not a valid project name (lower case letters, digits, '-', '_', '.'; start with a letter or digit). Try: {}",
            name, kebab
        ))
    }
}

/// A folder that is missing or empty is fine; anything else is refused.
fn check_destination(dest: &Path) -> Result<(), String> {
    match fs::read_dir(dest) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                Err(format!("ERROR: {} already exists and is not empty; choose another name or --dir", dest.display()))
            } else {
                Ok(())
            }
        }
        Err(_) => Ok(()),
    }
}

/// `npm ...` (a .cmd shim on Windows, so it goes through the shell) in `cwd`.
fn npm(args: &[&str], cwd: &Path, timeout: Duration) -> Result<(i32, String), String> {
    #[cfg(windows)]
    let mut cmd = {
        use std::os::windows::process::CommandExt;
        let mut c = Command::new("cmd");
        c.arg("/C").arg("npm").args(args).creation_flags(0x0800_0000);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("npm");
        c.args(args);
        c
    };
    cmd.current_dir(cwd).env("CI", "1").env("npm_config_yes", "true").env("npm_config_fund", "false").env("npm_config_audit", "false");
    let done = run_with_timeout(cmd, timeout).map_err(|e| format!("cannot run npm ({}). Install Node.js (https://nodejs.org) and try again.", e))?;
    if done.timed_out {
        return Err(format!("npm {} did not finish in {} s and was stopped", args.first().unwrap_or(&""), timeout.as_secs()));
    }
    Ok((done.exit_code, format!("{}\n{}", done.stdout, done.stderr).trim().to_string()))
}

fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

fn new_react(name: &str, base: &Path, params: &Value) -> String {
    if let Err(e) = check_package_name(name) {
        return out(false, &e);
    }
    let template = params.get("template").and_then(|v| v.as_str()).unwrap_or("react-ts");
    if !template.chars().all(|c| c.is_ascii_lowercase() || c == '-') || !template.starts_with("react") {
        return out(false, &format!("ERROR: template '{}' is not a React template (react-ts, react, react-swc-ts, react-swc)", template));
    }
    let dest = base.join(name);
    if let Err(e) = check_destination(&dest) {
        return out(false, &e);
    }
    match npm(&["--version"], base, Duration::from_secs(30)) {
        Ok((0, _)) => {}
        Ok((_, o)) => return out(false, &format!("ERROR: npm does not work: {}", last_lines(&o, 3))),
        Err(e) => return out(false, &format!("ERROR: {}", e)),
    }

    eprintln!("[1/2] creating {} with the official Vite generator ({}) - this downloads create-vite from npm...", name, template);
    match npm(&["create", "vite@latest", name, "--", "--template", template], base, CREATE_TIMEOUT) {
        Ok((0, _)) if dest.join("package.json").exists() => {}
        Ok((code, o)) => return out(false, &format!("ERROR: creating the project failed (exit {}):\n{}", code, last_lines(&o, 12))),
        Err(e) => return out(false, &format!("ERROR: {}", e)),
    }
    eprintln!("[2/2] npm install in {} ...", dest.display());
    match npm(&["install"], &dest, INSTALL_TIMEOUT) {
        Ok((0, _)) => {}
        Ok((code, o)) => {
            return out(
                false,
                &format!(
                    "ERROR: the project was created at {} but npm install failed (exit {}):\n{}\nFix the cause (network?) and run `npm install` there.",
                    dest.display(),
                    code,
                    last_lines(&o, 12)
                ),
            )
        }
        Err(e) => return out(false, &format!("ERROR: project created at {}, but {}", dest.display(), e)),
    }

    let files = count_files(&dest);
    let quoted = dest.display().to_string();
    out(
        true,
        &format!(
            "Created React project '{}' ({}, Vite) at {}\n{} files, dependencies installed.\nnext:\n  cd \"{}\"   (mysid finds the project from the folder you are in)\n  mysid build            (tsc + production build)\n  mysid skill install    (workflow guides for this workspace)\n  mysid commit \"Initial project\"   (creates the git repository)",
            name, template, quoted, files, quoted
        ),
    )
}

fn count_files(dir: &Path) -> usize {
    let mut n = 0;
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if e.file_name() == "node_modules" {
                continue;
            }
            if p.is_dir() {
                n += count_files(&p);
            } else {
                n += 1;
            }
        }
    }
    n
}

fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_names_follow_package_rules_and_get_a_suggestion() {
        for ok in ["my-app", "app2", "a.b_c", "x"] {
            assert!(check_package_name(ok).is_ok(), "{}", ok);
        }
        let bad = check_package_name("My Player").unwrap_err();
        assert!(bad.contains("Try: my-player"), "{}", bad);
        for bad in ["", "-x", "_x", "a/b", "a b", "ÄÖ"] {
            assert!(check_package_name(bad).is_err(), "{:?}", bad);
        }
    }

    #[test]
    fn only_missing_or_empty_destinations_are_accepted() {
        let dir = std::env::temp_dir().join(format!("mysid_new_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(check_destination(&dir.join("fresh")).is_ok());
        fs::create_dir_all(dir.join("empty")).unwrap();
        assert!(check_destination(&dir.join("empty")).is_ok());
        fs::create_dir_all(dir.join("full")).unwrap();
        fs::write(dir.join("full").join("a.txt"), "x").unwrap();
        let err = check_destination(&dir.join("full")).unwrap_err();
        assert!(err.contains("not empty"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_errors_are_explained_without_touching_the_disk() {
        let run = |q: &str| execute_new(&json!({ "query": q }), None);
        assert!(run("").contains("usage: mysid new"));
        assert!(run("cobol app").contains("unknown stack"));
        assert!(run("react").contains("name is required"));
        assert!(run("android app").contains("not available yet"));
    }
}
