//! Version control: one folder, one registry.
//!
//! - `initializer`: turns a folder into a git repository (`bootstrap`), and the repository setup `commit` relies on.
//! - `committer`: `commit` (branch and message as named arguments, an optional list of paths), and the helpers the
//!   checkpoint and publish flows reuse.
//! - `tracker`: read-only views: `status`, `branch`, `history`.
//! - `publisher`: the first upload to a remote (`publish`), preview unless `--yes`.
//! - `checkpoint`: `checkpoint`, `diff`, `restore` (snapshots that never touch a branch).
//! - `flow`: what to suggest next after a command, from the repository's state.
//!
//! The registry below maps each command name to its handler: `main.rs` asks `vcs::dispatch(name, ...)` and does not
//! know any of these commands individually.

use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};

mod committer;
pub mod checkpoint;
mod flow;
mod initializer;
mod publisher;
mod purge;
mod tracker;

pub(crate) use committer::commit_all;
pub(crate) use initializer::ensure_repo;

/// Files that must never be committed by an agent without a human doing it on purpose.
const SECRET_NAMES: &[&str] = &["keystore.properties", "local.properties", "google-services-private.json", "id_rsa", "id_ed25519", "credentials.json"];
const SECRET_EXTS: &[&str] = &["jks", "keystore", "pem", "p12", "pfx", "key", "ppk"];

pub(crate) fn is_secret(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if SECRET_NAMES.contains(&name.as_str()) || name == ".env" || name.starts_with(".env.") && !name.ends_with(".example") {
        return true;
    }
    name.rsplit_once('.').map_or(false, |(_, ext)| SECRET_EXTS.contains(&ext))
}

pub(crate) struct Out {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

pub(crate) fn git(root: &Path, args: &[&str]) -> Result<Out, String> {
    git_env(root, args, &[])
}

/// `git` with extra environment variables (e.g. a private `GIT_INDEX_FILE`).
pub(crate) fn git_env(root: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<Out, String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(root).stdin(Stdio::null()).env("GIT_TERMINAL_PROMPT", "0");
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let o = cmd.output().map_err(|e| format!("git is not available ({}). Install git and try again.", e))?;
    Ok(Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).trim_end().to_string(),
        stderr: String::from_utf8_lossy(&o.stderr).trim_end().to_string(),
    })
}

pub(crate) fn in_repo(root: &Path) -> bool {
    git(root, &["rev-parse", "--is-inside-work-tree"]).map_or(false, |o| o.code == 0 && o.stdout == "true")
}

pub(crate) fn has_commits(root: &Path) -> bool {
    git(root, &["rev-parse", "--verify", "-q", "HEAD"]).map_or(false, |o| o.code == 0)
}

/// ` 4 files changed, 37 insertions(+), 9 deletions(-)` -> `4 files, +37 -9`.
pub(crate) fn compact_stat(line: &str) -> String {
    let num = |word: &str| -> usize {
        line.split(',')
            .find(|part| part.contains(word))
            .and_then(|part| part.split_whitespace().next())
            .and_then(|n| n.parse().ok())
            .unwrap_or(0)
    };
    let files = num("file");
    format!("{} file{}, +{} -{}", files, if files == 1 { "" } else { "s" }, num("insertion"), num("deletion"))
}

pub(crate) fn text(success: bool, output: &str) -> String {
    json!({ "success": success, "output": output }).to_string()
}

/// One version-control command: its names, a one-line summary and its handler.
pub struct VcsCommand {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub run: fn(&Value, Option<&str>) -> String,
}

/// The registry: every version-control command and the function that runs it. Adding a command is one line here
/// (plus its handler); `main.rs` needs no change to dispatch it.
pub const COMMANDS: &[VcsCommand] = &[
    VcsCommand { name: "bootstrap", aliases: &["init"], summary: "initialize a local git repository (no staging, no commit)", run: initializer::execute_bootstrap },
    VcsCommand { name: "commit", aliases: &[], summary: "commit the changes: --message, optional --branch and --paths", run: committer::execute_commit },
    VcsCommand { name: "status", aliases: &[], summary: "branch, ahead/behind, changed files", run: tracker::execute_status },
    VcsCommand { name: "branch", aliases: &[], summary: "local branches with their last commit", run: tracker::execute_branch },
    VcsCommand { name: "history", aliases: &[], summary: "the last commits, compactly", run: tracker::execute_history },
    VcsCommand { name: "publish", aliases: &[], summary: "first upload to a remote (preview; --yes pushes)", run: publisher::execute_publish },
    VcsCommand { name: "checkpoint", aliases: &[], summary: "snapshot the working tree (never touches a branch)", run: checkpoint::execute_checkpoint },
    VcsCommand { name: "diff", aliases: &[], summary: "what changed since a checkpoint", run: checkpoint::execute_diff },
    VcsCommand { name: "restore", aliases: &[], summary: "put the files back to a checkpoint (preview; --yes applies)", run: checkpoint::execute_restore },
    VcsCommand { name: "purge", aliases: &[], summary: "shorthand for `mysid git purge`: remove git from the project (preview; --yes does it)", run: purge::execute_purge },
    VcsCommand { name: "git", aliases: &[], summary: "git-level operations: `mysid git purge` removes git from the project (preview; --yes does it)", run: git_command },
];

/// `mysid git <action>`: the git-level operations that are not part of the daily loop. Today: `purge`.
fn git_command(params: &Value, default_root: Option<&str>) -> String {
    let action = ["action", "query"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).map(|s| s.trim().to_lowercase()).unwrap_or_default();
    match action.as_str() {
        "purge" => purge::execute_purge(params, default_root),
        "" | "help" | "-h" | "--help" => text(true, PURGE_HELP),
        other => text(false, &format!("unknown git action '{}': use mysid git purge", other)),
    }
}

pub const PURGE_HELP: &str = "mysid git purge: remove git from the project, as if git had never been used here\n\n\
Usage:\n  \
  mysid git purge                      PREVIEW: what would be removed; nothing is changed\n  \
  mysid git purge --yes                remove .git/ (commits, branches, tags, stashes, the index, the repository config,\n                                       remote URLs, mysid checkpoints)\n  \
  mysid git purge --gitignore --yes    also .gitignore (it may hold project rules: only when asked)\n  \
  mysid purge [flags] [--yes]          the same command, shorter\n  \
  flags: --gitignore  --gitattributes  --gitmodules  --extras (.git-blame-ignore-revs, .gitkeep files)\n         \
         --nested (repositories inside subfolders)  --all (every flag above)\n\n\
Source code, documents, build files and every other project file are never touched. It cannot be undone: commits that exist\n\
only in this repository are lost, and the preview says how many. A copy on GitHub is not affected. Refuses in a drive root or\n\
the home folder, and inside a parent repository (run it where the .git is). Afterwards `mysid bootstrap` starts a new one.\n\
An agent must ask the owner before --yes.\n";

/// The command a name or alias refers to.
pub fn find(name: &str) -> Option<&'static VcsCommand> {
    COMMANDS.iter().find(|c| c.name == name || c.aliases.contains(&name))
}

/// Runs a registered command, then adds the flow hint for the state it left (`flow`). None when `name` is not a
/// version-control command.
pub fn dispatch(name: &str, params: &Value, default_root: Option<&str>) -> Option<String> {
    let command = find(name)?;
    let output = (command.run)(params, default_root);
    Some(flow::with_next(command.name, params, default_root, output))
}

/// `mysid vcs`: the workflow and the commands in it, straight from the registry.
pub fn overview() -> String {
    let mut lines = vec![
        "version control in mysid: bootstrap -> .gitignore -> commit -> status -> publish".to_string(),
        "  1. mysid bootstrap                                   git init only; nothing staged or committed".to_string(),
        "  2. edit .gitignore (or: mysid bootstrap --gitignore)  what must not be tracked; it does not untrack committed files".to_string(),
        "  3. mysid commit --branch main --message \"Initial commit\"   then, on any branch: mysid commit --message \"...\"".to_string(),
        "  4. mysid status                                      branch, changed files".to_string(),
        "  5. mysid publish <repo-url>                          preview; --yes uploads (ask the owner)".to_string(),
        String::new(),
        "commands:".to_string(),
    ];
    for c in COMMANDS {
        let aliases = if c.aliases.is_empty() { String::new() } else { format!(" (also: {})", c.aliases.join(", ")) };
        lines.push(format!("  mysid {:<11} {}{}", c.name, c.summary, aliases));
    }
    lines.push("each command's answer ends with a next: line when the repository state suggests one (flow.rs)".to_string());
    lines.join("\n")
}

/// Every name the registry answers to.
pub fn names() -> Vec<&'static str> {
    COMMANDS.iter().flat_map(|c| std::iter::once(c.name).chain(c.aliases.iter().copied())).collect()
}

#[cfg(test)]
pub(crate) fn git_available() -> bool {
    Command::new("git").arg("--version").output().map_or(false, |o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_is_complete_and_unambiguous() {
        let names = names();
        let mut sorted = names.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), names.len(), "a name or alias is registered twice: {:?}", names);
        for n in &names {
            assert!(crate::KNOWN_COMMANDS.contains(n), "{} is registered but missing from KNOWN_COMMANDS", n);
        }
        assert_eq!(find("init").map(|c| c.name), Some("bootstrap"));
        assert!(find("nonsense").is_none());
        assert!(dispatch("nonsense", &json!({}), None).is_none());
        let o = overview();
        assert!(o.contains("bootstrap") && o.contains("mysid publish") && COMMANDS.iter().all(|c| o.contains(c.name)), "{}", o);
    }

    #[test]
    fn secrets_are_recognised() {
        for s in ["my-release-key.jks", "app/keystore.properties", ".env", "cfg/.env.production", "certs/server.pem", "local.properties"] {
            assert!(is_secret(s), "{}", s);
        }
        for s in ["src/Main.kt", ".env.example", "README.md", "keys.kt", "app/build.gradle.kts"] {
            assert!(!is_secret(s), "{}", s);
        }
    }
}
