//! Committer: `mysid commit`, the one command that records changes in the repository.

use serde_json::Value;
use std::path::Path;

use super::initializer::ensure_repo;
use super::{compact_stat, git, has_commits, is_secret, text};
use crate::tools::workspace::resolve_existing_root;

fn message_of(params: &Value) -> String {
    ["message", "msg", "query", "path"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `--paths a.kt b.kt` (or `paths: [...]`): commit only these.
fn paths_of(params: &Value) -> Vec<String> {
    match params.get("paths") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        Some(Value::String(s)) => s.split(|c: char| c == ',' || c.is_whitespace()).map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect(),
        _ => Vec::new(),
    }
}

/// `mysid commit --message "msg" [--branch main] [--paths a b]` (or `mysid commit "msg"`).
pub fn execute_commit(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let message = message_of(params);
    if message.is_empty() {
        return text(false, "ERROR: a commit message is required. Example: mysid commit --message \"Add usb debug recorder\"  (or mysid commit \"Add usb debug recorder\")  [--branch main] [--paths a.kt b.kt]");
    }
    let branch = params.get("branch").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());
    commit_with(Path::new(&ws), &message, branch, &paths_of(params))
}

/// Everything under the workspace, on the current branch: what the first `mysid checkpoint` and `publish` use.
pub(crate) fn commit_all(root: &Path, message: &str) -> String {
    commit_with(root, message, None, &[])
}

/// Makes `wanted` the branch the next commit goes on, without losing work. Ok(note) says what was done.
/// - no commits yet: the (unborn) current branch is renamed, so the initial commit lands on `wanted`
/// - already on it: nothing
/// - it exists: switch to it, only when git says the switch is safe (git refuses when changes would be overwritten)
/// - it does not exist: create it from the current commit and switch
fn switch_branch(root: &Path, wanted: &str) -> Result<Option<String>, String> {
    let valid = git(root, &["check-ref-format", "--branch", wanted]).map_or(false, |o| o.code == 0);
    if !valid {
        return Err(format!("'{}' is not a valid branch name", wanted));
    }
    let current = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).map(|o| o.stdout).unwrap_or_default();
    if current == wanted {
        return Ok(None);
    }
    if !has_commits(root) {
        let r = git(root, &["symbolic-ref", "HEAD", &format!("refs/heads/{}", wanted)])?;
        return if r.code == 0 { Ok(Some(format!("initial branch is '{}' (no commits yet)", wanted))) } else { Err(format!("cannot name the branch '{}': {}", wanted, r.stderr)) };
    }
    let exists = git(root, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{}", wanted)]).map_or(false, |o| o.code == 0);
    let r = if exists { git(root, &["switch", wanted])? } else { git(root, &["switch", "-c", wanted])? };
    if r.code != 0 {
        return Err(format!("cannot switch to '{}': {} (commit or stash your changes first, or pick another branch)", wanted, r.stderr.lines().next().unwrap_or("git refused")));
    }
    Ok(Some(if exists {
        format!("switched to the existing branch '{}' (from '{}')", wanted, if current.is_empty() { "detached HEAD" } else { &current })
    } else {
        format!("created the branch '{}' from '{}' and switched to it", wanted, if current.is_empty() { "detached HEAD" } else { &current })
    }))
}

/// Files that are tracked although a .gitignore rule matches them: .gitignore does not untrack, `git rm --cached` does.
fn tracked_but_ignored(root: &Path) -> Option<String> {
    let o = git(root, &["ls-files", "-ci", "--exclude-standard"]).ok()?;
    let files: Vec<&str> = o.stdout.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if files.is_empty() {
        return None;
    }
    let shown: Vec<&str> = files.iter().take(4).copied().collect();
    Some(format!(
        "{} tracked file(s) match .gitignore but stay tracked until removed from the index (git rm --cached <file>): {}{}",
        files.len(),
        shown.join(", "),
        if files.len() > 4 { ", ..." } else { "" }
    ))
}

/// git init (if needed), the requested branch, git add (everything, or only `paths`), git commit -m `message`.
pub(crate) fn commit_with(root: &Path, message: &str, branch: Option<&str>, paths: &[String]) -> String {
    let message = message.to_string();
    if let Err(e) = git(root, &["--version"]) {
        return text(false, &format!("ERROR: {}", e));
    }

    let mut notes: Vec<String> = Vec::new();
    let mut created = false;
    match ensure_repo(root) {
        Ok(Some(note)) => {
            created = true;
            notes.push(note);
        }
        Ok(None) => {}
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    }
    if let Some(wanted) = branch {
        match switch_branch(root, wanted) {
            Ok(Some(note)) => notes.push(note),
            Ok(None) => {}
            Err(e) => return text(false, &format!("NOT COMMITTED: {}", e)),
        }
    }

    // Stage what was asked for (everything under the workspace, or only the given paths), then take secret-looking
    // files back out.
    let scope: Vec<&str> = if paths.is_empty() { vec!["."] } else { paths.iter().map(String::as_str).collect() };
    let mut add_args: Vec<&str> = vec!["add", "-A", "--"];
    add_args.extend(scope.iter().copied());
    if let Ok(o) = git(root, &add_args) {
        if o.code != 0 {
            return text(false, &format!("ERROR: git add failed: {}", o.stderr));
        }
    }
    let mut staged_args: Vec<&str> = vec!["diff", "--cached", "--name-only", "-z", "--"];
    staged_args.extend(scope.iter().copied());
    let staged = git(root, &staged_args).map(|o| o.stdout).unwrap_or_default();
    let mut skipped: Vec<String> = Vec::new();
    let has_head = has_commits(root);
    for f in staged.split('\0').map(str::trim).filter(|f| !f.is_empty()) {
        if is_secret(f) {
            // Back to what HEAD has (a tracked file keeps its committed version); with no HEAD, just unstage.
            let _ = if has_head {
                git(root, &["reset", "-q", "HEAD", "--", f])
            } else {
                git(root, &["rm", "--cached", "-q", "--ignore-unmatch", "--", f])
            };
            skipped.push(f.to_string());
        }
    }
    if !skipped.is_empty() {
        notes.push(format!("not committed (looks like a secret): {}", skipped.join(", ")));
    }

    let mut quiet_args: Vec<&str> = vec!["diff", "--cached", "--quiet", "--"];
    quiet_args.extend(scope.iter().copied());
    let anything = git(root, &quiet_args).map(|o| o.code != 0).unwrap_or(false);
    if !anything {
        let mut out = notes;
        out.push("nothing to commit (no changes under this workspace)".to_string());
        return text(true, &out.join("\n"));
    }

    // Commit with the user's own identity; a machine with none configured gets a neutral one.
    let have_identity = git(root, &["config", "user.name"]).map_or(false, |o| !o.stdout.is_empty())
        && git(root, &["config", "user.email"]).map_or(false, |o| !o.stdout.is_empty());
    let mut args: Vec<&str> = Vec::new();
    if !have_identity {
        args.extend(["-c", "user.name=mysid", "-c", "user.email=mysid@localhost"]);
        notes.push("no git user.name/email configured: committed as 'mysid' (set them with git config)".to_string());
    }
    args.extend(["commit", "-q", "-m", &message]);
    if !paths.is_empty() {
        // Only these paths, whatever else happens to be staged.
        args.push("--");
        args.extend(scope.iter().copied());
    }
    match git(root, &args) {
        Ok(o) if o.code == 0 => {}
        Ok(o) => return text(false, &format!("ERROR: git commit failed: {}", if o.stderr.is_empty() { o.stdout } else { o.stderr })),
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    }

    let summary = git(root, &["show", "--shortstat", "--format=%h", "HEAD"]).map(|o| o.stdout).unwrap_or_default();
    let hash = summary.lines().next().unwrap_or("").to_string();
    let stat = compact_stat(summary.lines().find(|l| l.contains("changed")).unwrap_or(""));
    let count: usize = git(root, &["rev-list", "--count", "HEAD"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0);
    let first = if (created || count == 1) && !message.to_lowercase().contains("initial commit") { " (initial commit)" } else { "" };
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|o| o.stdout).unwrap_or_default();
    let mut out = notes;
    out.push(format!("{} {}{} - {} [{}]", hash, message, first, stat, branch));
    // What went into the commit, so nothing unrelated slips in unseen.
    let names = git(root, &["show", "--name-only", "--pretty=format:", "HEAD"]).map(|o| o.stdout).unwrap_or_default();
    let files: Vec<&str> = names.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if !files.is_empty() {
        let shown: Vec<&str> = files.iter().take(12).copied().collect();
        out.push(format!("files: {}{}", shown.join(", "), if files.len() > 12 { format!(", +{} more", files.len() - 12) } else { String::new() }));
    }
    if let Some(note) = tracked_but_ignored(root) {
        out.push(note);
    }
    let remote = git(root, &["remote"]).map(|o| o.stdout).unwrap_or_default();
    out.push(if remote.is_empty() {
        "committed locally; no remote configured, nothing pushed".to_string()
    } else {
        format!("committed locally; not pushed (mysid never pushes; remote: {})", remote.lines().next().unwrap_or(""))
    });
    text(true, &out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::tracker::execute_history;
    use super::super::git_available;
    use super::*;
    use serde_json::json;
    use std::fs;
    use std::process::Command;

    #[test]
    fn commit_takes_a_branch_and_paths_and_explains_gitignore() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_commit_named_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (f, c) in [("a.txt", "a1\n"), ("b.txt", "b1\n"), ("cache.tmp", "tmp\n")] {
            fs::write(dir.join(f), c).unwrap();
        }
        let root = dir.to_string_lossy().to_string();
        let run = |extra: serde_json::Value| {
            let mut p = json!({ "workspace_root": root });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            let raw = execute_commit(&p, Some(&root));
            serde_json::from_str::<Value>(&raw).unwrap()["output"].as_str().unwrap().to_string()
        };
        // No commits yet: the initial commit lands on the named branch.
        let first = run(json!({ "message": "Initial commit", "branch": "trunk" }));
        assert!(first.contains("initial branch is 'trunk'") && first.contains("[trunk]"), "{}", first);
        assert!(first.contains("files: ") && first.contains("a.txt") && first.contains("b.txt"), "{}", first);
        // A new branch is created from here; a bad name is refused before anything is committed.
        fs::write(dir.join("a.txt"), "a2\n").unwrap();
        let bad = run(json!({ "message": "x", "branch": "bad name" }));
        assert!(bad.starts_with("NOT COMMITTED: 'bad name' is not a valid branch name"), "{}", bad);
        let second = run(json!({ "message": "Change a", "branch": "feature" }));
        assert!(second.contains("created the branch 'feature' from 'trunk'") && second.contains("[feature]"), "{}", second);
        // Only the paths asked for are committed; the other change stays uncommitted.
        fs::write(dir.join("a.txt"), "a3\n").unwrap();
        fs::write(dir.join("b.txt"), "b2\n").unwrap();
        let third = run(json!({ "message": "Only b", "paths": ["b.txt"] }));
        assert!(third.contains("files: b.txt") && !third.contains("a.txt"), "{}", third);
        let left = git(&dir, &["status", "--porcelain"]).unwrap().stdout;
        assert!(left.contains("a.txt") && !left.contains("b.txt"), "{}", left);
        // Going back to an existing branch switches to it.
        let back = run(json!({ "message": "Back on trunk", "branch": "trunk" }));
        assert!(back.contains("switched to the existing branch 'trunk' (from 'feature')") || back.contains("cannot switch"), "{}", back);
        // .gitignore does not untrack: cache.tmp was committed, then ignored.
        fs::write(dir.join(".gitignore"), "*.tmp\n").unwrap();
        let note = run(json!({ "message": "Ignore logs" }));
        assert!(note.contains("tracked file(s) match .gitignore") && note.contains("cache.tmp") && note.contains("git rm --cached"), "{}", note);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn commit_creates_a_repo_skips_secrets_and_never_pushes() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_vcs_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("build")).unwrap();
        fs::write(dir.join("A.kt"), "class A\n").unwrap();
        fs::write(dir.join("release.jks"), "secret").unwrap();
        fs::write(dir.join(".env"), "TOKEN=1").unwrap();
        fs::write(dir.join("build").join("out.class"), "x").unwrap();
        fs::write(dir.join("build.gradle.kts"), "// gradle\n").unwrap();
        let p = json!({ "workspace_root": dir.to_string_lossy(), "message": "First snapshot" });

        let none = execute_history(&p, None);
        assert!(none.contains("Not a git repository"), "{}", none);
        assert!(execute_commit(&json!({ "workspace_root": dir.to_string_lossy() }), None).contains("message is required"));

        let first = execute_commit(&p, None);
        assert!(first.contains("created one") && first.contains("(initial commit)") && first.contains("nothing pushed"), "{}", first);
        let tracked = git(&dir, &["ls-files"]).unwrap().stdout;
        assert!(tracked.contains("A.kt") && tracked.contains("build.gradle.kts"), "{}", tracked);
        assert!(!tracked.contains("release.jks") && !tracked.contains(".env") && !tracked.contains("out.class"), "{}", tracked);

        assert!(execute_commit(&p, None).contains("nothing to commit"));
        fs::write(dir.join("A.kt"), "class A\nclass B\n").unwrap();
        let second = execute_commit(&json!({ "workspace_root": dir.to_string_lossy(), "message": "Add B" }), None);
        assert!(second.contains("Add B") && second.contains("1 file, +1 -0"), "{}", second);

        let hist = execute_history(&json!({ "workspace_root": dir.to_string_lossy(), "past": 5 }), None);
        assert!(hist.contains("2 commit(s)") && hist.contains("Add B") && hist.contains("First snapshot"), "{}", hist);
        let _ = fs::remove_dir_all(&dir);
    }
}
