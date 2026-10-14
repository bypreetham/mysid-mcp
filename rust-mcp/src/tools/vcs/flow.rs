//! Flow: after a version-control command, what is the sensible next step for the repository's current state?
//!
//! The table is the workflow: bootstrap -> choose what to ignore -> commit on a named branch -> publish. It is data
//! (`FLOW`), not scattered `if`s: a step applies to a command and a condition on the repository, and the first step
//! that matches adds one `next:` line to the answer. Commands whose answer needs no suggestion have no step.

use serde_json::Value;
use std::path::Path;

use super::git;
use crate::tools::workspace::resolve_existing_root;

/// What the conditions can ask about the repository.
pub struct State {
    pub repo: bool,
    pub commits: usize,
    pub remote: bool,
    pub gitignore: bool,
}

struct Step {
    /// The command this step follows.
    after: &'static str,
    when: fn(&State) -> bool,
    hint: &'static str,
}

const FLOW: &[Step] = &[
    Step {
        after: "bootstrap",
        when: |s| s.repo && s.commits == 0 && !s.gitignore,
        hint: "next: write a .gitignore for what must not be tracked (mysid bootstrap --gitignore adds one for this stack), then mysid commit --branch main --message \"Initial commit\"",
    },
    Step { after: "bootstrap", when: |s| s.repo && s.commits == 0, hint: "next: mysid commit --branch main --message \"Initial commit\"" },
    Step { after: "bootstrap", when: |s| s.repo && s.commits > 0 && !s.remote, hint: "next: mysid publish <repo-url> to upload (it previews first)" },
    Step { after: "commit", when: |s| s.commits == 1 && !s.remote, hint: "next: mysid status to review, mysid publish <repo-url> to upload (it previews first)" },
    Step { after: "status", when: |s| s.repo && s.commits == 0, hint: "next: mysid commit --branch main --message \"Initial commit\"" },
];

fn state(root: &Path) -> State {
    let repo = git(root, &["rev-parse", "--is-inside-work-tree"]).map_or(false, |o| o.code == 0 && o.stdout == "true");
    let commits = if repo { git(root, &["rev-list", "--count", "HEAD"]).ok().filter(|o| o.code == 0).and_then(|o| o.stdout.trim().parse().ok()).unwrap_or(0) } else { 0 };
    let remote = repo && git(root, &["remote"]).map_or(false, |o| !o.stdout.trim().is_empty());
    State { repo, commits, remote, gitignore: root.join(".gitignore").exists() }
}

/// The `next:` line for `command` in the current state, if any.
pub fn next_hint(command: &str, root: &Path) -> Option<&'static str> {
    if !FLOW.iter().any(|s| s.after == command) {
        return None;
    }
    let st = state(root);
    FLOW.iter().find(|s| s.after == command && (s.when)(&st)).map(|s| s.hint)
}

/// Adds the hint to a successful `{success, output}` answer. Anything else is returned unchanged.
pub fn with_next(command: &str, params: &Value, default_root: Option<&str>, output: String) -> String {
    let Ok(mut v) = serde_json::from_str::<Value>(&output) else { return output };
    if v.get("success").and_then(|s| s.as_bool()) != Some(true) {
        return output;
    }
    let Ok(ws) = resolve_existing_root(params, "workspace_root", default_root) else { return output };
    let Some(hint) = next_hint(command, Path::new(&ws)) else { return output };
    let Some(text) = v.get("output").and_then(|o| o.as_str()).map(String::from) else { return output };
    v["output"] = Value::String(format!("{}\n{}", text.trim_end(), hint));
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flow_follows_the_repository_state() {
        let blank = State { repo: true, commits: 0, remote: false, gitignore: false };
        let find = |cmd: &str, s: &State| FLOW.iter().find(|st| st.after == cmd && (st.when)(s)).map(|st| st.hint);
        assert!(find("bootstrap", &blank).unwrap().contains(".gitignore"));
        let with_ignore = State { gitignore: true, ..blank };
        assert!(find("bootstrap", &with_ignore).unwrap().starts_with("next: mysid commit --branch main"));
        let first = State { repo: true, commits: 1, remote: false, gitignore: true };
        assert!(find("commit", &first).unwrap().contains("publish"));
        // The second commit and later: no suggestion, so ordinary commits stay quiet.
        let later = State { commits: 2, ..first };
        assert!(find("commit", &later).is_none());
        // A repository that already has a remote needs no publish hint.
        let published = State { repo: true, commits: 1, remote: true, gitignore: true };
        assert!(find("commit", &published).is_none());
    }
}
