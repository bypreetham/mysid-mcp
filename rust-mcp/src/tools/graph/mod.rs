mod build;
mod model;
mod query;
mod view;

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::workspace::resolve_existing_root;
use crate::index::{Index, Located, SymbolId};
use build::{full_graph, partial_graph};
use model::Node;
use query::{label, loc, resolve, Resolved};

const MAX_DEPTH: u64 = 5;

/// `mysid graph [symbol] [--depth N] [--flow]`
///
/// - no symbol:  orientation map of the workspace (or of a directory/file path)
/// - symbol:     callers + symbol + callees (1 hop by default, `--depth N` for more)
/// - `--flow`:   same relationships, drawn as UPSTREAM / TARGET / DOWNSTREAM trees
///
/// A symbol lookup reads only what it needs from the index (the matching symbols, then the
/// `depth`-hop neighborhood), so its cost does not grow with the size of the project.
pub fn execute_graph(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let target = ["symbol", "query", "entry", "path", "filter"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let flow = params.get("flow").and_then(|v| v.as_bool()).unwrap_or(false);
    let depth = params.get("depth").and_then(|v| v.as_u64()).unwrap_or(1).clamp(1, MAX_DEPTH) as usize;

    let root = Path::new(&workspace_root);
    let t_open = std::time::Instant::now();
    let index = Index::open(root);
    if crate::index::timing_enabled() {
        eprintln!("[timing] open + refresh index: {:?}", t_open.elapsed());
    }
    let t_query = std::time::Instant::now();
    let root_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("workspace");

    let output = match target {
        None => view::map::render(&full_graph(&index), root_name, None),
        Some(q) => lookup(&index, &q, flow, depth, root_name),
    };

    if crate::index::timing_enabled() {
        eprintln!("[timing] query + render: {:?}", t_query.elapsed());
    }
    json!({ "success": true, "output": output }).to_string()
}

fn qualified(l: &Located) -> String {
    match &l.symbol.owner {
        Some(o) => format!("{}.{}", o, l.symbol.name),
        None => l.symbol.name.clone(),
    }
}

/// A C++ prototype in a header and its definition in a .cpp are one function: keep the definition.
fn collapse_declarations(found: Vec<Located>) -> Vec<Located> {
    let defined: HashSet<(Option<String>, String)> =
        found.iter().filter(|l| l.symbol.has_body).map(|l| (l.symbol.owner.clone(), l.symbol.name.clone())).collect();
    found.into_iter().filter(|l| l.symbol.has_body || !defined.contains(&(l.symbol.owner.clone(), l.symbol.name.clone()))).collect()
}

fn capped(items: &BTreeSet<String>, n: usize) -> String {
    let shown: Vec<&str> = items.iter().take(n).map(String::as_str).collect();
    if items.len() > n { format!("{}, +{}", shown.join(", "), items.len() - n) } else { shown.join(", ") }
}

/// `mysid graph File.kt`: what the file defines, which other files it calls into, and which files call it.
fn file_card(index: &Index, q: &str) -> Option<String> {
    let ext = q.rsplit('.').next().unwrap_or("");
    if !q.contains('.') || !crate::parsers::is_supported_extension(&ext.to_lowercase()) {
        return None;
    }
    let files = index.files_named(q).ok()?;
    if files.is_empty() {
        return None;
    }
    if files.len() > 1 {
        let list: Vec<String> = files.iter().take(8).map(|f| format!("  {}", f.path)).collect();
        return Some(format!("'{}' is {} files - repeat with a folder, e.g. mysid graph {}:\n{}", q, files.len(), files[0].path, list.join("\n")));
    }
    let f = &files[0];
    let syms = index.symbols_in_file(f.id).ok()?;
    let mut out = vec![format!("{}  ({}, {} lines, {} symbols)", f.path, f.language, f.lines, syms.len())];
    if syms.is_empty() {
        return Some(out.join("\n"));
    }
    // Types with their method counts, and the free functions; the full outline is `mysid overview`.
    let types: Vec<String> = syms
        .iter()
        .filter(|l| l.symbol.kind == crate::index::SymbolKind::Class)
        .take(12)
        .map(|l| {
            let members = syms.iter().filter(|m| m.symbol.owner.as_deref() == Some(l.symbol.name.as_str())).count();
            format!("{}:{} ({} method{})", l.symbol.name, l.symbol.start_line, members, if members == 1 { "" } else { "s" })
        })
        .collect();
    let free: Vec<String> = syms.iter().filter(|l| l.symbol.owner.is_none() && l.symbol.kind != crate::index::SymbolKind::Class).take(10).map(|l| format!("{}:{}", l.symbol.name, l.symbol.start_line)).collect();
    let mut defines = types;
    if !free.is_empty() {
        defines.push(format!("functions {}", free.join(", ")));
    }
    out.push(format!("DEFINES: {}   (outline: mysid overview {})", defines.join("; "), q));

    let ids: HashSet<SymbolId> = syms.iter().map(|l| l.symbol.id).collect();
    let seeds: Vec<SymbolId> = ids.iter().copied().collect();
    let hood = index.neighborhood(&seeds, 1).ok()?;
    let by_id: HashMap<SymbolId, &Located> = hood.symbols.iter().map(|l| (l.symbol.id, l)).collect();
    // other file -> (what it defines that this file calls, who in this file calls it)
    let mut calls_out: BTreeMap<String, (BTreeSet<String>, BTreeSet<String>)> = BTreeMap::new();
    let mut called_by: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut inside = 0usize;
    for e in &hood.edges {
        let (Some(from), Some(to)) = (by_id.get(&e.from), by_id.get(&e.to)) else { continue };
        match (ids.contains(&e.from), ids.contains(&e.to)) {
            (true, true) => inside += 1,
            (true, false) => {
                let jni = if from.symbol.detail == "native" { " (JNI)" } else { "" };
                let entry = calls_out.entry(to.path.clone()).or_default();
                entry.0.insert(format!("{}{}", qualified(to), jni));
                entry.1.insert(qualified(from));
            }
            (false, true) => {
                called_by.entry(from.path.clone()).or_default().insert(format!("{} -> {}", qualified(from), qualified(to)));
            }
            _ => {}
        }
    }
    if !calls_out.is_empty() {
        out.push(format!("CALLS OUT (to {} file{}):", calls_out.len(), if calls_out.len() == 1 { "" } else { "s" }));
        for (path, (callees, callers)) in calls_out.iter().take(14) {
            out.push(format!("  {}: {}   (from {})", path, capped(callees, 6), capped(callers, 4)));
        }
        if calls_out.len() > 14 {
            out.push(format!("  +{} more files", calls_out.len() - 14));
        }
    }
    if !called_by.is_empty() {
        out.push(format!("CALLED BY (from {} file{}):", called_by.len(), if called_by.len() == 1 { "" } else { "s" }));
        for (path, calls) in called_by.iter().take(14) {
            out.push(format!("  {}: {}", path, capped(calls, 5)));
        }
        if called_by.len() > 14 {
            out.push(format!("  +{} more files", called_by.len() - 14));
        }
    }
    if calls_out.is_empty() && called_by.is_empty() {
        out.push("no calls to or from other files in the index".to_string());
    }
    out.push(format!("calls within the file: {}   (one symbol: mysid graph <Class.method>)", inside));
    Some(out.join("\n"))
}

fn lookup(index: &Index, q: &str, flow: bool, depth: usize, root_name: &str) -> String {
    if let Some(card) = file_card(index, q) {
        return card;
    }
    let candidates = match index.candidates(q) {
        Ok(c) => collapse_declarations(c),
        Err(e) => return format!("ERROR: index query failed: {}", e),
    };
    // Resolve the user's words to one symbol using only the candidates the index returned.
    let matches = partial_graph(&candidates, &[]);

    match resolve(&matches.graph, q) {
        Resolved::One(node) => {
            let Some(&seed) = matches.ids.get(&node.id) else {
                return format!("'{}' could not be located in the index.", q);
            };
            let mut seeds = vec![seed];
            if node.node_type == "class" {
                // A class stands for itself and its methods.
                if let Some(class) = candidates.iter().find(|l| l.symbol.id == seed) {
                    seeds.extend(index.members(class).unwrap_or_default().iter().map(|m| m.symbol.id));
                }
            }
            let hood = match index.neighborhood(&seeds, depth) {
                Ok(h) => h,
                Err(e) => return format!("ERROR: index query failed: {}", e),
            };
            let part = partial_graph(&hood.symbols, &hood.edges);
            let Some(target) = part.graph.nodes.get(&node.id) else {
                return format!("'{}' could not be located in the index.", q);
            };
            if flow {
                view::flow::render(&part.graph, target, depth)
            } else {
                view::card::render(&part.graph, target, depth)
            }
        }
        Resolved::Many(nodes) => ambiguous(q, &nodes),
        // Not a symbol: a directory or file path drills into the map instead.
        Resolved::Similar(_) | Resolved::NotFound if index.has_path(q).unwrap_or(false) => {
            view::map::render(&full_graph(index), root_name, Some(q))
        }
        Resolved::Similar(nodes) => format!(
            "No symbol named '{}'. Similar:\n{}",
            q,
            nodes.iter().map(|n| format!("  {}  {}", label(n), loc(n))).collect::<Vec<_>>().join("\n")
        ),
        Resolved::NotFound => format!(
            "'{}' not found as a symbol or path. Run `mysid graph` for a map, or `mysid search {}`.",
            q, q
        ),
    }
}

fn ambiguous(query: &str, nodes: &[&Node]) -> String {
    let mut lines = vec![format!(
        "'{}' matches {} symbols - repeat with Class.method or file:Name:",
        query,
        nodes.len()
    )];
    for n in nodes.iter().take(8) {
        lines.push(format!("  {}  [{}]  {}", label(n), n.node_type, loc(n)));
    }
    if nodes.len() > 8 {
        lines.push(format!("  +{} more", nodes.len() - 8));
    }
    lines.join("\n")
}

#[cfg(test)]
mod file_card_tests {
    use super::*;
    use std::fs;

    #[test]
    fn a_file_name_gives_the_file_card_and_a_prototype_gives_way_to_its_definition() {
        let dir = std::env::temp_dir().join(format!("mysid_filecard_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("Player.kt"), "class Player(private val engine: Engine) {\n    fun play() {\n        engine.start()\n    }\n}\n").unwrap();
        fs::write(dir.join("Engine.kt"), "class Engine {\n    fun start() {\n    }\n}\n").unwrap();
        fs::write(dir.join("Main.kt"), "class Main(private val player: Player) {\n    fun go() {\n        player.play()\n    }\n}\n").unwrap();
        fs::write(dir.join("engine.h"), "class Core {\npublic:\n    void run();\n};\n").unwrap();
        fs::write(dir.join("engine.cpp"), "void Core::run() {\n}\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let graph = |q: &str| {
            let raw = execute_graph(&json!({ "workspace_root": root, "query": q }), Some(&root));
            serde_json::from_str::<Value>(&raw).unwrap()["output"].as_str().unwrap().to_string()
        };

        let card = graph("Player.kt");
        assert!(card.starts_with("Player.kt  (Kotlin,"), "{}", card);
        assert!(card.contains("DEFINES: Player:1 (1 method)") && card.contains("CALLS OUT (to 1 file):") && card.contains("Engine.kt: Engine.start   (from Player.play)"), "{}", card);
        assert!(card.contains("CALLED BY (from 1 file):") && card.contains("Main.kt: Main.go -> Player.play"), "{}", card);

        // One match, not two: the header prototype and the definition are the same function.
        let run = graph("Core.run");
        assert!(run.starts_with("Core.run [method]") && run.contains("engine.cpp"), "{}", run);
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod map_tests {
    use super::*;
    use std::fs;

    fn project() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mysid_map_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join("app/build.gradle.kts").parent().map(|p| { fs::create_dir_all(p).unwrap(); p.join("build.gradle.kts") }).unwrap(), "// build\n").unwrap();
        for (folder, n) in [("ui", 14), ("data", 14), ("util", 3)] {
            let d = dir.join("app/src/main/java/com/x").join(folder);
            fs::create_dir_all(&d).unwrap();
            for i in 0..n {
                fs::write(d.join(format!("{}{}.kt", folder, i)), format!("class {}{} {{ fun run() {{ }} }}\n", folder, i)).unwrap();
            }
        }
        dir
    }

    #[test]
    fn the_project_map_opens_up_to_the_folders_where_the_code_lives_and_a_folder_gets_its_own_map() {
        let dir = project();
        let root = dir.to_string_lossy().to_string();
        let map = execute_graph(&json!({ "workspace_root": root }), Some(&root));
        assert!(map.contains("app/src/main/java/com/x/ui/  14 files"), "{}", map);
        assert!(map.contains("app/src/main/java/com/x/data/  14 files"), "{}", map);
        assert!(map.contains("app/src/main/java/com/x/util/  3 files"), "{}", map);
        assert!(!map.lines().any(|l| l.trim_start().starts_with("app/  ")), "should not stop at app/: {}", map);

        // `read <folder>` says what to use for a folder instead of printing something that looks like a file
        let read = crate::tools::read::execute_read(&json!({ "workspace_root": root, "project_path": root, "path": "app" }), Some(&root));
        assert!(read.starts_with("ERROR: 'app' is a folder, not a file") && read.contains("mysid list app"), "{}", read);
        let _ = fs::remove_dir_all(&dir);
    }
}
