//! Tracker: read-only views of the repository: `status`, `branch`, `history`.

use serde_json::Value;
use std::path::Path;

use super::{compact_stat, git, has_commits, in_repo, is_secret, text};

const DEFAULT_HISTORY: u64 = 5;
const MAX_HISTORY: u64 = 100;
use crate::tools::workspace::resolve_existing_root;

/// `mysid status`: the branch, how it relates to its upstream, and the changed files.
pub fn execute_status(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    if let Err(e) = git(root, &["--version"]) {
        return text(false, &format!("ERROR: {}", e));
    }
    if !in_repo(root) {
        return text(true, "not a git repository here (mysid commit or mysid checkpoint creates one)");
    }
    let o = match git(root, &["status", "--porcelain=v1", "-b", "--untracked-files=all", "--", "."]) {
        Ok(o) if o.code == 0 => o,
        Ok(o) => return text(false, &format!("ERROR: git status failed: {}", o.stderr)),
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    let mut lines = o.stdout.lines();
    let head = lines.next().unwrap_or("").trim_start_matches("## ").to_string();
    let (mut staged, mut unstaged, mut untracked) = (0, 0, 0);
    let mut entries: Vec<String> = Vec::new();
    for l in lines.filter(|l| l.len() > 3) {
        let (x, y) = (l.as_bytes()[0] as char, l.as_bytes()[1] as char);
        if x == '?' {
            untracked += 1;
        } else {
            if x != ' ' {
                staged += 1;
            }
            if y != ' ' {
                unstaged += 1;
            }
        }
        let path = l[3..].to_string();
        let flag = if is_secret(path.rsplit(" -> ").next().unwrap_or(&path)) { "  (looks like a secret: mysid commit leaves it out)" } else { "" };
        entries.push(format!("{} {}{}", &l[..2], path, flag));
    }
    let branch = head.replace("...", " -> ").replace("No commits yet on ", "no commits yet on ");
    let last = git(root, &["log", "-1", "--format=%h %s (%cr)"]).map(|o| o.stdout).unwrap_or_default();
    let stashes = git(root, &["stash", "list"]).map(|o| o.stdout.lines().count()).unwrap_or(0);
    let mut out = Vec::new();
    if entries.is_empty() {
        out.push(format!("{} - clean, nothing to commit", branch));
    } else {
        out.push(format!("{} - {} change(s): {} staged, {} unstaged, {} untracked", branch, entries.len(), staged, unstaged, untracked));
        let shown = entries.len().min(40);
        out.extend(entries.iter().take(shown).cloned());
        if entries.len() > shown {
            out.push(format!("... and {} more", entries.len() - shown));
        }
    }
    if !last.trim().is_empty() {
        out.push(format!("last commit: {}", last.trim()));
    }
    if stashes > 0 {
        out.push(format!("{} stash(es)", stashes));
    }
    text(true, &out.join("\n"))
}

/// `mysid branch`: local branches with their last commit (read only).
pub fn execute_branch(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    if !in_repo(root) {
        return text(true, "not a git repository here (mysid commit or mysid checkpoint creates one)");
    }
    match git(root, &["branch", "-vv", "--no-color"]) {
        Ok(o) if o.code == 0 && !o.stdout.trim().is_empty() => text(true, o.stdout.lines().take(40).map(|l| l.chars().take(160).collect::<String>()).collect::<Vec<_>>().join("\n").as_str()),
        Ok(o) if o.code == 0 => text(true, "no branches yet (no commits)"),
        Ok(o) => text(false, &format!("ERROR: git branch failed: {}", o.stderr)),
        Err(e) => text(false, &format!("ERROR: {}", e)),
    }
}

pub fn execute_history(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    if let Err(e) = git(root, &["--version"]) {
        return text(false, &format!("ERROR: {}", e));
    }
    if !in_repo(root) {
        return text(true, "Not a git repository yet. `mysid commit \"message\"` creates one and makes the first commit.");
    }
    if !has_commits(root) {
        return text(true, "No commits yet. `mysid commit \"message\"` makes the first one.");
    }
    let n = ["past", "n", "count", "limit"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_u64()))
        .unwrap_or(DEFAULT_HISTORY)
        .clamp(1, MAX_HISTORY);
    let arg = format!("-{}", n);
    let log = match git(root, &["log", &arg, "--shortstat", "--format=@%h|%cr|%s"]) {
        Ok(o) if o.code == 0 => o.stdout,
        Ok(o) => return text(false, &format!("ERROR: git log failed: {}", o.stderr)),
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    text(true, &format_history(&log))
}

/// `@hash|age|subject` headers, each followed by an optional shortstat line.
fn format_history(log: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in log.lines() {
        if let Some(h) = line.strip_prefix('@') {
            let mut p = h.splitn(3, '|');
            let (hash, age, subject) = (p.next().unwrap_or(""), p.next().unwrap_or(""), p.next().unwrap_or(""));
            out.push(format!("{}  {:<14} {}", hash, age, subject));
        } else if line.contains("changed") {
            if let Some(last) = out.last_mut() {
                last.push_str(&format!("  ({})", compact_stat(line)));
            }
        }
    }
    format!("{} commit(s), newest first:\n{}", out.len(), out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_and_history_lines_are_compact() {
        assert_eq!(compact_stat(" 4 files changed, 37 insertions(+), 9 deletions(-)"), "4 files, +37 -9");
        assert_eq!(compact_stat(" 1 file changed, 2 insertions(+)"), "1 file, +2 -0");
        let log = "@a1b2c3d|2 hours ago|Add recorder\n\n 4 files changed, 37 insertions(+), 9 deletions(-)\n@e4f5a6b|3 days ago|Init\n\n 1 file changed, 1 insertion(+)\n";
        let out = format_history(log);
        assert!(out.starts_with("2 commit(s), newest first:"), "{}", out);
        assert!(out.contains("a1b2c3d  2 hours ago") && out.contains("Add recorder  (4 files, +37 -9)"), "{}", out);
    }
}
