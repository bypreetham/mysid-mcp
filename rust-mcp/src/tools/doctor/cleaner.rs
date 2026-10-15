//! `mysid clean <folder>`: delete the `node_modules` folders of a Node / React project so `npm install` can rebuild them.
//!
//! Safe by construction: it only ever removes directories named exactly `node_modules`, only inside the given folder, only
//! when a `package.json` sits next to them, never follows symbolic links, and without `--yes` it only shows what it would do.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use super::compare::{human, thousands};
use super::health::measure;

/// How many folder levels below the given folder are searched (a folder of many projects, like a learning directory,
/// keeps them several levels down).
const MAX_DEPTH: usize = 12;
/// Folders whose node_modules `npm install` does not rebuild: another tool recreates them.
const TOOL_DIRS: &[(&str, &str)] = &[(".netlify", "the Netlify CLI"), (".aws-sam", "sam build")];

#[derive(Clone)]
struct Found {
    /// Path of the node_modules folder relative to the given folder, `/` separated.
    rel: String,
    path: PathBuf,
    /// The project that owns it (the folder with the package.json).
    project: PathBuf,
}

fn rel_of(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// Every `node_modules` under `base` (not inside another one), split into those with a package.json beside them and those without.
fn find(base: &Path) -> (Vec<Found>, Vec<String>, Vec<Found>) {
    let (mut found, mut skipped, mut tools) = (Vec::new(), Vec::new(), Vec::new());
    let mut stack: Vec<(PathBuf, usize)> = vec![(base.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() || !ft.is_dir() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            let path = e.path();
            if name == "node_modules" {
                if dir.join("package.json").is_file() {
                    let f = Found { rel: rel_of(base, &path), path, project: dir.clone() };
                    if tool_owner(&f.rel).is_some() {
                        tools.push(f);
                    } else {
                        found.push(f);
                    }
                } else {
                    skipped.push(rel_of(base, &path));
                }
            } else if name != ".git" && depth < MAX_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    tools.sort_by(|a, b| a.rel.cmp(&b.rel));
    skipped.sort();
    (found, skipped, tools)
}

/// The tool that recreates this node_modules, when it sits inside one of its folders (`.netlify/plugins/node_modules`).
fn tool_owner(rel: &str) -> Option<&'static str> {
    rel.split('/').find_map(|part| TOOL_DIRS.iter().find(|(dir, _)| *dir == part).map(|(_, tool)| *tool))
}

/// `(name, is_react)` from the project's package.json.
fn describe(project: &Path) -> (String, bool) {
    let Ok(text) = fs::read_to_string(project.join("package.json")) else { return (String::new(), false) };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let has = |k: &str| v.get(k).and_then(|d| d.get("react")).is_some();
    (name, has("dependencies") || has("devDependencies") || has("peerDependencies"))
}

/// The install command that matches the lock file (looked for in the project, then in the given folder).
fn install_command(project: &Path, base: &Path) -> &'static str {
    for dir in [project, base] {
        if dir.join("pnpm-lock.yaml").is_file() {
            return "pnpm install";
        }
        if dir.join("yarn.lock").is_file() {
            return "yarn install";
        }
        if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
            return "bun install";
        }
        if dir.join("package-lock.json").is_file() {
            return "npm install";
        }
    }
    "npm install"
}

fn flag(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| match params.get(*k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"),
        _ => false,
    })
}

pub fn run(params: &Value, default_root: Option<&str>) -> String {
    let fail = |m: String| json!({ "success": false, "error": m }).to_string();
    let base_dir = params.get("workspace_root").and_then(|v| v.as_str()).or(default_root).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let target = match params.get("path").or_else(|| params.get("a")).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        Some(p) => {
            let pb = PathBuf::from(p.trim());
            if pb.is_absolute() { pb } else { base_dir.join(pb) }
        }
        None => base_dir,
    };
    if !target.is_dir() {
        return fail(format!("not a folder: {}", target.display()));
    }
    let target = target.canonicalize().map(|p| PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))).unwrap_or(target);
    let yes = flag(params, &["yes", "y", "confirm"]);
    let shown = target.display().to_string().replace('\\', "/");
    let (mut found, skipped, tools) = find(&target);
    let include_tools = flag(params, &["include_tools", "include-tools"]);
    let max_listed = params.get("max").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map_or(40, |n| (n as usize).clamp(1, 500));
    // node_modules that another tool rebuilds: left alone unless asked for.
    let tools_left: Vec<Found> = if include_tools {
        found.extend(tools.iter().cloned());
        Vec::new()
    } else {
        tools
    };
    let tools_note = |left: &[Found]| -> Option<String> {
        if left.is_empty() {
            return None;
        }
        let mut owners: Vec<&str> = left.iter().filter_map(|f| tool_owner(&f.rel)).collect();
        owners.sort();
        owners.dedup();
        let bytes: u64 = left.iter().map(|f| measure(&f.path, 0, 1).total_bytes).sum();
        Some(format!(
            "left alone: {} node_modules folder(s), {}, that {} recreate, not npm install (add --include-tools to delete them too)",
            left.len(),
            human(bytes),
            owners.join(" and ")
        ))
    };

    if found.is_empty() {
        let mut msg = format!("clean: {}\nnothing to clean: no node_modules folder next to a package.json", shown);
        if !skipped.is_empty() {
            msg.push_str(&format!("\n(left alone, no package.json beside them: {})", skipped.join(", ")));
        }
        if let Some(n) = tools_note(&tools_left) {
            msg.push_str(&format!("\n{}", n));
        }
        return json!({ "success": true, "output": msg }).to_string();
    }

    // Sizes first: the preview and the report both need them.
    let mut sized: Vec<(&Found, u64, usize)> = found
        .iter()
        .map(|f| {
            let h = measure(&f.path, 0, 1);
            (f, h.total_bytes, h.files)
        })
        .collect();
    // Largest first: the few that matter are on top when there are dozens.
    sized.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.rel.cmp(&b.0.rel)));
    let total: u64 = sized.iter().map(|s| s.1).sum();

    let mut out: Vec<String> = vec![format!("clean: {}", shown)];
    out.push(format!(
        "{} node_modules folder(s), {} in all{}:",
        found.len(),
        human(total),
        if yes { "" } else { " (preview: nothing is deleted yet)" }
    ));
    for (i, (f, bytes, files)) in sized.iter().enumerate().take(if yes { 0 } else { max_listed }) {
        let (name, react) = describe(&f.project);
        let what = match (name.is_empty(), react) {
            (false, true) => format!("React project {}", name),
            (true, true) => "React project".to_string(),
            (false, false) => format!("project {}", name),
            (true, false) => "Node project".to_string(),
        };
        out.push(format!("  {}. {:>10}  {}/  ({} files, {})", i + 1, human(*bytes), f.rel, thousands(*files), what));
    }
    if !yes && sized.len() > max_listed {
        let rest: u64 = sized.iter().skip(max_listed).map(|s| s.1).sum();
        out.push(format!("  ... +{} more folder(s), {} (the smaller ones; --max {} lists more)", sized.len() - max_listed, human(rest), sized.len().min(500)));
    }
    if !skipped.is_empty() {
        out.push(format!("left alone (no package.json beside them): {}", skipped.join(", ")));
    }
    if let Some(n) = tools_note(&tools_left) {
        out.push(n);
    }

    if !yes {
        out.push(String::new());
        out.push("Nothing was deleted. To delete these node_modules folders:  mysid clean <folder> --yes".to_string());
        out.push("package.json and the lock file stay, so the install brings back exactly the same versions.".to_string());
        let cmd = install_command(&found[0].project, &target);
        out.push(format!("Then reinstall with:  {}   (in {})", cmd, if found[0].rel == "node_modules" { shown.clone() } else { format!("{}/{}", shown, rel_of(&target, &found[0].project)) }));
        return json!({ "success": true, "dry_run": true, "output": out.join("\n") }).to_string();
    }

    // Delete. Each folder on its own, so one that cannot be removed does not stop the others.
    out.push(String::new());
    let (mut freed, mut failed, mut deleted_n) = (0u64, 0usize, 0usize);
    let mut installs: Vec<String> = Vec::new();
    for (f, bytes, _) in &sized {
        match fs::remove_dir_all(&f.path) {
            Ok(()) => {
                freed += bytes;
                deleted_n += 1;
                if deleted_n <= max_listed {
                    out.push(format!("deleted {}/  ({})", f.rel, human(*bytes)));
                }
                let cmd = install_command(&f.project, &target);
                let at = if f.project == target { shown.clone() } else { format!("{}/{}", shown, rel_of(&target, &f.project)) };
                let line = format!("{}   (in {})", cmd, at);
                if !installs.contains(&line) {
                    installs.push(line);
                }
            }
            Err(e) => {
                failed += 1;
                out.push(format!("NOT deleted {}/: {}. Stop the dev server (mysid dev stop) and close editors or terminals using it, then run again.", f.rel, e));
            }
        }
    }
    if deleted_n > max_listed {
        out.push(format!("... and {} more deleted", deleted_n - max_listed));
    }
    out.push(format!("freed {} from {} folder(s){}", human(freed), deleted_n, if failed > 0 { format!("; {} folder(s) could not be removed", failed) } else { String::new() }));
    if !installs.is_empty() {
        out.push(format!("Reinstall with:  {}", installs.join("\n                 ")));
    }
    json!({ "success": failed == 0, "freed_bytes": freed, "output": out.join("\n") }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn fixture(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mysid_clean_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        put(&d, "package.json", r#"{"name":"web","dependencies":{"react":"18"}}"#);
        put(&d, "package-lock.json", "{}");
        put(&d, "src/App.tsx", "export const App = 1");
        put(&d, "node_modules/react/index.js", "module.exports = 1");
        put(&d, "node_modules/react/node_modules/inner/i.js", "x");
        put(&d, "packages/ui/package.json", r#"{"name":"@x/ui"}"#);
        put(&d, "packages/ui/node_modules/lib/l.js", "y");
        put(&d, "stray/node_modules/z/z.js", "z");
        // Rebuilt by other tools, not by npm install.
        put(&d, ".netlify/plugins/package.json", "{}");
        put(&d, ".netlify/plugins/node_modules/n/n.js", "n");
        put(&d, "api/package.json", "{}");
        put(&d, "api/.aws-sam/build/Fn/package.json", "{}");
        put(&d, "api/.aws-sam/build/Fn/node_modules/s/s.js", "s");
        d
    }

    fn call(root: &Path, extra: Value) -> Value {
        let mut p = json!({ "path": root.to_string_lossy() });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        serde_json::from_str(&run(&p, None)).unwrap()
    }

    #[test]
    fn without_yes_nothing_is_deleted_and_the_report_says_what_would_go() {
        let root = fixture("preview");
        let r = call(&root, json!({}));
        let o = r["output"].as_str().unwrap();
        assert!(o.contains("2 node_modules folder(s)") && o.contains("preview: nothing is deleted yet"), "{}", o);
        assert!(o.contains("React project web") && o.contains("project @x/ui"), "{}", o);
        assert!(o.contains("left alone (no package.json beside them): stray/node_modules"), "{}", o);
        assert!(o.contains("left alone: 2 node_modules folder(s)") && o.contains("sam build and the Netlify CLI recreate") || o.contains("the Netlify CLI and sam build recreate"), "{}", o);
        assert!(o.contains("mysid clean <folder> --yes") && o.contains("npm install"), "{}", o);
        assert!(root.join("node_modules/react/index.js").is_file() && root.join("packages/ui/node_modules").is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn yes_deletes_only_project_node_modules_and_keeps_everything_else() {
        let root = fixture("delete");
        put(&root, "yarn.lock", "");
        fs::remove_file(root.join("package-lock.json")).unwrap();
        let r = call(&root, json!({ "yes": true }));
        let o = r["output"].as_str().unwrap();
        assert_eq!(r["success"], true, "{}", o);
        assert!(o.contains("deleted node_modules/") && o.contains("deleted packages/ui/node_modules/") && o.contains("freed "), "{}", o);
        assert!(o.contains("yarn install"), "the lock file picks the command: {}", o);
        assert!(!root.join("node_modules").exists() && !root.join("packages/ui/node_modules").exists());
        // The source, both package.json files, the lock file and the stray folder (no package.json) stay.
        assert!(root.join("src/App.tsx").is_file() && root.join("package.json").is_file() && root.join("packages/ui/package.json").is_file());
        assert!(root.join("yarn.lock").is_file() && root.join("stray/node_modules/z/z.js").is_file());
        assert!(root.join(".netlify/plugins/node_modules/n/n.js").is_file() && root.join("api/.aws-sam/build/Fn/node_modules/s/s.js").is_file());
        // --include-tools takes those too.
        let all = call(&root, json!({ "yes": true, "include_tools": true }));
        assert!(!root.join(".netlify/plugins/node_modules").exists() && !root.join("api/.aws-sam/build/Fn/node_modules").exists(), "{}", all);
        // Running it again finds nothing.
        let again = call(&root, json!({ "yes": true }));
        assert!(again["output"].as_str().unwrap().contains("nothing to clean"), "{}", again);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_folder_that_is_not_a_node_project_is_not_touched() {
        let root = fixture("notnode");
        let other = root.join("stray");
        let r = call(&other, json!({ "yes": true }));
        assert!(r["output"].as_str().unwrap().contains("nothing to clean"), "{}", r);
        assert!(other.join("node_modules/z/z.js").is_file());
        assert!(call(&root.join("missing"), json!({}))["error"].as_str().unwrap().contains("not a folder"));
        let _ = fs::remove_dir_all(&root);
    }
}
