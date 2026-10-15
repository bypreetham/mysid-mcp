//! `mysid new react <name>`: the official Vite generator (`npm create vite@latest`), then `npm install`.

use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::{check_destination, check_package_name, out};
use crate::tools::build::exec::run_with_timeout;

const CREATE_TIMEOUT: Duration = Duration::from_secs(300);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);

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

pub(super) fn new_react(name: &str, base: &Path, params: &Value) -> String {
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
