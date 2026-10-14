//! `mysid checkpoint`, `mysid diff`, `mysid restore`: snapshot, review and roll back working-tree changes.
//!
//! A checkpoint is a commit stored under `refs/mysid/checkpoints/<n>`: it never moves a branch, never touches the
//! index or HEAD, and does not show up in `git log`. It captures the working tree under the workspace (new files
//! included, ignored files and secret-looking files excluded) by building a tree in a private temporary index.
//! `diff` compares the working tree with a checkpoint (or HEAD); `restore` puts the files back, previews first,
//! and takes a safety checkpoint before it changes anything.

use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::{commit_all, ensure_repo, git, git_env, has_commits, in_repo, is_secret};
use crate::tools::workspace::resolve_existing_root;

const REF_PREFIX: &str = "refs/mysid/checkpoints/";
/// git's empty tree: the "before" of a repository with no commits.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
const HUNK_LINES_PER_FILE: usize = 30;
const HUNK_LINES_TOTAL: usize = 150;
const PLAN_FILES_SHOWN: usize = 40;

fn text(success: bool, output: &str) -> String {
    json!({ "success": success, "output": output }).to_string()
}

struct Checkpoint {
    n: u32,
    sha: String,
    age: String,
    label: String,
}

fn checkpoints(root: &Path) -> Vec<Checkpoint> {
    let out = git(root, &["for-each-ref", REF_PREFIX, "--format=%(refname)|%(objectname)|%(creatordate:relative)|%(subject)"])
        .map(|o| o.stdout)
        .unwrap_or_default();
    let mut v: Vec<Checkpoint> = out
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(4, '|');
            let n = p.next()?.strip_prefix(REF_PREFIX)?.parse().ok()?;
            let sha = p.next()?.to_string();
            let age = p.next()?.to_string();
            let subject = p.next().unwrap_or("");
            Some(Checkpoint { n, sha, age, label: subject.strip_prefix("mysid checkpoint: ").unwrap_or(subject).to_string() })
        })
        .collect();
    v.sort_by(|a, b| b.n.cmp(&a.n));
    v
}

fn prefix_of(root: &Path) -> String {
    git(root, &["rev-parse", "--show-prefix"]).map(|o| o.stdout).unwrap_or_default()
}

/// Largest single file a checkpoint stores; bigger ones are left out and listed.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// More files than this and the folder is too big to snapshot as a whole.
const MAX_FILES: usize = 20_000;
const HASH_BATCH: usize = 400;

/// A snapshot of the working tree: the tree id and the large files that were left out.
struct Snapshot {
    tree: String,
    skipped: Vec<String>,
}

/// `git` with bytes on stdin (the pipe is fed from a thread so a large input cannot deadlock the output).
fn git_stdin(root: &Path, args: &[&str], env: &[(&str, &str)], input: Vec<u8>) -> Result<(i32, String, String), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("cannot run git: {}", e))?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let feeder = std::thread::spawn(move || {
        let _ = stdin.write_all(&input);
    });
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let _ = feeder.join();
    Ok((
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).trim_end().to_string(),
        String::from_utf8_lossy(&out.stderr).trim_end().to_string(),
    ))
}

/// Tree of the working tree under `root`: modified and untracked files, but not ignored, secret-looking or very
/// large ones. File contents are stored byte for byte (no line-ending or attribute conversion), so a checkpoint
/// and a restore never alter a file, whatever `core.autocrlf`, `core.safecrlf` or `.gitattributes` say. The tree is
/// built in a private index, so the real index, HEAD and branches are untouched.
fn snapshot_tree(root: &Path) -> Result<Snapshot, String> {
    snapshot_tree_limited(root, MAX_FILES, MAX_FILE_BYTES)
}

fn snapshot_tree_limited(root: &Path, max_files: usize, max_bytes: u64) -> Result<Snapshot, String> {
    let listed = git(root, &["ls-files", "-z", "--cached", "--others", "--exclude-standard", "--", "."])?;
    if listed.code != 0 {
        return Err(format!("git ls-files failed: {}", listed.stderr));
    }
    let mut files: Vec<String> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for f in listed.stdout.split('\0').filter(|f| !f.is_empty()) {
        if !seen.insert(f.to_string()) || is_secret(f) || f.contains('\n') {
            continue;
        }
        // Tracked files that were deleted, directories and symlinks are not part of the snapshot.
        let Ok(meta) = fs::symlink_metadata(root.join(f)) else { continue };
        if !meta.is_file() {
            continue;
        }
        if meta.len() > max_bytes {
            skipped.push(format!("{} ({} MB)", f, meta.len() / (1024 * 1024)));
            continue;
        }
        files.push(f.to_string());
    }
    if files.len() > max_files {
        return Err(format!(
            "{} files under this workspace is too many to snapshot at once (limit {}). Run it on the part you are changing: \
             mysid checkpoint --workspace_root <subfolder>  (and diff/restore with the same folder).",
            files.len(),
            max_files
        ));
    }

    let git_dir = git(root, &["rev-parse", "--absolute-git-dir"])?.stdout;
    let prefix = prefix_of(root);
    let idx: PathBuf = Path::new(&git_dir).join(format!("mysid-index-{}", std::process::id()));
    let _ = fs::remove_file(&idx);
    let idx_str = idx.to_string_lossy().to_string();
    let env = [("GIT_INDEX_FILE", idx_str.as_str())];
    let result = (|| {
        for batch in files.chunks(HASH_BATCH) {
            // Store each file's bytes as a blob without any conversion...
            // Absolute paths: git resolves `--stdin-paths` from the repository top, not from the current folder.
            let paths: String = batch.iter().map(|f| format!("{}\n", root.join(f).to_string_lossy())).collect();
            let (code, shas, err) = git_stdin(root, &["hash-object", "-w", "--no-filters", "--stdin-paths"], &[], paths.into_bytes())?;
            if code != 0 {
                return Err(format!("git hash-object failed: {}", first_line(&err)));
            }
            let shas: Vec<&str> = shas.lines().collect();
            if shas.len() != batch.len() {
                return Err("git hash-object returned an unexpected result".to_string());
            }
            // ...and record them in the private index (paths there are relative to the repository root).
            let mut info = Vec::new();
            for (path, sha) in batch.iter().zip(shas) {
                info.extend_from_slice(format!("100644 {}\t{}{}", sha, prefix, path).as_bytes());
                info.push(0);
            }
            let (code, _, err) = git_stdin(root, &["update-index", "--add", "-z", "--index-info"], &env, info)?;
            if code != 0 {
                return Err(format!("git update-index failed: {}", first_line(&err)));
            }
        }
        let tree = git_env(root, &["write-tree"], &env)?;
        if tree.code != 0 {
            return Err(format!("git write-tree failed: {}", first_line(&tree.stderr)));
        }
        Ok(tree.stdout)
    })();
    let _ = fs::remove_file(&idx);
    result.map(|tree| Snapshot { tree, skipped })
}

fn first_line(s: &str) -> &str {
    s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim()
}

/// A user-facing target resolved to (tree id, description): `last`/empty, a checkpoint number, `HEAD`, or any git revision.
fn resolve_target(root: &Path, target: &str) -> Result<(String, String), String> {
    let t = target.trim();
    let cps = checkpoints(root);
    if t.is_empty() || t.eq_ignore_ascii_case("last") {
        // The automatic "before restore" checkpoints are for undoing; "last" means the one the user made.
        if let Some(c) = cps.iter().find(|c| !c.label.starts_with("auto:")).or_else(|| cps.first()) {
            return Ok((tree_of(root, &c.sha)?, describe(c)));
        }
        return head_tree(root).map(|t| (t, "HEAD (no checkpoint yet)".to_string()));
    }
    if let Ok(n) = t.trim_start_matches('#').parse::<u32>() {
        return match cps.iter().find(|c| c.n == n) {
            Some(c) => Ok((tree_of(root, &c.sha)?, describe(c))),
            None => Err(format!("no checkpoint #{} (see: mysid checkpoint list)", n)),
        };
    }
    if t.eq_ignore_ascii_case("head") {
        return head_tree(root).map(|t| (t, "HEAD".to_string()));
    }
    if t.starts_with('-') {
        return Err(format!("'{}' is not a checkpoint number or a git revision", t));
    }
    let spec = format!("{}^{{tree}}", t);
    match git(root, &["rev-parse", "--verify", "-q", &spec]) {
        Ok(o) if o.code == 0 => Ok((o.stdout, t.to_string())),
        _ => Err(format!("'{}' is not a checkpoint number or a git revision", t)),
    }
}

fn describe(c: &Checkpoint) -> String {
    format!("checkpoint #{}{} ({})", c.n, if c.label.is_empty() { String::new() } else { format!(" \"{}\"", c.label) }, c.age)
}

fn tree_of(root: &Path, commit: &str) -> Result<String, String> {
    let spec = format!("{}^{{tree}}", commit);
    let o = git(root, &["rev-parse", "--verify", "-q", &spec])?;
    if o.code == 0 { Ok(o.stdout) } else { Err(format!("cannot read {}", commit)) }
}

fn head_tree(root: &Path) -> Result<String, String> {
    if !has_commits(root) {
        return Ok(EMPTY_TREE.to_string());
    }
    tree_of(root, "HEAD")
}

/// Stores a checkpoint of the current working tree. Returns (number, files captured).
fn create_checkpoint(root: &Path, label: &str) -> Result<(u32, usize, Vec<String>), String> {
    let snap = snapshot_tree(root)?;
    let tree = snap.tree;
    let n = checkpoints(root).first().map_or(1, |c| c.n + 1);
    let mut args: Vec<String> = Vec::new();
    let have_identity = git(root, &["config", "user.name"]).map_or(false, |o| !o.stdout.is_empty())
        && git(root, &["config", "user.email"]).map_or(false, |o| !o.stdout.is_empty());
    if !have_identity {
        args.extend(["-c", "user.name=mysid", "-c", "user.email=mysid@localhost"].map(String::from));
    }
    args.extend(["commit-tree".to_string(), tree.clone(), "-m".to_string(), format!("mysid checkpoint: {}", label)]);
    if has_commits(root) {
        args.extend(["-p".to_string(), "HEAD".to_string()]);
    }
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let commit = git(root, &argv)?;
    if commit.code != 0 {
        return Err(format!("git commit-tree failed: {}", commit.stderr));
    }
    let refname = format!("{}{}", REF_PREFIX, n);
    let upd = git(root, &["update-ref", &refname, &commit.stdout])?;
    if upd.code != 0 {
        return Err(format!("git update-ref failed: {}", upd.stderr));
    }
    let files = git(root, &["ls-tree", "-r", "--name-only", &tree]).map(|o| o.stdout.lines().count()).unwrap_or(0);
    Ok((n, files, snap.skipped))
}

fn root_of(params: &Value, default_root: Option<&str>) -> Result<PathBuf, String> {
    let ws = resolve_existing_root(params, "workspace_root", default_root)?;
    let root = PathBuf::from(ws);
    if let Err(e) = git(&root, &["--version"]) {
        return Err(text(false, &format!("ERROR: {}", e)));
    }
    Ok(root)
}

pub fn execute_checkpoint(params: &Value, default_root: Option<&str>) -> String {
    let root = match root_of(params, default_root) {
        Ok(r) => r,
        Err(e) => return e,
    };
    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();

    if query.eq_ignore_ascii_case("list") {
        if !in_repo(&root) {
            return text(true, "No checkpoints (not a git repository yet). `mysid checkpoint` creates one.");
        }
        let cps = checkpoints(&root);
        if cps.is_empty() {
            return text(true, "No checkpoints yet. `mysid checkpoint \"label\"` saves one.");
        }
        let lines: Vec<String> = cps.iter().map(|c| format!("#{:<3} {:<16} {}", c.n, c.age, if c.label.is_empty() { "(no label)" } else { &c.label })).collect();
        return text(true, &format!("{} checkpoint(s), newest first:\n{}", cps.len(), lines.join("\n")));
    }

    let mut notes = Vec::new();
    let label = if query.is_empty() { "checkpoint".to_string() } else { query };
    match ensure_repo(&root) {
        Ok(Some(n)) => {
            notes.push(n);
            // A new repository gets its first commit too (git add, git commit -m <label>), so it has a HEAD to diff against.
            let first = commit_all(&root, &label);
            let line = serde_json::from_str::<serde_json::Value>(&first)
                .ok()
                .and_then(|v| v.get("output").and_then(|o| o.as_str()).map(String::from))
                .unwrap_or_default();
            notes.extend(line.lines().filter(|l| !l.starts_with("committed locally")).map(|l| format!("initial commit: {}", l)));
        }
        Ok(None) => {}
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    }
    match create_checkpoint(&root, &label) {
        Ok((n, files, skipped)) => {
            notes.push(format!(
                "checkpoint #{} \"{}\": {} file(s) saved (branch and index untouched; secrets and ignored files left out)\nreview: mysid diff    roll back: mysid restore {} --yes",
                n, label, files, n
            ));
            if !skipped.is_empty() {
                let shown: Vec<&str> = skipped.iter().take(5).map(String::as_str).collect();
                notes.push(format!(
                    "left out ({} file(s) over {} MB, not part of this checkpoint, diff or restore): {}{}",
                    skipped.len(),
                    MAX_FILE_BYTES / (1024 * 1024),
                    shown.join(", "),
                    if skipped.len() > 5 { ", ..." } else { "" }
                ));
            }
            text(true, &notes.join("\n"))
        }
        Err(e) => text(false, &format!("ERROR: {}", e)),
    }
}

struct Change {
    status: char,
    path: String,
}

/// Files that differ between `target` and the working tree (workspace-relative paths).
fn changes_since(root: &Path, target_tree: &str, now_tree: &str) -> Result<Vec<Change>, String> {
    let o = git(root, &["diff", "--name-status", "--no-renames", "--relative", "-z", target_tree, now_tree, "--", "."])?;
    if o.code != 0 {
        return Err(format!("git diff failed: {}", o.stderr));
    }
    let mut parts = o.stdout.split('\0').filter(|p| !p.is_empty());
    let mut out = Vec::new();
    while let (Some(st), Some(path)) = (parts.next(), parts.next()) {
        out.push(Change { status: st.chars().next().unwrap_or('M'), path: path.to_string() });
    }
    Ok(out)
}

pub fn execute_diff(params: &Value, default_root: Option<&str>) -> String {
    let root = match root_of(params, default_root) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !in_repo(&root) {
        return text(true, "Not a git repository yet: nothing to compare. `mysid checkpoint` or `mysid commit \"msg\"` creates one.");
    }
    let target = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let (before, label) = match resolve_target(&root, target) {
        Ok(t) => t,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    let now = match snapshot_tree(&root) {
        Ok(t) => t.tree,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    let changes = match changes_since(&root, &before, &now) {
        Ok(c) => c,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    if changes.is_empty() {
        return text(true, &format!("No changes since {}.", label));
    }

    // Per-file line counts.
    let numstat = git(&root, &["diff", "--numstat", "--no-renames", "--relative", &before, &now, "--", "."]).map(|o| o.stdout).unwrap_or_default();
    let counts: std::collections::HashMap<String, (String, String)> = numstat
        .lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let (a, d, path) = (p.next()?, p.next()?, p.next()?);
            Some((path.to_string(), (a.to_string(), d.to_string())))
        })
        .collect();
    let (mut add_total, mut del_total) = (0usize, 0usize);
    let mut lines = Vec::new();
    for c in &changes {
        let (a, d) = counts.get(&c.path).cloned().unwrap_or_default();
        add_total += a.parse::<usize>().unwrap_or(0);
        del_total += d.parse::<usize>().unwrap_or(0);
        let stat = if a == "-" {
            "binary".to_string()
        } else {
            match c.status {
                'A' => format!("+{}", a),
                'D' => format!("-{}", d),
                _ => format!("+{} -{}", a, d),
            }
        };
        lines.push(format!(" {}  {}  {}", c.status, c.path, stat));
    }
    let mut out = format!("diff vs {}: {} file(s), +{} -{}\n{}\n", label, changes.len(), add_total, del_total, lines.join("\n"));

    // Hunks, trimmed: a few lines per file and a total cap.
    let patch = git(&root, &["diff", "-U2", "--no-renames", "--relative", "--no-color", &before, &now, "--", "."]).map(|o| o.stdout).unwrap_or_default();
    let (mut shown_total, mut hidden) = (0usize, 0usize);
    let mut shown_in_file = 0usize;
    for l in patch.lines() {
        if let Some(rest) = l.strip_prefix("diff --git ") {
            let name = rest.rsplit(" b/").next().unwrap_or(rest);
            shown_in_file = 0;
            if shown_total < HUNK_LINES_TOTAL {
                out.push_str(&format!("--- {}\n", name));
            }
            continue;
        }
        if l.starts_with("index ") || l.starts_with("--- ") || l.starts_with("+++ ") || l.starts_with("new file") || l.starts_with("deleted file") || l.starts_with("Binary files") {
            continue;
        }
        if shown_in_file >= HUNK_LINES_PER_FILE || shown_total >= HUNK_LINES_TOTAL {
            hidden += 1;
            continue;
        }
        out.push_str(l);
        out.push('\n');
        shown_in_file += 1;
        shown_total += 1;
    }
    if hidden > 0 {
        out.push_str(&format!("... {} more diff line(s) hidden (read the files, or git diff, for everything)\n", hidden));
    }
    out.push_str("(new files are included; ignored and secret-looking files are not)");
    text(true, &out)
}

pub fn execute_restore(params: &Value, default_root: Option<&str>) -> String {
    let root = match root_of(params, default_root) {
        Ok(r) => r,
        Err(e) => return e,
    };
    if !in_repo(&root) {
        return text(false, "ERROR: not a git repository, so there is nothing to restore from.");
    }
    let yes = params.get("yes").map_or(false, |v| v.as_bool().unwrap_or(true));
    let target = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let (before, label) = match resolve_target(&root, target) {
        Ok(t) => t,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    let now = match snapshot_tree(&root) {
        Ok(t) => t.tree,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    let changes = match changes_since(&root, &before, &now) {
        Ok(c) => c,
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    };
    if changes.is_empty() {
        return text(true, &format!("Nothing to restore: the working tree already matches {}.", label));
    }
    if changes.iter().any(|c| Path::new(&c.path).components().any(|p| matches!(p, Component::ParentDir))) {
        return text(false, "ERROR: refusing paths outside the workspace.");
    }

    // 'A' = added since the target (will be deleted), 'M' = changed (content restored), 'D' = deleted since (recreated).
    let describe_change = |c: &Change| match c.status {
        'A' => format!(" delete   {}", c.path),
        'D' => format!(" recreate {}", c.path),
        _ => format!(" restore  {}", c.path),
    };
    let mut plan: Vec<String> = changes.iter().take(PLAN_FILES_SHOWN).map(describe_change).collect();
    if changes.len() > PLAN_FILES_SHOWN {
        plan.push(format!(" ... +{} more", changes.len() - PLAN_FILES_SHOWN));
    }
    if !yes {
        return text(
            true,
            &format!(
                "Would restore {} file(s) to {}:\n{}\nNothing changed. Run again with --yes to apply (a safety checkpoint is saved first, so this can be undone).",
                changes.len(),
                label,
                plan.join("\n")
            ),
        );
    }

    let safety = match create_checkpoint(&root, "auto: before restore") {
        Ok((n, _, _)) => n,
        Err(e) => return text(false, &format!("ERROR: could not save the safety checkpoint, so nothing was changed: {}", e)),
    };
    let prefix = prefix_of(&root);
    let (mut restored, mut deleted) = (0usize, 0usize);
    for c in &changes {
        let target_path = root.join(&c.path);
        if c.status == 'A' {
            if fs::remove_file(&target_path).is_ok() {
                deleted += 1;
                remove_empty_parents(&target_path, &root);
            }
        } else {
            let spec = format!("{}:{}{}", before, prefix, c.path);
            let bytes = match std::process::Command::new("git").args(["cat-file", "blob", &spec]).current_dir(&root).output() {
                Ok(o) if o.status.success() => o.stdout,
                _ => return text(false, &format!("ERROR: could not read {} from {}; stopped. Undo what was done with: mysid restore {} --yes", c.path, label, safety)),
            };
            if let Some(dir) = target_path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            if let Err(e) = fs::write(&target_path, bytes) {
                return text(false, &format!("ERROR: cannot write {}: {}. Undo what was done with: mysid restore {} --yes", c.path, e, safety));
            }
            restored += 1;
        }
    }
    let verified = snapshot_tree(&root).map_or(false, |t| t.tree == before);
    text(
        true,
        &format!(
            "Restored to {}: {} file(s) written, {} new file(s) removed{}.\nundo: mysid restore {} --yes   (state before this restore)",
            label,
            restored,
            deleted,
            if verified { ", working tree now matches it exactly" } else { " (some files differ: ignored or secret-looking files are never touched)" },
            safety
        ),
    )
}

fn remove_empty_parents(file: &Path, root: &Path) {
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d == root || !d.starts_with(root) || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_available() -> bool {
        std::process::Command::new("git").arg("--version").output().map_or(false, |o| o.status.success())
    }

    fn run(f: fn(&Value, Option<&str>) -> String, dir: &Path, query: &str, yes: bool) -> String {
        let r = f(&json!({ "workspace_root": dir.to_string_lossy(), "query": query, "yes": yes }), None);
        serde_json::from_str::<Value>(&r).unwrap()["output"].as_str().unwrap().to_string()
    }

    #[test]
    fn checkpoint_diff_and_restore_round_trip() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_cp_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/A.kt"), "class A\n").unwrap();
        fs::write(dir.join("C.txt"), "keep me\n").unwrap();
        fs::write(dir.join(".env"), "TOKEN=1\n").unwrap();

        let made = run(execute_checkpoint, &dir, "base", false);
        assert!(made.contains("created one") && made.contains("checkpoint #1 \"base\": 2 file(s)"), "{}", made);
        assert!(run(execute_diff, &dir, "", false).contains("No changes since checkpoint #1"));

        fs::write(dir.join("src/A.kt"), "class A\nclass B\n").unwrap();
        fs::write(dir.join("New.kt"), "class N\n").unwrap();
        fs::remove_file(dir.join("C.txt")).unwrap();
        fs::write(dir.join(".env"), "TOKEN=changed\n").unwrap();
        let d = run(execute_diff, &dir, "", false);
        assert!(d.contains("3 file(s)") && d.contains(" M  src/A.kt  +1 -0") && d.contains(" A  New.kt  +1") && d.contains(" D  C.txt  -1"), "{}", d);
        assert!(d.contains("+class B") && !d.contains("TOKEN"), "{}", d);

        let plan = run(execute_restore, &dir, "1", false);
        assert!(plan.contains("Would restore 3") && plan.contains("delete   New.kt") && plan.contains("recreate C.txt"), "{}", plan);
        assert!(dir.join("New.kt").exists(), "a preview changes nothing");

        let done = run(execute_restore, &dir, "1", true);
        assert!(done.contains("2 file(s) written, 1 new file(s) removed") && done.contains("matches it exactly"), "{}", done);
        assert_eq!(fs::read_to_string(dir.join("src/A.kt")).unwrap(), "class A\n");
        assert_eq!(fs::read_to_string(dir.join("C.txt")).unwrap(), "keep me\n");
        assert!(!dir.join("New.kt").exists());
        assert_eq!(fs::read_to_string(dir.join(".env")).unwrap(), "TOKEN=changed\n", "secrets are never touched");

        // The safety checkpoint (#2) holds the state before the restore, so it can be undone.
        let list = run(execute_checkpoint, &dir, "list", false);
        assert!(list.contains("2 checkpoint(s)") && list.contains("auto: before restore"), "{}", list);
        run(execute_restore, &dir, "2", true);
        assert!(dir.join("New.kt").exists() && !dir.join("C.txt").exists());

        assert!(run(execute_restore, &dir, "9", false).contains("no checkpoint #9"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_workspace_inside_a_larger_repo_only_sees_its_own_files() {
        if !git_available() {
            return;
        }
        let top = std::env::temp_dir().join(format!("mysid_cp_sub_{}", std::process::id()));
        let _ = fs::remove_dir_all(&top);
        fs::create_dir_all(top.join("sub")).unwrap();
        fs::write(top.join("outside.txt"), "o\n").unwrap();
        fs::write(top.join("sub/in.txt"), "i\n").unwrap();
        let out = std::process::Command::new("git").args(["init", "-q"]).current_dir(&top).output().unwrap();
        assert!(out.status.success());
        let sub = top.join("sub");

        run(execute_checkpoint, &sub, "s", false);
        fs::write(sub.join("in.txt"), "changed\n").unwrap();
        fs::write(top.join("outside.txt"), "changed too\n").unwrap();
        let d = run(execute_diff, &sub, "", false);
        assert!(d.contains("1 file(s)") && d.contains("in.txt") && !d.contains("outside"), "{}", d);
        run(execute_restore, &sub, "", true);
        assert_eq!(fs::read_to_string(sub.join("in.txt")).unwrap(), "i\n");
        assert_eq!(fs::read_to_string(top.join("outside.txt")).unwrap(), "changed too\n", "files outside the workspace are never touched");
        let _ = fs::remove_dir_all(&top);
    }

    #[test]
    fn strict_line_ending_settings_do_not_break_checkpoints_and_restore_keeps_every_byte() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_cp_crlf_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let g = |args: &[&str]| assert!(std::process::Command::new("git").args(args).current_dir(&dir).output().unwrap().status.success(), "{:?}", args);
        g(&["init", "-q"]);
        g(&["config", "core.autocrlf", "true"]);
        g(&["config", "core.safecrlf", "true"]); // `git add` fails on a file with bare LF: the reported failure
        fs::write(dir.join(".gitattributes"), "*.cs text eol=crlf\n").unwrap();
        let mixed: &[u8] = b"line1\r\nline2\nline3\r\n";
        let lf: &[u8] = b"a\nb\n";
        fs::write(dir.join("mixed.cs"), mixed).unwrap();
        fs::write(dir.join("lf.cs"), lf).unwrap();

        let made = run(execute_checkpoint, &dir, "base", false);
        assert!(made.contains("checkpoint #1 \"base\""), "{}", made);

        fs::write(dir.join("mixed.cs"), b"changed\n").unwrap();
        fs::write(dir.join("lf.cs"), b"x\r\n").unwrap();
        assert!(run(execute_diff, &dir, "", false).contains("2 file(s)"));
        run(execute_restore, &dir, "1", true);
        assert_eq!(fs::read(dir.join("mixed.cs")).unwrap(), mixed, "mixed endings come back byte for byte");
        assert_eq!(fs::read(dir.join("lf.cs")).unwrap(), lf);
        assert!(run(execute_diff, &dir, "", false).contains("No changes"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn large_files_are_left_out_and_too_many_files_get_a_clear_message() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_cp_big_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&dir).output().unwrap().status.success());
        for n in 0..4 {
            fs::write(dir.join(format!("f{}.txt", n)), "x\n").unwrap();
        }
        fs::write(dir.join("big.bin"), vec![b'z'; 3000]).unwrap();
        let snap = snapshot_tree_limited(&dir, 100, 1000).unwrap();
        assert_eq!(snap.skipped.len(), 1);
        assert!(snap.skipped[0].starts_with("big.bin"), "{:?}", snap.skipped);
        let err = snapshot_tree_limited(&dir, 3, 1000).err().unwrap();
        assert!(err.contains("too many") && err.contains("--workspace_root <subfolder>"), "{}", err);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn first_checkpoint_in_a_folder_without_git_also_makes_the_initial_commit() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_cp_first_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), "{}\n").unwrap();
        fs::write(dir.join("a.txt"), "hi\n").unwrap();
        let out = run(execute_checkpoint, &dir, "start", false);
        assert!(out.contains("created one") && out.contains("initial commit") && out.contains("checkpoint #1"), "{}", out);
        let log = git(&dir, &["log", "--format=%s"]).unwrap().stdout;
        assert_eq!(log.trim(), "start");
        let _ = fs::remove_dir_all(&dir);
    }
}
