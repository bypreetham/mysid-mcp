//! Purge: `mysid git purge` removes git from a project so that it is as if git had never been used there.
//!
//! The repository (`.git/`: commits, branches, tags, stashes, the index, the repository config, remote URLs and mysid's
//! checkpoints) is removed. Git's helper files are removed only when asked for, one flag each, because a `.gitignore` or
//! `.gitattributes` may hold project rules worth keeping. Ordinary project files are never touched. Nothing happens
//! without `--yes`; without it the command only shows what would go. It cannot be undone: the commits that exist only here
//! are gone, which the preview says (the copy on GitHub is not affected).

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

use super::{git, in_repo, text};
use crate::tools::workspace::resolve_existing_root;

/// Folders never searched for nested repositories and `.gitkeep` files.
const SKIP_DIRS: &[&str] = &["node_modules", "target", "build", "dist", ".gradle", ".idea", ".venv", "venv", "__pycache__", ".mysid"];

fn flag(params: &Value, key: &str) -> bool {
    match params.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no"),
        Some(Value::Number(n)) => n.as_f64().map_or(false, |n| n != 0.0),
        _ => false,
    }
}

/// `https://user:token@host/x.git` -> `https://***@host/x.git`: a remote URL must not leak a credential into the answer.
pub(crate) fn mask_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let end = rest.find('/').unwrap_or(rest.len());
    match rest[..end].rsplit_once('@') {
        Some((_, host)) => format!("{}://***@{}{}", scheme, host, &rest[end..]),
        None => url.to_string(),
    }
}

fn human(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// Total size of a folder, not following links.
fn dir_size(p: &Path) -> u64 {
    let Ok(md) = fs::symlink_metadata(p) else { return 0 };
    if md.file_type().is_symlink() {
        return 0;
    }
    if md.is_file() {
        return md.len();
    }
    fs::read_dir(p).map(|rd| rd.flatten().map(|e| dir_size(&e.path())).sum()).unwrap_or(0)
}

/// Removes a file or folder tree. Git marks object and pack files read-only, which `remove_dir_all` cannot delete on
/// Windows, so the read-only flag is cleared first. A link is removed as a link: what it points to is never entered.
fn remove_tree(p: &Path) -> std::io::Result<()> {
    let md = fs::symlink_metadata(p)?;
    if md.file_type().is_symlink() {
        return fs::remove_file(p).or_else(|_| fs::remove_dir(p));
    }
    if md.is_dir() {
        for e in fs::read_dir(p)? {
            remove_tree(&e?.path())?;
        }
        return fs::remove_dir(p);
    }
    let mut perm = md.permissions();
    if perm.readonly() {
        perm.set_readonly(false);
        let _ = fs::set_permissions(p, perm);
    }
    fs::remove_file(p)
}

/// Other git artifacts under `root`: nested repositories (folders with their own `.git`) and `.gitkeep` files.
fn scan_artifacts(root: &Path) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut nested, mut keeps) = (Vec::new(), Vec::new());
    let mut visited = 0usize;
    fn walk(dir: &Path, root: &Path, depth: usize, nested: &mut Vec<PathBuf>, keeps: &mut Vec<PathBuf>, visited: &mut usize) {
        if depth > 8 || *visited > 20_000 {
            return;
        }
        *visited += 1;
        let Ok(rd) = fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if name == ".git" {
                    if dir != root {
                        nested.push(dir.to_path_buf());
                    }
                    continue;
                }
                if SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                walk(&p, root, depth + 1, nested, keeps, visited);
            } else if name == ".git" && dir != root {
                nested.push(dir.to_path_buf()); // a `.git` file: a submodule or worktree pointer
            } else if name == ".gitkeep" {
                keeps.push(p);
            }
        }
    }
    walk(root, root, 0, &mut nested, &mut keeps, &mut visited);
    (nested, keeps)
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| p.to_string_lossy().to_string())
}

/// `mysid git purge [--gitignore] [--gitattributes] [--gitmodules] [--extras] [--nested] [--yes]`
pub fn execute_purge(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    let yes = flag(params, "yes");
    let everything = flag(params, "all");
    let (want_ignore, want_attr, want_modules) = (everything || flag(params, "gitignore"), everything || flag(params, "gitattributes"), everything || flag(params, "gitmodules"));
    let (want_extras, want_nested) = (everything || flag(params, "extras"), everything || flag(params, "nested"));

    // Never purge a drive root or the home folder: that is a wrong workspace, not a project.
    let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).ok().and_then(|h| Path::new(&h).canonicalize().ok());
    if canon.parent().is_none() || home.as_deref() == Some(canon.as_path()) {
        return text(false, &format!("NOT PURGED: {} is a drive root or the home folder: this is not a project. Run it inside the project.", root.display()));
    }

    let dot_git = root.join(".git");
    let has_git = fs::symlink_metadata(&dot_git).is_ok();
    // Inside a parent repository: the `.git` there is not this folder's to remove.
    if !has_git && in_repo(root) {
        let top = git(root, &["rev-parse", "--show-toplevel"]).map(|o| o.stdout).unwrap_or_default();
        return text(true, &format!("nothing to purge here: this folder has no .git of its own; it belongs to the repository at {}. Run `mysid git purge` there.", top));
    }

    // What the repository holds, for the warning (read before anything is removed).
    let mut facts: Vec<String> = Vec::new();
    let mut unpushed = 0usize;
    if has_git && dot_git.is_dir() {
        let count = |args: &[&str]| git(root, args).ok().filter(|o| o.code == 0).map(|o| o.stdout.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
        let commits: usize = git(root, &["rev-list", "--count", "--all"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0);
        let branches = count(&["for-each-ref", "refs/heads", "--format=%(refname)"]);
        let tags = count(&["tag", "--list"]);
        let stashes = count(&["stash", "list"]);
        let checkpoints = count(&["for-each-ref", "refs/mysid/checkpoints", "--format=%(refname)"]);
        unpushed = git(root, &["rev-list", "--count", "--branches", "--not", "--remotes"]).ok().and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0);
        facts.push(format!("{} commit(s) on {} branch(es)", commits, branches));
        if tags > 0 {
            facts.push(format!("{} tag(s)", tags));
        }
        if stashes > 0 {
            facts.push(format!("{} stash(es)", stashes));
        }
        if checkpoints > 0 {
            facts.push(format!("{} mysid checkpoint(s)", checkpoints));
        }
        let remotes: Vec<String> = git(root, &["remote", "-v"]).map(|o| o.stdout).unwrap_or_default().lines().filter(|l| l.ends_with("(fetch)")).map(|l| {
            let mut p = l.split_whitespace();
            format!("{} {}", p.next().unwrap_or(""), mask_credentials(p.next().unwrap_or("")))
        }).collect();
        if !remotes.is_empty() {
            facts.push(format!("remote {}", remotes.join(", ")));
        }
        facts.push("the index and the repository config".to_string());
    }

    // Everything that could go: each item says whether it is selected.
    struct Item {
        label: String,
        /// The name used in the result line.
        short: String,
        path: PathBuf,
        selected: bool,
        flag: &'static str,
    }
    let mut items: Vec<Item> = Vec::new();
    if has_git {
        let size = if dot_git.is_dir() { format!(" ({})", human(dir_size(&dot_git))) } else { " (a pointer file to a repository elsewhere)".to_string() };
        items.push(Item { label: format!(".git{}: {}", size, if facts.is_empty() { "the repository link".to_string() } else { facts.join(", ") }), short: ".git".to_string(), path: dot_git.clone(), selected: true, flag: "" });
    }
    for (name, selected, flag_name) in [(".gitignore", want_ignore, "--gitignore"), (".gitattributes", want_attr, "--gitattributes"), (".gitmodules", want_modules, "--gitmodules"), (".git-blame-ignore-revs", want_extras, "--extras")] {
        let p = root.join(name);
        if fs::symlink_metadata(&p).is_ok() {
            items.push(Item { label: name.to_string(), short: name.to_string(), path: p, selected, flag: flag_name });
        }
    }
    let (nested, keeps) = scan_artifacts(root);
    if !keeps.is_empty() {
        items.push(Item { label: format!("{} .gitkeep file(s)", keeps.len()), short: ".gitkeep files".to_string(), path: PathBuf::new(), selected: want_extras, flag: "--extras" });
    }
    for n in &nested {
        let label = format!("nested repository {}/.git", rel(root, n));
        items.push(Item { short: label.clone(), label, path: n.join(".git"), selected: want_nested, flag: "--nested" });
    }

    if items.is_empty() {
        return text(true, "nothing to purge: there is no git repository or git file here");
    }

    let chosen: Vec<&Item> = items.iter().filter(|i| i.selected).collect();
    let kept: Vec<&Item> = items.iter().filter(|i| !i.selected).collect();

    if !yes {
        let mut out = vec![format!("PURGE PREVIEW - nothing was removed. Target: {} (git is removed from the project; its files stay)", root.display())];
        for i in &chosen {
            out.push(format!("  will remove: {}", i.label));
        }
        if unpushed > 0 {
            out.push(format!("  WARNING: {} commit(s) exist only in this repository (on no remote): they cannot be recovered afterwards", unpushed));
        } else if has_git && dot_git.is_dir() && facts.iter().any(|f| f.starts_with("remote")) {
            out.push("  every commit is also on a remote: the history survives there".to_string());
        } else if has_git && dot_git.is_dir() {
            out.push("  WARNING: this repository has no remote: its history cannot be recovered afterwards".to_string());
        }
        if !kept.is_empty() {
            let list: Vec<String> = kept.iter().map(|i| format!("{} ({})", i.short, i.flag)).collect();
            out.push(format!("  found and kept (add the flag to remove): {}", list.join("; ")));
        }
        out.push("  never touched: source code, documents, build files and every other project file; a copy on GitHub".to_string());
        out.push("To do it: mysid git purge <same flags> --yes     (irreversible: do it only when the owner has said so)".to_string());
        return text(true, &out.join("\n"));
    }

    // The removal.
    let mut done: Vec<String> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    for i in &chosen {
        if i.label.contains(".gitkeep file(s)") {
            let mut ok = 0usize;
            for k in &keeps {
                if remove_tree(k).is_ok() {
                    ok += 1;
                }
            }
            done.push(format!("{} of {} .gitkeep file(s)", ok, keeps.len()));
            continue;
        }
        // A path that is not exactly under the workspace is never removed.
        if !i.path.starts_with(root) {
            failed.push(format!("{}: outside the project, skipped", i.label));
            continue;
        }
        match remove_tree(&i.path) {
            Ok(()) => done.push(i.short.clone()),
            Err(e) => failed.push(format!("{}: {}", rel(root, &i.path), e)),
        }
    }
    let mut out = Vec::new();
    if failed.is_empty() {
        out.push(format!("PURGED: {}", done.join(", ")));
    } else {
        out.push(format!("PURGE INCOMPLETE: removed {}; could not remove:", if done.is_empty() { "nothing".to_string() } else { done.join(", ") }));
        for f in &failed {
            out.push(format!("  {}", f));
        }
        out.push("  (a file may be open in another program: close it and run the same command again)".to_string());
    }
    if has_git && !dot_git.exists() {
        out.push("git no longer knows this folder: `git status` fails here, and history, branches, remotes and checkpoints are gone. Project files are untouched.".to_string());
        out.push("a new repository: mysid bootstrap".to_string());
    }
    if !kept.is_empty() {
        out.push(format!("kept: {}", kept.iter().map(|i| i.short.clone()).collect::<Vec<_>>().join(", ")));
    }
    text(failed.is_empty(), &out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::super::git_available;
    use super::*;
    use serde_json::json;

    fn out(raw: String) -> String {
        serde_json::from_str::<Value>(&raw).unwrap()["output"].as_str().unwrap().to_string()
    }

    #[test]
    fn credentials_never_reach_the_answer() {
        assert_eq!(mask_credentials("https://user:ghp_secret@github.com/o/r.git"), "https://***@github.com/o/r.git");
        assert_eq!(mask_credentials("https://github.com/o/r.git"), "https://github.com/o/r.git");
        assert_eq!(mask_credentials("git@github.com:o/r.git"), "git@github.com:o/r.git");
    }

    #[test]
    fn purge_previews_then_removes_git_and_only_what_was_asked() {
        if !git_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("mysid_purge_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src").join("empty")).unwrap();
        fs::create_dir_all(dir.join("vendor").join("lib")).unwrap();
        for (f, c) in [("src/Main.kt", "class Main\n"), ("README.md", "doc\n"), ("notes.txt", "n\n"), (".gitattributes", "* text=auto\n"), ("src/empty/.gitkeep", "")] {
            fs::write(dir.join(f), c).unwrap();
        }
        let root = dir.to_string_lossy().to_string();
        // A repository with a commit that exists on no remote, a credentialed remote, and a nested repository.
        out(super::super::initializer::execute_bootstrap(&json!({ "workspace_root": root, "gitignore": true }), Some(&root)));
        super::super::committer::commit_all(&dir, "First");
        git(&dir, &["remote", "add", "origin", "https://me:ghp_TOKEN123@github.com/me/proj.git"]).unwrap();
        let nested = dir.join("vendor").join("lib");
        git(&nested, &["init", "-q"]).unwrap();
        let run = |extra: Value| {
            let mut p = json!({ "workspace_root": root });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            out(execute_purge(&p, Some(&root)))
        };

        // Preview: lists what would go, warns about the unpushed commit, masks the token, removes nothing.
        let preview = run(json!({}));
        assert!(preview.starts_with("PURGE PREVIEW - nothing was removed"), "{}", preview);
        assert!(preview.contains("will remove: .git") && preview.contains("1 commit(s) on 1 branch(es)"), "{}", preview);
        assert!(preview.contains("WARNING: 1 commit(s) exist only in this repository"), "{}", preview);
        assert!(preview.contains("https://***@github.com/me/proj.git") && !preview.contains("ghp_TOKEN123"), "{}", preview);
        assert!(preview.contains(".gitignore") && preview.contains("--gitignore") && preview.contains(".gitattributes") && preview.contains("nested repository vendor/lib/.git"), "{}", preview);
        assert!(dir.join(".git").is_dir() && dir.join(".gitignore").exists(), "a preview removes nothing");

        // --yes with only .gitignore added: .git and .gitignore go; the other git files, the nested repository and every project file stay.
        let done = run(json!({ "yes": true, "gitignore": true }));
        assert!(done.starts_with("PURGED: .git, .gitignore"), "{}", done);
        assert!(!dir.join(".git").exists() && !dir.join(".gitignore").exists());
        assert!(dir.join(".gitattributes").exists() && nested.join(".git").exists() && dir.join("src/empty/.gitkeep").exists());
        for f in ["src/Main.kt", "README.md", "notes.txt"] {
            assert!(dir.join(f).exists(), "{} must stay", f);
        }
        assert!(!in_repo_outside_parent(&dir), "git must no longer know the folder");

        // Nothing left of the main repository: the rest only with their flags.
        let again = run(json!({ "yes": true, "gitattributes": true, "extras": true, "nested": true }));
        assert!(again.starts_with("PURGED: .gitattributes") && again.contains("nested repository vendor/lib/.git"), "{}", again);
        assert!(!dir.join(".gitattributes").exists() && !nested.join(".git").exists() && !dir.join("src/empty/.gitkeep").exists());
        assert!(dir.join("src/Main.kt").exists());
        assert!(run(json!({})).starts_with("nothing to purge"));
        let _ = fs::remove_dir_all(&dir);
    }

    /// True when git still sees `dir` as inside a repository of its own (`dir/.git`).
    fn in_repo_outside_parent(dir: &Path) -> bool {
        dir.join(".git").exists()
    }
}
