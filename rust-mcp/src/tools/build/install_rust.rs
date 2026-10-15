//! `mysid install` in a Cargo project: build the release binary and put it in the per-user bin folder
//! (`%LOCALAPPDATA%\mysid\bin`, or `~/.local/bin`). A running executable cannot be overwritten on Windows, but it can
//! be renamed, so the old file is moved to `<name>.<time>.old` first.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use super::build_rust;

/// The binary name: `[[bin]] name`, else `[package] name`.
fn bin_name(manifest: &str) -> Option<String> {
    let mut section = String::new();
    let (mut package, mut bin) = (None, None);
    for line in manifest.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            section = l.to_string();
        } else if let Some(rest) = l.strip_prefix("name") {
            let v = rest.trim_start().strip_prefix('=').map(|v| v.trim().trim_matches('"').to_string());
            match section.as_str() {
                "[package]" => package = package.or(v),
                "[[bin]]" => bin = bin.or(v),
                _ => {}
            }
        }
    }
    bin.or(package)
}

fn install_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var("LOCALAPPDATA").ok().map(|p| PathBuf::from(p).join("mysid").join("bin"))
    } else {
        std::env::var("HOME").ok().map(|p| PathBuf::from(p).join(".local").join("bin"))
    }
}

/// Copies `src` over `dest`, moving an existing (possibly running) `dest` aside first.
fn replace_file(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
    }
    // Earlier `.old` copies may still be running (and so locked): delete what can be deleted, use a fresh name.
    if let (Some(dir), Some(stem)) = (dest.parent(), dest.file_name().and_then(|n| n.to_str())) {
        if let Ok(entries) = fs::read_dir(dir) {
            for e in entries.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.starts_with(stem) && n.ends_with(".old") {
                    let _ = fs::remove_file(e.path());
                }
            }
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let old = dest.with_file_name(format!("{}.{}.old", dest.file_name().and_then(|n| n.to_str()).unwrap_or("mysid"), stamp));
    if dest.exists() {
        fs::rename(dest, &old).map_err(|e| format!("cannot move the installed file aside ({}): {}", dest.display(), e))?;
    }
    if let Err(e) = fs::copy(src, dest) {
        let _ = fs::rename(&old, dest); // put the previous version back
        return Err(format!("cannot copy to {}: {}", dest.display(), e));
    }
    Ok(())
}

/// `root` must contain a Cargo.toml.
pub fn execute(root: &Path, _params: &Value) -> String {
    let manifest = match fs::read_to_string(root.join("Cargo.toml")) {
        Ok(m) => m,
        Err(e) => return format!("NOT INSTALLED: cannot read Cargo.toml: {}", e),
    };
    let Some(name) = bin_name(&manifest) else {
        return "NOT INSTALLED: Cargo.toml has no package name.".to_string();
    };
    let Some(dir) = install_dir() else {
        return "NOT INSTALLED: cannot tell where to install (LOCALAPPDATA / HOME not set).".to_string();
    };

    let started = Instant::now();
    let out = Command::new(build_rust::find_cargo())
        .args(["build", "--release", "--message-format=short"])
        .current_dir(root)
        .output();
    let out = match out {
        Ok(o) => o,
        Err(e) => return format!("NOT INSTALLED: cannot run cargo: {}", e),
    };
    let ok = out.status.success();
    let summary = build_rust::summarize(
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
        ok,
        started.elapsed(),
        false,
    );
    if !ok {
        return format!("NOT INSTALLED: the build failed.\n{}", summary);
    }

    let exe = if cfg!(windows) { format!("{}.exe", name) } else { name.clone() };
    let built = root.join("target").join("release").join(&exe);
    if !built.exists() {
        return format!("NOT INSTALLED: {} was not produced (is this a library crate?).", built.display());
    }
    let dest = dir.join(&exe);
    if let Err(e) = replace_file(&built, &dest) {
        return format!("NOT INSTALLED: {}", e);
    }
    // Like install.ps1: a copy already in ~/.local/bin (often earlier on PATH) is refreshed too, never created.
    let mut also = String::new();
    if cfg!(windows) {
        if let Ok(home) = std::env::var("USERPROFILE") {
            let mirror = PathBuf::from(home).join(".local").join("bin").join(&exe);
            if mirror.exists() {
                match replace_file(&built, &mirror) {
                    Ok(()) => also = format!("
  {}", mirror.display()),
                    Err(e) => also = format!("
  (not updated: {})", e),
                }
            }
        }
    }
    let size = fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    format!(
        "INSTALLED {} ({} KB, build {:.1}s)\n  {}{}\nA running copy keeps the old version until it restarts (MCP clients: reload the server).",
        exe,
        size / 1024,
        started.elapsed().as_secs_f64(),
        dest.display(),
        also
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bin_name_prefers_bin_section() {
        assert_eq!(bin_name("[package]\nname = \"a\"\n[[bin]]\nname = \"b\"\n").as_deref(), Some("b"));
        assert_eq!(bin_name("[package]\nname = \"a\"\n[dependencies]\nname = \"x\"\n").as_deref(), Some("a"));
    }

    #[test]
    fn replace_moves_old_aside() {
        let d = std::env::temp_dir().join(format!("mysid_inst_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        let (src, dest) = (d.join("new.bin"), d.join("bin").join("t.exe"));
        fs::write(&src, "new").unwrap();
        replace_file(&src, &dest).unwrap();
        fs::write(&src, "newer").unwrap();
        replace_file(&src, &dest).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "newer");
        let _ = fs::remove_dir_all(&d);
    }
}
