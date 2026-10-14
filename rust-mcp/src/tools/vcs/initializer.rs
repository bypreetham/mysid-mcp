//! Initializer: makes a folder a git repository, locally. Nothing is staged, committed or uploaded here.

use serde_json::Value;
use std::path::Path;

use super::{git, has_commits, in_repo, text};
use crate::tools::workspace::resolve_existing_root;

/// A folder where creating a repository would clearly be a mistake (a drive root or the user's home).
fn refuse_init(root: &Path) -> Option<String> {
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok();
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let is_home = home.map_or(false, |h| Path::new(&h).canonicalize().map_or(false, |c| c == canon));
    if canon.parent().is_none() || is_home {
        Some(format!("Refusing to create a git repository in {} (drive root or home folder). Set the right workspace first.", root.display()))
    } else {
        None
    }
}

/// Local-only ignore rules for a repository mysid just created, so the first commit does not sweep up build
/// output or secrets. Written to `.git/info/exclude` (never a tracked project file).
fn write_exclude(root: &Path) {
    let mut lines = vec!["# added by mysid when it created this repository (local only, not tracked)", ".mysid/", "*.log"];
    if root.join("Cargo.toml").exists() {
        lines.push("/target/");
    }
    if root.join("package.json").exists() {
        lines.push("node_modules/");
    }
    if ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "gradlew"].iter().any(|f| root.join(f).exists()) {
        lines.extend(["build/", ".gradle/", ".idea/", "local.properties", ".cxx/"]);
    }
    lines.extend(["*.jks", "*.keystore", "*.p12", "*.pem", ".env", "keystore.properties"]);
    let path = root.join(".git").join("info").join("exclude");
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::write(&path, format!("{}{}\n{}\n", existing, if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" }, lines.join("\n")));
}

/// Creates a repository in `root` when there is none (never in a drive root or home folder).
/// Ok(Some(note)) when it did.
pub(crate) fn ensure_repo(root: &Path) -> Result<Option<String>, String> {
    if in_repo(root) {
        return Ok(None);
    }
    if let Some(why) = refuse_init(root) {
        return Err(why);
    }
    // `-b main` needs git 2.28; older versions get the default branch name.
    let init = git(root, &["init", "-q", "-b", "main"]).and_then(|o| if o.code == 0 { Ok(o) } else { git(root, &["init", "-q"]) })?;
    if init.code != 0 {
        return Err(format!("git init failed: {}", init.stderr));
    }
    write_exclude(root);
    Ok(Some(format!("no git repository here: created one in {} (local only; build output and secrets excluded)", root.display())))
}

/// A `.gitignore` for the stack found in `root`: build output, dependencies, local config, secrets. mysid proposes it
/// (`mysid bootstrap --gitignore`); the project decides what to keep.
fn gitignore_template(root: &Path) -> String {
    let mut out: Vec<&str> = vec!["# Created by mysid bootstrap: edit freely.", ".mysid/", "*.log", ".env", ".env.*", "!.env.example", "*.jks", "*.keystore", "*.p12", "*.pem", ".DS_Store", "Thumbs.db", ".idea/", ".vscode/"];
    let has = |f: &str| root.join(f).exists();
    if has("package.json") {
        out.extend(["", "# Node", "node_modules/", "dist/", "build/", "coverage/", ".next/", ".vite/", ".netlify/"]);
    }
    if has("Cargo.toml") {
        out.extend(["", "# Rust", "/target/"]);
    }
    if ["build.gradle", "build.gradle.kts", "settings.gradle", "settings.gradle.kts", "gradlew"].iter().any(|f| has(f)) {
        out.extend(["", "# Gradle / Android", "build/", ".gradle/", "local.properties", "keystore.properties", ".cxx/", "*.apk", "*.aab", "captures/"]);
    }
    if has("pom.xml") {
        out.extend(["", "# Maven", "target/"]);
    }
    if has("pyproject.toml") || has("requirements.txt") {
        out.extend(["", "# Python", "__pycache__/", "*.pyc", ".venv/", "venv/", ".pytest_cache/"]);
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// `mysid bootstrap [--branch main] [--gitignore]`: local git initialization and nothing else. Nothing is staged, committed
/// or uploaded. When a repository is already here it says so and changes nothing.
pub fn execute_bootstrap(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    if let Err(e) = git(root, &["--version"]) {
        return text(false, &format!("ERROR: {}", e));
    }
    let want_gitignore = match params.get("gitignore") {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no"),
        _ => false,
    };
    let branch = params.get("branch").and_then(|v| v.as_str()).map(str::trim).filter(|b| !b.is_empty()).unwrap_or("main").to_string();

    if in_repo(root) {
        let top = git(root, &["rev-parse", "--show-toplevel"]).map(|o| o.stdout).unwrap_or_default();
        let same = Path::new(&top).canonicalize().ok() == root.canonicalize().ok();
        let current = git(root, &["symbolic-ref", "--short", "-q", "HEAD"]).map(|o| o.stdout).unwrap_or_default();
        let count: usize = if has_commits(root) { git(root, &["rev-list", "--count", "HEAD"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0) } else { 0 };
        let remote = git(root, &["remote", "get-url", "origin"]).ok().filter(|o| o.code == 0).map(|o| o.stdout);
        let mut lines = vec![if same {
            format!("git repository already exists here: nothing was changed ({}, {}, {}).", if current.is_empty() { "detached HEAD".to_string() } else { format!("branch {}", current) }, if count == 0 { "no commits yet".to_string() } else { format!("{} commit(s)", count) }, remote.map_or("no remote".to_string(), |r| format!("origin {}", r)))
        } else {
            format!("this folder is inside the git repository at {}: nothing was created. mysid status / commit work on that repository (run git init here only if this folder should be its own).", top)
        }];
        if same && want_gitignore {
            lines.push(write_gitignore(root));
        }
        return text(true, &lines.join("\n"));
    }

    if let Some(why) = refuse_init(root) {
        return text(false, &format!("NOT INITIALIZED: {}", why));
    }
    let init = git(root, &["init", "-q", "-b", &branch]).and_then(|o| if o.code == 0 { Ok(o) } else { git(root, &["init", "-q"]) });
    match init {
        Ok(o) if o.code == 0 => {}
        Ok(o) => return text(false, &format!("NOT INITIALIZED: git init failed: {}", o.stderr)),
        Err(e) => return text(false, &format!("ERROR: {}", e)),
    }
    write_exclude(root);
    let mut lines = vec![format!("git repository created in {} (branch {}). Nothing was staged or committed.", root.display(), branch)];
    lines.push("local excludes: build output and secret-looking files are hidden through .git/info/exclude (not a tracked file)".to_string());
    if want_gitignore {
        lines.push(write_gitignore(root));
    } else if root.join(".gitignore").exists() {
        lines.push(".gitignore: found (it controls untracked files; it does not untrack a file that is already committed)".to_string());
    } else {
        lines.push(".gitignore: none yet. Decide what must not be tracked before the first commit (mysid bootstrap --gitignore writes one for this stack)".to_string());
    }
    text(true, &lines.join("\n"))
}

/// Writes `.gitignore` for the stack unless one exists; never overwrites.
fn write_gitignore(root: &Path) -> String {
    let path = root.join(".gitignore");
    if path.exists() {
        return ".gitignore: already exists, left as it is".to_string();
    }
    match std::fs::write(&path, gitignore_template(root)) {
        Ok(_) => ".gitignore: written for this stack (review it before the first commit)".to_string(),
        Err(e) => format!(".gitignore: could not be written: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::super::git_available;
    use super::*;
    use serde_json::json;
    use std::fs;

    fn out(raw: String) -> String {
        serde_json::from_str::<Value>(&raw).unwrap()["output"].as_str().unwrap().to_string()
    }

    #[test]
    fn bootstrap_initializes_only_and_is_idempotent() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_boot_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("package.json"), "{}\n").unwrap();
        fs::write(dir.join("a.txt"), "hi\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let run = |extra: Value| {
            let mut p = json!({ "workspace_root": root });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            out(execute_bootstrap(&p, Some(&root)))
        };
        let first = run(json!({}));
        assert!(first.starts_with("git repository created") && first.contains("Nothing was staged or committed") && first.contains(".gitignore: none yet"), "{}", first);
        assert!(!has_commits(&dir), "bootstrap must not commit");
        let staged = git(&dir, &["diff", "--cached", "--name-only"]).unwrap().stdout;
        assert!(staged.is_empty(), "bootstrap must not stage: {}", staged);
        // Again: says it exists and changes nothing.
        let second = run(json!({}));
        assert!(second.starts_with("git repository already exists here: nothing was changed"), "{}", second);
        // --gitignore writes one for the stack, once, and never overwrites.
        let third = run(json!({ "gitignore": true }));
        assert!(third.contains(".gitignore: written for this stack"), "{}", third);
        let body = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(body.contains("node_modules/") && body.contains(".env"), "{}", body);
        fs::write(dir.join(".gitignore"), "custom\n").unwrap();
        assert!(run(json!({ "gitignore": true })).contains("already exists, left as it is"));
        assert_eq!(fs::read_to_string(dir.join(".gitignore")).unwrap(), "custom\n");
        // A subfolder of a repository is reported, not turned into a nested repository.
        fs::create_dir_all(dir.join("sub")).unwrap();
        let sub = dir.join("sub").to_string_lossy().to_string();
        let nested = out(execute_bootstrap(&json!({ "workspace_root": sub }), Some(&sub)));
        assert!(nested.contains("inside the git repository at"), "{}", nested);
        assert!(!dir.join("sub").join(".git").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
