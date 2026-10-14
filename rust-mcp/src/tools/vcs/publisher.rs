//! Publisher: the first upload of a project to a git remote (`mysid publish`). A preview unless `--yes`.

use serde_json::Value;
use std::path::Path;
use std::process::Command;

use super::committer::commit_all;
use super::initializer::ensure_repo;
use super::{git, has_commits, in_repo, is_secret, text, Out};
use crate::tools::workspace::resolve_existing_root;

/// `owner/repo` or a full URL -> the URL git will use. None for anything else (a local path is not a remote to publish to).
fn normalize_remote(raw: &str) -> Option<String> {
    let r = raw.trim().trim_matches('"');
    if r.starts_with("https://") || r.starts_with("http://") || r.starts_with("ssh://") || r.starts_with("git@") || r.starts_with("file://") {
        return Some(r.to_string());
    }
    let shorthand = r.split('/').count() == 2
        && r.split('/').all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'));
    shorthand.then(|| format!("https://github.com/{}.git", r))
}

/// The browsable address of a remote (`git@github.com:o/r.git` -> `https://github.com/o/r`), for the answer.
fn web_url(url: &str) -> String {
    let u = url.trim_end_matches(".git");
    match u.strip_prefix("git@").and_then(|s| s.split_once(':')) {
        Some((host, path)) => format!("https://{}/{}", host, path),
        None => u.to_string(),
    }
}

/// `git push` with a time limit: a credential prompt that never gets an answer must not hang the caller.
fn git_push(root: &Path, name: &str, branch: &str, secs: u64) -> Result<Out, String> {
    let mut cmd = Command::new("git");
    cmd.args(["push", "-u", name, branch]).current_dir(root).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let run = crate::tools::exec::run_with_timeout(cmd, std::time::Duration::from_secs(secs)).map_err(|e| format!("git is not available ({}).", e))?;
    if run.timed_out {
        return Err(format!("the push did not finish in {}s (a sign-in window may be waiting for you, or the network is slow); nothing is lost, run it again", secs));
    }
    Ok(Out { code: run.exit_code, stdout: run.stdout.trim_end().to_string(), stderr: run.stderr.trim_end().to_string() })
}

/// What went wrong with a push, in a sentence the caller can act on, plus git's own last lines.
fn push_failure(stderr: &str) -> String {
    let low = stderr.to_lowercase();
    let why = if low.contains("repository not found") || low.contains("not found") && low.contains("repository") {
        "the repository was not found: check the address, and create the repository on GitHub first (empty, without a README)"
    } else if low.contains("authentication failed") || low.contains("could not read username") || low.contains("permission denied") || low.contains("terminal prompts disabled") {
        "git could not sign in. HTTPS: sign in once with git's credential manager (run `git push` in a terminal), or use the SSH address (git@github.com:owner/repo.git) with your key set up"
    } else if low.contains("rejected") || low.contains("fetch first") || low.contains("non-fast-forward") {
        "the remote already has different commits. mysid never overwrites them (no force push): publish to an empty repository, or fetch and merge in git yourself"
    } else {
        "git refused the push"
    };
    let tail: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{}\n  {}", why, tail.join("\n  "))
}

/// `mysid publish <repo-url>`: the first upload of a project to GitHub (or any git remote). Shows the plan unless `--yes`.
/// Adds the remote, makes the first commit if there are none, and pushes the current branch. Never forces, never
/// overwrites a remote that already has other commits, and refuses when secret-looking files are in the history.
pub fn execute_publish(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    let raw = ["url", "query", "repo", "remote_url"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).map(str::trim).filter(|s| !s.is_empty());
    let Some(raw) = raw else {
        return text(false, "NOT PUBLISHED: give the repository: mysid publish https://github.com/owner/repo.git   (or owner/repo)");
    };
    let Some(url) = normalize_remote(raw) else {
        return text(false, &format!("NOT PUBLISHED: '{}' is not a repository address. Use https://github.com/owner/repo.git, git@github.com:owner/repo.git, or owner/repo.", raw));
    };
    let yes = ["yes", "confirm", "go"].iter().any(|k| match params.get(*k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "" | "false" | "0" | "no"),
        _ => false,
    });
    let name = params.get("name").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).unwrap_or("origin").to_string();
    if let Err(e) = git(root, &["--version"]) {
        return text(false, &format!("ERROR: {}", e));
    }

    let repo = in_repo(root);
    let commits = repo && has_commits(root);
    let count: usize = if commits { git(root, &["rev-list", "--count", "HEAD"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0) } else { 0 };
    let branch = if commits { git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|o| o.stdout).unwrap_or_default() } else { "main".to_string() };
    let uncommitted = if repo { git(root, &["status", "--porcelain", "--", "."]).map(|o| o.stdout.lines().count()).unwrap_or(0) } else { 0 };
    let existing = if repo { git(root, &["remote", "get-url", &name]).ok().filter(|o| o.code == 0).map(|o| o.stdout) } else { None };

    // Secret-looking files anywhere in what would be uploaded (the whole history goes with a push).
    let mut secrets: Vec<String> = Vec::new();
    if commits {
        for args in [vec!["ls-files"], vec!["log", "--name-only", "--pretty=format:"]] {
            if let Ok(o) = git(root, &args) {
                for f in o.stdout.lines().map(str::trim).filter(|f| !f.is_empty()) {
                    if is_secret(f) && !secrets.iter().any(|s| s == f) {
                        secrets.push(f.to_string());
                    }
                }
            }
        }
    }
    if !secrets.is_empty() {
        let shown: Vec<&str> = secrets.iter().take(8).map(String::as_str).collect();
        return text(
            false,
            &format!(
                "NOT PUBLISHED: secret-looking files are in this repository's history: {}{}.\nA push uploads all of it. Remove them (git rm --cached, add to .gitignore, and rewrite history if they were committed) before publishing.",
                shown.join(", "),
                if secrets.len() > 8 { ", ..." } else { "" }
            ),
        );
    }
    if let Some(cur) = &existing {
        if web_url(cur) != web_url(&url) {
            return text(false, &format!("NOT PUBLISHED: remote '{}' already points to {}. Use --name <other> to publish to {} under another remote name.", name, cur, url));
        }
    }

    if !yes {
        let mut plan = vec![format!("PUBLISH PREVIEW - nothing was changed or uploaded. Target: {}", web_url(&url))];
        plan.push(if !repo {
            "repository: none here; --yes will create one and make an initial commit of everything (secret-looking files and build output left out)".to_string()
        } else if !commits {
            "repository: no commits yet; --yes will make an initial commit of everything (secret-looking files and build output left out)".to_string()
        } else {
            format!("repository: branch {}, {} commit(s) will be pushed", branch, count)
        });
        if uncommitted > 0 && commits {
            plan.push(format!("{} uncommitted change(s) are NOT included: run mysid commit \"message\" first if they should be", uncommitted));
        }
        plan.push(if existing.is_some() { format!("remote '{}': already set to this address", name) } else { format!("remote '{}': will be added", name) });
        plan.push("secret-looking files: none found".to_string());
        plan.push("never forces, never overwrites commits that are already on the remote".to_string());
        plan.push(format!("To upload: mysid publish {} --yes     (this sends the code to GitHub: do it only when the owner has said so)", raw));
        return text(true, &plan.join("\n"));
    }

    // The upload itself.
    let mut notes: Vec<String> = Vec::new();
    match ensure_repo(root) {
        Ok(Some(n)) => notes.push(n),
        Ok(None) => {}
        Err(e) => return text(false, &format!("NOT PUBLISHED: {}", e)),
    }
    if !has_commits(root) {
        let first = commit_all(root, "Initial commit");
        let line = serde_json::from_str::<Value>(&first).ok().and_then(|v| v.get("output").and_then(|o| o.as_str()).map(String::from)).unwrap_or_default();
        if !has_commits(root) {
            return text(false, &format!("NOT PUBLISHED: there is nothing to commit, so there is nothing to upload.\n{}", line));
        }
        notes.push(format!("initial commit: {}", line.lines().next().unwrap_or("")));
    }
    let branch = git(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|o| o.stdout).unwrap_or_else(|_| "main".to_string());
    if existing.is_none() {
        match git(root, &["remote", "add", &name, &url]) {
            Ok(o) if o.code == 0 => notes.push(format!("remote '{}' added: {}", name, url)),
            Ok(o) => return text(false, &format!("NOT PUBLISHED: git remote add failed: {}", o.stderr)),
            Err(e) => return text(false, &format!("ERROR: {}", e)),
        }
    }
    let total: usize = git(root, &["rev-list", "--count", "HEAD"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0);
    match git_push(root, &name, &branch, 180) {
        Ok(o) if o.code == 0 => {
            notes.push(format!("PUBLISHED {} ({} commit(s), branch {})", web_url(&url), total, branch));
            if uncommitted > 0 {
                notes.push(format!("{} uncommitted change(s) were not part of the push", uncommitted));
            }
            text(true, &notes.join("\n"))
        }
        Ok(o) => {
            notes.push(format!("NOT PUBLISHED: {}", push_failure(&o.stderr)));
            notes.push(format!("(remote '{}' is set; run the same command again after fixing it)", name));
            text(false, &notes.join("\n"))
        }
        Err(e) => {
            notes.push(format!("NOT PUBLISHED: {}", e));
            text(false, &notes.join("\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::git_available;
    use super::*;
    use serde_json::json;
    use std::fs;

    #[test]
    fn publish_previews_then_pushes_and_never_uploads_secrets() {
        if !git_available() {
            return;
        }
        let base = std::env::temp_dir().join(format!("mysid_publish_{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (proj, bare) = (base.join("proj"), base.join("remote.git"));
        fs::create_dir_all(&proj).unwrap();
        fs::create_dir_all(&bare).unwrap();
        assert!(Command::new("git").args(["init", "-q", "--bare"]).current_dir(&bare).output().unwrap().status.success());
        fs::write(proj.join("package.json"), "{}\n").unwrap();
        fs::write(proj.join("a.txt"), "hello\n").unwrap();
        let root = proj.to_string_lossy().to_string();
        let url = format!("file:///{}", bare.to_string_lossy().replace('\\', "/").trim_start_matches('/'));
        let run = |extra: Value| {
            let mut p = json!({ "workspace_root": root, "url": url });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            let raw = execute_publish(&p, Some(&root));
            serde_json::from_str::<Value>(&raw).unwrap()["output"].as_str().unwrap().to_string()
        };

        // Preview: says what would happen and changes nothing (no repository is created).
        let preview = run(json!({}));
        assert!(preview.starts_with("PUBLISH PREVIEW") && preview.contains("--yes"), "{}", preview);
        assert!(!proj.join(".git").exists(), "a preview must not create a repository");

        // A local path or nonsense is not a remote.
        let bad = execute_publish(&json!({ "workspace_root": root, "url": "C:\\some\\folder" }), Some(&root));
        assert!(bad.contains("not a repository address"), "{}", bad);

        // --yes: repository, first commit, remote, push.
        let done = run(json!({ "yes": true }));
        assert!(done.contains("PUBLISHED") && done.contains("initial commit") && done.contains("remote 'origin' added"), "{}", done);
        let log = Command::new("git").args(["log", "--all", "--format=%s"]).current_dir(&bare).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&log.stdout).trim(), "Initial commit");

        // A second run to the same remote is fine (nothing new to push), and a different address under the same name is refused.
        assert!(run(json!({ "yes": true })).contains("PUBLISHED"));
        let other = execute_publish(&json!({ "workspace_root": root, "url": "https://github.com/someone/else.git", "yes": true }), Some(&root));
        assert!(other.contains("already points to"), "{}", other);

        // A secret-looking file in the history stops it, preview or not.
        fs::write(proj.join(".env"), "TOKEN=1\n").unwrap();
        assert!(Command::new("git").args(["add", "-f", ".env"]).current_dir(&proj).output().unwrap().status.success());
        assert!(Command::new("git").args(["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "oops"]).current_dir(&proj).output().unwrap().status.success());
        let blocked = run(json!({ "yes": true }));
        assert!(blocked.starts_with("NOT PUBLISHED: secret-looking files") && blocked.contains(".env"), "{}", blocked);
        let _ = fs::remove_dir_all(&base);
    }
}
