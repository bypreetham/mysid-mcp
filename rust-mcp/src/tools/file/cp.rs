//! `mysid file cp <src>... <dest>` (also copy): copy files or folders into the workspace.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use crate::tools::workspace::resolve_root;

/// Universal dispatcher for `file <action> ...` subcommands:
/// `new`, `create`, `write`, `delete`, `remove`, `rm`, `rename`, `mv`, `mkdir`, `list`, `ls`
/// Files to copy as (source file, destination file); `.git`, `node_modules` and `.mysid` are not entered.
fn collect_copy(src: &Path, dst: &Path, out: &mut Vec<(std::path::PathBuf, std::path::PathBuf)>, skipped: &mut Vec<String>) {
    if src.is_dir() {
        let Ok(rd) = fs::read_dir(src) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".git" | "node_modules" | ".mysid") && e.path().is_dir() {
                skipped.push(name);
                continue;
            }
            collect_copy(&e.path(), &dst.join(&name), out, skipped);
        }
    } else {
        out.push((src.to_path_buf(), dst.to_path_buf()));
    }
}

/// `mysid file copy <src>... <dest>`: files or folders, many in one call, into the workspace. Nothing is overwritten
/// unless `--overwrite`, and nothing is copied when any source is missing or any target exists.
pub fn execute_copy(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let reply = |ok: bool, t: String| json!({ "success": ok, "output": t }).to_string();
    let sources: Vec<String> = params.get("sources").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default();
    let dest = params.get("dest").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if sources.is_empty() || dest.is_empty() {
        return reply(false, "NOT COPIED: usage: mysid file copy <source>... <destination>  (several sources need a destination folder)".to_string());
    }
    let overwrite = ["overwrite", "force"].iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false));
    let root = Path::new(&workspace_root);

    let dest_path = if Path::new(&dest).is_absolute() { std::path::PathBuf::from(&dest) } else { root.join(&dest) };
    if !crate::tools::build::exec::inside(root, &dest_path) {
        return reply(false, format!("NOT COPIED: the destination {} is outside the workspace ({}). Sources may be anywhere; the destination must be inside it.", dest, workspace_root));
    }
    let mut from: Vec<std::path::PathBuf> = Vec::new();
    for s in &sources {
        let p = if Path::new(s).is_absolute() { std::path::PathBuf::from(s) } else { crate::tools::workspace::resolve_or_join(root, s, &workspace_root) };
        if !p.exists() {
            return reply(false, format!("NOT COPIED: the source {} does not exist ({})", s, p.display()));
        }
        from.push(p);
    }
    // `copy src/ dest/` (trailing slash), several sources, or a file into an existing folder: inside the destination.
    // One folder to a destination without a slash: its contents ARE the destination (never src/src on a second run).
    let into_dir = from.len() > 1 || dest.ends_with('/') || dest.ends_with('\\') || (dest_path.is_dir() && !from[0].is_dir());
    let mut pairs = Vec::new();
    let mut skipped = Vec::new();
    for p in &from {
        let target = if into_dir { dest_path.join(p.file_name().unwrap_or_default()) } else { dest_path.clone() };
        collect_copy(p, &target, &mut pairs, &mut skipped);
    }
    if pairs.is_empty() {
        return reply(false, "NOT COPIED: nothing to copy (the sources are empty folders)".to_string());
    }
    let existing: Vec<String> = pairs.iter().filter(|(_, d)| d.exists()).map(|(_, d)| d.strip_prefix(root).unwrap_or(d).to_string_lossy().replace('\\', "/")).collect();
    if !existing.is_empty() && !overwrite {
        let shown: Vec<&str> = existing.iter().take(8).map(String::as_str).collect();
        return reply(false, format!("NOT COPIED: {} target file(s) already exist: {}{}. Nothing was copied; add --overwrite to replace them.", existing.len(), shown.join(", "), if existing.len() > 8 { ", ..." } else { "" }));
    }
    let mut bytes = 0u64;
    for (s, d) in &pairs {
        if let Some(parent) = d.parent() {
            if let Err(e) = fs::create_dir_all(parent) {
                return reply(false, format!("COPY STOPPED: cannot create {}: {}", parent.display(), e));
            }
        }
        match fs::copy(s, d) {
            Ok(n) => bytes += n,
            Err(e) => return reply(false, format!("COPY STOPPED after an error on {}: {}", s.display(), e)),
        }
    }
    let mut out = format!("copied {} file(s), {} KB to {}", pairs.len(), bytes / 1024, dest);
    if !skipped.is_empty() {
        skipped.sort();
        skipped.dedup();
        out.push_str(&format!("\nnot copied: {} (never copied)", skipped.join(", ")));
    }
    reply(true, out)
}
