//! `mysid skill`: workflow guides for LLMs, shipped inside the binary.
//!
//! `mysid skill`            lists them
//! `mysid skill android`    prints one (any LLM client can read it)
//! `mysid skill install`    writes them as Claude Code skills (`.claude/skills/mysid-<name>/SKILL.md`)

use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;

use super::workspace::resolve_existing_root;

/// (name, file). `base` comes first: every other skill builds on it.
const SKILLS: &[(&str, &str)] = &[
    ("base", include_str!("../../skills/base.md")),
    ("android", include_str!("../../skills/android.md")),
    ("react", include_str!("../../skills/react.md")),
];

/// The text after the `---` front matter.
fn body(file: &str) -> &str {
    let rest = file.strip_prefix("---").unwrap_or(file);
    match rest.find("\n---") {
        Some(i) if file.starts_with("---") => rest[i + 4..].trim_start_matches(['\r', '\n']),
        _ => file,
    }
}

/// `description:` from the front matter.
fn description(file: &str) -> String {
    file.lines()
        .find_map(|l| l.strip_prefix("description:"))
        .map(|d| d.trim().to_string())
        .unwrap_or_default()
}

fn find(name: &str) -> Option<&'static (&'static str, &'static str)> {
    let n = name.trim().to_ascii_lowercase();
    let n = n.strip_prefix("mysid-").unwrap_or(&n);
    SKILLS.iter().find(|(s, _)| *s == n)
}

pub fn execute_skill(params: &Value, default_root: Option<&str>) -> String {
    let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let mut words = query.split_whitespace();
    match words.next() {
        None | Some("list") => out(true, &list()),
        Some("install") => install(params, default_root, words.next()),
        Some(name) => match find(name) {
            Some((_, file)) => out(true, body(file)),
            None => out(false, &format!("ERROR: no skill named '{}'. Available: {}", name, SKILLS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))),
        },
    }
}

fn list() -> String {
    let mut s = String::from("mysid skills (workflow guides for LLMs):\n");
    for (name, file) in SKILLS {
        s.push_str(&format!("  {:<9} {}\n", name, description(file)));
    }
    s.push_str("\nmysid skill <name>            print one (stack skills assume `base`)\n");
    s.push_str("mysid skill install [name]    write them as Claude Code skills in .claude/skills/ (all by default; --global for ~/.claude)");
    s
}

fn install(params: &Value, default_root: Option<&str>, which: Option<&str>) -> String {
    let global = params.get("global").map_or(false, |v| v.as_bool().unwrap_or(true));
    let base_dir: PathBuf = if global {
        match std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")) {
            Ok(h) => PathBuf::from(h).join(".claude").join("skills"),
            Err(_) => return out(false, "ERROR: cannot find your home folder for --global"),
        }
    } else {
        match resolve_existing_root(params, "workspace_root", default_root) {
            Ok(ws) => PathBuf::from(ws).join(".claude").join("skills"),
            Err(e) => return e,
        }
    };

    // A stack skill is useless without `base`, so it travels along.
    let chosen: Vec<&(&str, &str)> = match which {
        None | Some("all") => SKILLS.iter().collect(),
        Some(n) => match find(n) {
            Some(s) if s.0 == "base" => vec![s],
            Some(s) => vec![find("base").unwrap(), s],
            None => return out(false, &format!("ERROR: no skill named '{}'. Available: {}", n, SKILLS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "))),
        },
    };

    let mut lines = Vec::new();
    for (name, file) in chosen {
        let dir = base_dir.join(format!("mysid-{}", name));
        let path = dir.join("SKILL.md");
        let existed = path.exists();
        if let Err(e) = fs::create_dir_all(&dir).and_then(|_| fs::write(&path, file)) {
            return out(false, &format!("ERROR: cannot write {}: {}", path.display(), e));
        }
        lines.push(format!("{} mysid-{}  {}", if existed { "updated  " } else { "installed" }, name, path.display()));
    }
    lines.push("Claude Code loads skills at session start: restart it (or open a new session) to pick them up.".to_string());
    out(true, &lines.join("\n"))
}

fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_skill_has_front_matter_and_a_matching_name() {
        for (name, file) in SKILLS {
            assert!(file.starts_with("---\n"), "{} has no front matter", name);
            assert!(file.contains(&format!("\nname: mysid-{}\n", name)), "{}: name must be mysid-{}", name, name);
            assert!(!description(file).is_empty(), "{} has no description", name);
            assert!(body(file).starts_with("# mysid"), "{}: body should start at the heading, got {:?}", name, &body(file)[..20.min(body(file).len())]);
        }
    }

    #[test]
    fn skills_print_list_and_install_into_a_project() {
        let all: Value = serde_json::from_str(&execute_skill(&json!({ "query": "" }), None)).unwrap();
        assert!(all["output"].as_str().unwrap().contains("android"));
        let one: Value = serde_json::from_str(&execute_skill(&json!({ "query": "mysid-android" }), None)).unwrap();
        assert!(one["output"].as_str().unwrap().starts_with("# mysid: Android"));
        let react: Value = serde_json::from_str(&execute_skill(&json!({ "query": "react" }), None)).unwrap();
        assert!(react["output"].as_str().unwrap().starts_with("# mysid: React"));
        assert!(execute_skill(&json!({ "query": "nope" }), None).contains("no skill named"));

        let dir = std::env::temp_dir().join(format!("mysid_skill_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let p = json!({ "query": "install android", "workspace_root": dir.to_string_lossy() });
        let res = execute_skill(&p, None);
        assert!(res.contains("installed mysid-base") && res.contains("installed mysid-android"), "{}", res);
        assert!(dir.join(".claude/skills/mysid-android/SKILL.md").exists() && dir.join(".claude/skills/mysid-base/SKILL.md").exists());
        assert!(execute_skill(&p, None).contains("updated"));
        let _ = fs::remove_dir_all(&dir);
    }
}
