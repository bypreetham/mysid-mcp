//! Orientation for an agent that has just entered a project: where the code is, in what language, where it starts, and the
//! folders worth looking at next. Used by `mysid set`, `mysid tree` and `mysid overview <folder> --depth 1`.

use ignore::WalkBuilder;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const MAX_ENTRIES: usize = 200_000;

/// Folders that never hold the project's own code, whatever the stack.
const NOT_CODE_DIRS: &[&str] = &[
    "node_modules", "target", "build", "dist", "out", "obj", "bin", "vendor", "external", "externals", "third_party", "third-party", "thirdparty", "3rdparty", "pods",
    "intermediates", "outputs", "venv", "env", "__pycache__", "coverage",
];

const CODE_EXTS: &[&str] = &[
    "ts", "tsx", "js", "jsx", "mjs", "cjs", "vue", "svelte", "py", "rs", "go", "java", "kt", "kts", "swift", "cs", "c", "cc", "cpp", "cxx", "h", "hpp", "dart", "rb", "php",
    "sh", "sql", "html", "css", "scss", "m", "mm", "lua", "scala", "ex", "exs",
];

fn is_code(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).map_or(false, |e| CODE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

fn language_of(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "ts" | "tsx" => "TypeScript",
        "js" | "jsx" | "mjs" | "cjs" => "JavaScript",
        "vue" => "Vue",
        "svelte" => "Svelte",
        "py" => "Python",
        "rs" => "Rust",
        "go" => "Go",
        "java" => "Java",
        "kt" | "kts" => "Kotlin",
        "swift" => "Swift",
        "cs" => "C#",
        "c" | "cc" | "cpp" | "cxx" | "h" | "hpp" => "C/C++",
        "dart" => "Dart",
        "rb" => "Ruby",
        "php" => "PHP",
        _ => return None,
    })
}

#[derive(Default, Clone)]
pub struct Counts {
    pub code: usize,
    pub files: usize,
    /// Code files per extension.
    pub exts: BTreeMap<String, usize>,
}

impl Counts {
    fn add(&mut self, path: &Path) {
        self.files += 1;
        if is_code(path) {
            self.code += 1;
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            *self.exts.entry(ext).or_insert(0) += 1;
        }
    }
}

/// Everything below `start`, as (path relative to `start`, is_dir) pairs. .gitignore'd files, dotfolders (unless `hidden`) and
/// generated/copied-library folders are left out.
fn entries_below(start: &Path, hidden: bool) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    let walker = WalkBuilder::new(start)
        .hidden(!hidden)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(|e| {
            if e.depth() == 0 {
                return true;
            }
            let is_dir = e.file_type().map_or(false, |t| t.is_dir());
            !(is_dir && NOT_CODE_DIRS.contains(&e.file_name().to_string_lossy().to_lowercase().as_str()))
        })
        .build();
    for e in walker.flatten() {
        if e.depth() == 0 {
            continue;
        }
        if out.len() >= MAX_ENTRIES {
            break;
        }
        let is_dir = e.file_type().map_or(false, |t| t.is_dir());
        if let Ok(rel) = e.path().strip_prefix(start) {
            out.push((rel.to_path_buf(), is_dir));
        }
    }
    out
}

fn first_component(p: &Path) -> Option<String> {
    p.components().next().map(|c| c.as_os_str().to_string_lossy().to_string())
}

/// Folders directly under `dir`, with code counts (code files anywhere below), most code first.
fn child_dirs(dir: &Path, hidden: bool) -> (Vec<(String, Counts)>, Counts) {
    let mut by_dir: BTreeMap<String, Counts> = BTreeMap::new();
    let mut loose = Counts::default();
    for (rel, is_dir) in entries_below(dir, hidden) {
        if is_dir {
            if rel.components().count() == 1 {
                by_dir.entry(first_component(&rel).unwrap_or_default()).or_default();
            }
            continue;
        }
        match (rel.components().count(), first_component(&rel)) {
            (1, _) => loose.add(&rel),
            (_, Some(top)) => by_dir.entry(top).or_default().add(&rel),
            _ => {}
        }
    }
    let mut v: Vec<(String, Counts)> = by_dir.into_iter().collect();
    v.sort_by(|a, b| b.1.code.cmp(&a.1.code).then(b.1.files.cmp(&a.1.files)).then(a.0.cmp(&b.0)));
    (v, loose)
}

fn package_json_words(root: &Path) -> String {
    std::fs::read_to_string(root.join("package.json")).unwrap_or_default().to_lowercase()
}

/// `TypeScript/React`, `Kotlin (Android)`, `Rust`, ...
fn stack_label(root: &Path, exts: &BTreeMap<String, usize>) -> String {
    let mut by_lang: BTreeMap<&str, usize> = BTreeMap::new();
    for (e, n) in exts {
        if let Some(l) = language_of(e) {
            *by_lang.entry(l).or_insert(0) += n;
        }
    }
    let Some((lang, _)) = by_lang.iter().max_by_key(|(_, n)| **n) else {
        return "no recognised code".to_string();
    };
    let pkg = package_json_words(root);
    let has = |e: &str| exts.contains_key(e);
    let mut label = lang.to_string();
    if matches!(*lang, "TypeScript" | "JavaScript") {
        let fw = [("next", "Next.js"), ("react", "React"), ("vue", "Vue"), ("svelte", "Svelte"), ("angular", "Angular"), ("express", "Express")]
            .iter()
            .find(|(k, _)| pkg.contains(&format!("\"{}", k)) || pkg.contains(&format!("@{}", k)))
            .map(|(_, v)| *v);
        label = match fw {
            Some(f) => format!("{}/{}", lang, f),
            None if has("tsx") || has("jsx") => format!("{}/React", lang),
            None => label,
        };
    }
    if matches!(*lang, "Kotlin" | "Java") && (root.join("app").join("src").join("main").join("AndroidManifest.xml").exists() || root.join("gradlew").exists()) {
        label = format!("{} (Android/Gradle)", lang);
    }
    if root.join("pubspec.yaml").exists() {
        label = "Dart/Flutter".to_string();
    }
    label
}

const ENTRY_NAMES: &[&str] = &[
    "main.tsx", "main.ts", "index.tsx", "index.ts", "main.jsx", "index.jsx", "main.js", "index.js", "app.tsx", "App.tsx", "main.rs", "lib.rs", "main.py", "__main__.py", "app.py",
    "main.go", "MainActivity.kt", "Main.kt", "MainActivity.java", "Main.java", "Program.cs", "main.dart", "main.cpp", "main.c", "main.swift", "App.swift",
];

fn entry_point(root: &Path, source: &Path) -> Option<String> {
    let found: Vec<PathBuf> = entries_below(source, false).into_iter().filter(|(p, d)| !*d && p.components().count() <= 6).map(|(p, _)| p).collect();
    for want in ENTRY_NAMES {
        let mut hits: Vec<&PathBuf> = found.iter().filter(|p| p.file_name().map_or(false, |n| n.to_string_lossy().eq_ignore_ascii_case(want))).collect();
        hits.sort_by_key(|p| p.components().count());
        if let Some(p) = hits.first() {
            let full = source.join(p);
            return Some(full.strip_prefix(root).unwrap_or(&full).to_string_lossy().replace('\\', "/"));
        }
    }
    None
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// The block `mysid set` prints under the workspace line.
pub fn workspace_map(root: &Path, all: bool) -> String {
    let (dirs, loose) = child_dirs(root, all);
    let code_dirs: Vec<&(String, Counts)> = dirs.iter().filter(|(_, c)| c.code > 0).collect();
    let mut out: Vec<String> = Vec::new();

    let source = code_dirs.first().map(|(n, c)| (n.clone(), c.clone()));
    // Code files sitting in the root itself (a script project): the root is the source.
    if source.as_ref().map_or(true, |(_, c)| loose.code > c.code) && loose.code > 0 {
        let label = stack_label(root, &loose.exts);
        out.push(format!("source root: ./ ({})  - {} code file(s) in the project folder itself", label, loose.code));
    } else if let Some((name, counts)) = &source {
        let dir = root.join(name);
        let label = stack_label(root, &counts.exts);
        let entry = entry_point(root, &dir).map_or(String::new(), |e| format!(" · entry: {}", e));
        out.push(format!("source root: {}/ ({}, {} code files){}", name, label, counts.code, entry));
        // Second level, following a single dominant folder down (app/ -> app/src/main/java/...) until the code splits.
        let mut cur = dir.clone();
        let mut cur_counts = counts.clone();
        let mut shown: Vec<String> = Vec::new();
        for _ in 0..6 {
            let (kids, loose_here) = child_dirs(&cur, all);
            let kids: Vec<&(String, Counts)> = kids.iter().filter(|(_, c)| c.code > 0).collect();
            let dominant = kids.first().filter(|(_, c)| kids.len() == 1 || c.code * 10 >= cur_counts.code.saturating_sub(loose_here.code) * 9);
            if let (Some((n, c)), true) = (dominant, loose_here.code == 0) {
                cur = cur.join(n);
                cur_counts = c.clone();
                continue;
            }
            for (n, c) in &kids {
                shown.push(format!("{}/{} ({})", rel(root, &cur), n, c.code));
            }
            if loose_here.code > 0 {
                shown.push(format!("{}/*.* ({} files)", rel(root, &cur), loose_here.code));
            }
            break;
        }
        if !shown.is_empty() {
            out.push(format!("  {}", shown.join(" · ")));
        }
    } else {
        out.push("source root: (no code files found)".to_string());
    }

    // Everything else: other folders by size, then the files that are only configuration.
    let source_name = source.as_ref().map(|(n, _)| n.clone());
    let others: Vec<String> = dirs
        .iter()
        .filter(|(n, c)| Some(n) != source_name.as_ref() && c.files > 0)
        .map(|(n, c)| if c.code > 0 { format!("{}/ ({})", n, c.code) } else { format!("{}/ ({} files)", n, c.files) })
        .collect();
    let config_files = list_root_config_files(root, all);
    let mut tail: Vec<String> = Vec::new();
    if !others.is_empty() {
        tail.push(format!("other: {}", others.join(", ")));
    }
    if all {
        if !config_files.is_empty() {
            tail.push(format!("config files: {}", config_files.join(", ")));
        }
    } else if !config_files.is_empty() {
        tail.push(format!("+{} config files (--all to list)", config_files.len()));
    }
    if !tail.is_empty() {
        out.push(tail.join(" · ").replace("other: ", "other: "));
    }
    out.push(match &source {
        Some((n, _)) => format!("next: mysid list {0}/<folder>   then mysid overview <file>, mysid read <file>#L1-60, mysid graph <Symbol>   (mysid tree for the whole map)", n),
        None => "next: mysid tree   or   mysid search <text>".to_string(),
    });
    out.join("\n")
}

/// Files in the project folder that are not code: lockfiles, package.json, configs, README ... (dotfiles only with `all`).
fn list_root_config_files(root: &Path, all: bool) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(root)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_type().map_or(false, |t| t.is_file()))
                .map(|e| e.file_name().to_string_lossy().to_string())
                .filter(|n| (all || !n.starts_with('.')) && !is_code(Path::new(n)))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// `mysid tree [path] [--depth n]`: folders with their code counts, nothing else; capped.
pub fn tree(root: &Path, start: &Path, depth: usize, all: bool) -> String {
    let depth = depth.clamp(1, 4);
    let mut counts: BTreeMap<String, Counts> = BTreeMap::new();
    let mut dirs: Vec<String> = Vec::new();
    for (rel_path, is_dir) in entries_below(start, all) {
        let parts: Vec<String> = rel_path.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
        if is_dir {
            if parts.len() <= depth {
                dirs.push(parts.join("/"));
            }
            continue;
        }
        // The file counts for every folder above it that is shown.
        for i in 1..parts.len().min(depth + 1) {
            counts.entry(parts[..i].join("/")).or_default().add(&rel_path);
        }
    }
    dirs.sort();
    let head = rel(root, start);
    let mut lines: Vec<String> = vec![format!("{} (folders with code files; counts are code files below)", if head.is_empty() { ".".to_string() } else { format!("{}/", head) })];
    let mut shown = 0;
    for d in &dirs {
        let c = counts.get(d).cloned().unwrap_or_default();
        if c.files == 0 {
            continue;
        }
        if shown >= 120 {
            lines.push("... more folders: mysid tree <folder>".to_string());
            break;
        }
        shown += 1;
        let indent = "  ".repeat(d.matches('/').count() + 1);
        let name = d.rsplit('/').next().unwrap_or(d);
        lines.push(format!("{}{}/ ({}{})", indent, name, c.code, if c.code == 0 { format!(" code, {} files", c.files) } else { String::new() }));
    }
    if shown == 0 {
        lines.push("  (no sub-folders with files)".to_string());
    }
    let (_, loose) = child_dirs(start, all);
    if loose.files > 0 {
        lines.push(format!("  + {} file(s) directly here ({} code)", loose.files, loose.code));
    }
    lines.join("\n")
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 { format!("{:.1} MB", bytes as f64 / 1048576.0) } else if bytes >= 1024 { format!("{:.0} KB", bytes as f64 / 1024.0) } else { format!("{} B", bytes) }
}

/// `mysid list <folder>`: what is directly in it. Folders first, most code first, each with its code-file count; code files
/// with their line count; other files collapsed; dot-folders and build output named once at the end.
pub fn list_view(root: &Path, dir: &Path) -> String {
    let shown = rel(root, dir);
    let prefix = if shown.is_empty() { String::new() } else { format!("{}/", shown) };
    let (dirs, loose) = child_dirs(dir, false);
    let mut hidden: Vec<String> = Vec::new();
    let mut code_files: Vec<(String, usize)> = Vec::new();
    let mut other_files: Vec<(String, u64)> = Vec::new();
    let mut hidden_files = 0usize;
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if name.starts_with('.') || NOT_CODE_DIRS.contains(&name.to_lowercase().as_str()) {
                    hidden.push(format!("{}/", name));
                }
            } else if name.starts_with('.') {
                hidden_files += 1;
            } else if is_code(Path::new(&name)) {
                let lines = std::fs::metadata(e.path()).ok().filter(|m| m.len() <= 4 * 1024 * 1024).and_then(|_| std::fs::read(e.path()).ok()).map(|b| b.iter().filter(|&&c| c == b'\n').count() + usize::from(!b.is_empty() && b.last() != Some(&b'\n')));
                code_files.push((name, lines.unwrap_or(0)));
            } else {
                other_files.push((name, e.metadata().map(|m| m.len()).unwrap_or(0)));
            }
        }
    }
    let total_code: usize = dirs.iter().map(|(_, c)| c.code).sum::<usize>() + loose.code;
    let mut out = vec![format!(
        "{} - {} folder(s), {} file(s) here; {} code file(s) in all",
        if shown.is_empty() { ".".to_string() } else { format!("{}/", shown) },
        dirs.len(),
        code_files.len() + other_files.len(),
        total_code
    )];
    for (name, c) in &dirs {
        out.push(if c.code > 0 { format!("D {}{}/ ({} code files)", prefix, name, c.code) } else if c.files > 0 { format!("D {}{}/ ({} files)", prefix, name, c.files) } else { format!("D {}{}/ (empty)", prefix, name) });
    }
    for (name, lines) in &code_files {
        out.push(format!("F {}{} ({} lines)", prefix, name, lines));
    }
    for (name, size) in other_files.iter().take(8) {
        out.push(format!("F {}{} ({})", prefix, name, human_size(*size)));
    }
    if other_files.len() > 8 {
        out.push(format!("+{} more non-code files (lockfiles, configs, assets)", other_files.len() - 8));
    }
    if !hidden.is_empty() || hidden_files > 0 {
        let mut parts = hidden;
        if hidden_files > 0 {
            parts.push(format!("{} dot-file(s)", hidden_files));
        }
        out.push(format!("not listed (hidden or build output): {}   (mysid list {} --depth 1 shows every entry)", parts.join(", "), if shown.is_empty() { "." } else { &shown }));
    }
    out.join("\n")
}

pub const TREE_HELP: &str ="mysid tree: the folder map of the project (read only)

Usage:
  mysid tree                 folders two levels deep, each with its number of code files
  mysid tree src/components  the same for one folder
  mysid tree --depth 3       deeper (up to 4)
  mysid tree --all           also dot-folders (.github ...); build output and node_modules are always left out

Folders only, counts only, capped at 120 lines: use it to choose the folder for mysid overview <folder>.
";

pub fn execute_tree(params: &serde_json::Value, default_root: Option<&str>) -> String {
    let fail = |m: String| serde_json::json!({ "success": false, "error": m }).to_string();
    let ws = match super::workspace::resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    let start = match params.get("path").and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        Some(p) => super::workspace::resolve_or_join(root, p.trim(), &ws),
        None => root.to_path_buf(),
    };
    if !start.is_dir() {
        return fail(format!("not a folder: {}", start.display()));
    }
    let depth = params.get("depth").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(2) as usize;
    let all = matches!(params.get("all"), Some(serde_json::Value::Bool(true))) || matches!(params.get("all"), Some(serde_json::Value::String(s)) if s != "false" && !s.is_empty());
    serde_json::json!({ "success": true, "output": tree(root, &start, depth, all) }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn react_project(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("mysid_orient_{}_{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&root);
        put(&root, "package.json", "{\"dependencies\":{\"react\":\"18\"}}");
        put(&root, "package-lock.json", "{}");
        put(&root, "tsconfig.json", "{}");
        put(&root, "index.html", "<html>");
        put(&root, ".gitignore", "dist\n");
        for i in 0..5 {
            put(&root, &format!("src/components/C{}.tsx", i), "export const C = 1");
        }
        for i in 0..2 {
            put(&root, &format!("src/pages/P{}.tsx", i), "export const P = 1");
        }
        put(&root, "src/main.tsx", "import './x'");
        put(&root, "plan/notes.md", "# plan");
        put(&root, "public/logo.png", "x");
        put(&root, "node_modules/x/index.js", "module.exports = 1");
        put(&root, ".git/config", "x");
        root
    }

    #[test]
    fn the_map_names_the_source_root_its_language_entry_and_sub_folders() {
        let root = react_project("map");
        let m = workspace_map(&root, false);
        assert!(m.contains("source root: src/ (TypeScript/React, 8 code files) · entry: src/main.tsx"), "{}", m);
        assert!(m.contains("src/components (5)") && m.contains("src/pages (2)") && m.contains("src/*.* (1 files)"), "{}", m);
        assert!(m.contains("other:") && m.contains("plan/ (1 files)") && m.contains("public/ (1 files)"), "{}", m);
        assert!(!m.contains("node_modules") && !m.contains(".git"), "{}", m);
        assert!(m.contains("config files (--all to list)") && !m.contains("package-lock.json"), "never lists lockfiles by default: {}", m);
        assert!(!m.contains("more"), "{}", m);
        assert!(m.contains("next: mysid list src/<folder>"), "{}", m);
        // --all lists the configuration files.
        let all = workspace_map(&root, true);
        assert!(all.contains("config files:") && all.contains("package-lock.json") && all.contains(".gitignore"), "{}", all);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_single_dominant_folder_chain_is_followed_down_to_where_the_code_splits() {
        let root = std::env::temp_dir().join(format!("mysid_orient_android_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        put(&root, "gradlew", "");
        put(&root, "app/src/main/AndroidManifest.xml", "<manifest/>");
        for i in 0..4 {
            put(&root, &format!("app/src/main/java/com/x/ui/S{}.kt", i), "class S");
        }
        for i in 0..3 {
            put(&root, &format!("app/src/main/java/com/x/data/D{}.kt", i), "class D");
        }
        put(&root, "app/src/main/java/com/x/MainActivity.kt", "class MainActivity");
        let m = workspace_map(&root, false);
        assert!(m.contains("Kotlin (Android/Gradle)") && m.contains("entry: app/src/main/java/com/x/MainActivity.kt"), "{}", m);
        assert!(m.contains("app/src/main/java/com/x/ui (4)") && m.contains("app/src/main/java/com/x/data (3)"), "{}", m);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_shows_folders_by_code_then_code_files_with_lines_and_hides_the_noise() {
        let root = react_project("list");
        let l = list_view(&root, &root.join("src"));
        assert!(l.starts_with("src/ - 2 folder(s), 1 file(s) here; 8 code file(s) in all"), "{}", l);
        assert!(l.contains("D src/components/ (5 code files)") && l.contains("D src/pages/ (2 code files)"), "{}", l);
        assert!(l.find("components").unwrap() < l.find("pages").unwrap(), "most code first: {}", l);
        assert!(l.contains("F src/main.tsx (1 lines)"), "{}", l);
        let top = list_view(&root, &root);
        assert!(top.contains("D plan/ (1 files)") && top.contains("F package-lock.json") && top.contains("F index.html (1 lines)"), "{}", top);
        assert!(top.contains("not listed (hidden or build output): .git/, node_modules/") && !top.contains("D .git"), "{}", top);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_tree_is_folders_and_counts_only() {
        let root = react_project("tree");
        let t = tree(&root, &root, 2, false);
        assert!(t.contains("src/ (8)") && t.contains("components/ (5)") && t.contains("pages/ (2)"), "{}", t);
        assert!(t.contains("plan/ (0 code, 1 files)"), "{}", t);
        assert!(!t.contains("C0.tsx") && !t.contains("node_modules"), "{}", t);
        let sub = tree(&root, &root.join("src"), 1, false);
        assert!(sub.starts_with("src/ ") && sub.contains("components/ (5)") && sub.contains("+ 1 file(s) directly here"), "{}", sub);
        let _ = fs::remove_dir_all(&root);
    }
}
