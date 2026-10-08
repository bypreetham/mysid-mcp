use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAX_WARNING_LINES: usize = 10;
const MAX_ERROR_LINES: usize = 25;

/// Locates cargo: $CARGO, ~/.cargo/bin, any rustup toolchain, then plain `cargo` on PATH.
pub fn find_cargo() -> String {
    let exe = if cfg!(windows) { "cargo.exe" } else { "cargo" };

    if let Ok(p) = std::env::var("CARGO") {
        if Path::new(&p).exists() {
            return p;
        }
    }

    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    if let Some(home) = home {
        let home = PathBuf::from(home);
        let direct = home.join(".cargo").join("bin").join(exe);
        if direct.exists() {
            return direct.to_string_lossy().to_string();
        }
        if let Ok(entries) = fs::read_dir(home.join(".rustup").join("toolchains")) {
            let mut toolchains: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            // Prefer the stable toolchain when several are installed.
            toolchains.sort_by_key(|p| !p.to_string_lossy().contains("stable"));
            for tc in toolchains {
                let candidate = tc.join("bin").join(exe);
                if candidate.exists() {
                    return candidate.to_string_lossy().to_string();
                }
            }
        }
    }
    "cargo".to_string()
}

/// Condenses `cargo build --message-format=short` output into a status line plus
/// short diagnostics. Success shows warnings; failure shows errors only.
pub fn summarize(stdout: &str, stderr: &str, success: bool, elapsed: Duration, show_warnings: bool) -> String {
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    // cargo prints `warning: msg` / `error[E0308]: msg`, then `  --> path:line:col`.
    let mut pending: Option<(bool, String)> = None; // (is_error, header)
    let mut flush = |pending: &mut Option<(bool, String)>, loc: Option<String>| {
        if let Some((is_error, header)) = pending.take() {
            let entry = match loc {
                Some(l) => format!("{} {}", l, header),
                None => header,
            };
            if seen.insert(entry.clone()) {
                if is_error { errors.push(entry) } else { warnings.push(entry) }
            }
        }
    };

    for raw in stderr.lines().chain(stdout.lines()) {
        let line = raw.trim();
        if let Some(loc) = line.strip_prefix("-->") {
            flush(&mut pending, Some(short_location(loc)));
            continue;
        }
        // Roll-up lines are not diagnostics.
        if line.starts_with("warning: `")
            || line.starts_with("error: could not compile")
            || line.starts_with("error: aborting")
            || line.starts_with("warning: build failed")
        {
            continue;
        }
        let is_error = line.starts_with("error");
        if is_error || line.starts_with("warning") {
            flush(&mut pending, None);
            pending = Some((is_error, line.to_string()));
        }
    }
    flush(&mut pending, None);

    let secs = elapsed.as_secs_f32();
    let mut out = if success {
        format!("BUILD OK (Rust, {:.1}s) - {} warning(s)", secs, warnings.len())
    } else {
        format!("BUILD FAILED (Rust, {:.1}s) - {} error(s), {} warning(s)", secs, errors.len(), warnings.len())
    };

    if !success {
        if errors.is_empty() {
            // Nothing parseable (e.g. manifest or spawn problem): show the raw tail.
            let tail: Vec<&str> = stderr.lines().rev().take(15).collect();
            for l in tail.into_iter().rev() {
                out.push_str(&format!("\n  {}", l));
            }
        }
        push_capped(&mut out, &errors, MAX_ERROR_LINES);
        if !warnings.is_empty() {
            out.push_str("\n(warnings hidden while build is failing)");
        }
    } else if show_warnings {
        push_capped(&mut out, &warnings, MAX_WARNING_LINES);
    }
    out
}

fn push_capped(out: &mut String, items: &[String], cap: usize) {
    for item in items.iter().take(cap) {
        out.push_str(&format!("\n  {}", item));
    }
    if items.len() > cap {
        out.push_str(&format!("\n  ... +{} more", items.len() - cap));
    }
}

/// `src\a.rs:37:7` -> `src/a.rs:37`
fn short_location(loc: &str) -> String {
    let loc = loc.trim().replace('\\', "/");
    let mut parts = loc.rsplitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(col), Some(row), Some(path)) if col.parse::<u32>().is_ok() && row.parse::<u32>().is_ok() => {
            format!("{}:{}", path, row)
        }
        _ => loc,
    }
}
