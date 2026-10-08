use ignore::WalkBuilder;
use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use super::model::CodeGraph;
use crate::parsers::find_parser;

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "venv", ".venv", "__pycache__", "target", "build", "dist", ".gradle",
    ".idea", ".vscode",
];

/// Identifiers followed by `(` that are language constructs or logging macros, never call targets.
const NOT_CALLS: &[&str] = &[
    "if", "for", "while", "switch", "match", "return", "catch", "fn", "def", "function", "class",
    "sizeof", "typeof", "new", "await", "async", "else", "loop", "when", "with", "assert", "super",
    "this", "self", "LOGD", "LOGE", "LOGI", "LOGW",
];

/// The Rust parser reports `impl Foo` / `impl<T> Trait for Foo<T>`; the owning type is `Foo`.
fn canonical_type(name: &str) -> String {
    match name.strip_prefix("impl ") {
        Some(rest) => {
            let target = rest.rsplit(" for ").next().unwrap_or(rest);
            target.split('<').next().unwrap_or(target).trim().to_string()
        }
        None => name.to_string(),
    }
}

/// Method names so common in std/collection APIs that a `.name(` call is almost never the
/// user's own method of that name; such calls only link within the same file.
const COMMON_METHODS: &[&str] = &[
    "push", "pop", "get", "set", "insert", "remove", "len", "is_empty", "new", "map", "iter", "clone",
    "unwrap", "contains", "join", "next", "take", "skip", "find", "filter", "collect", "from", "into",
    "to_string", "as_str", "trim", "split", "replace", "write", "read", "push_str", "extend", "clear",
    "first", "last", "keys", "values", "entry", "min", "max", "sort", "dedup", "flush", "lock", "open",
    "close", "run", "add", "delete", "update", "parse", "format", "print", "log", "then", "catch",
    "forEach", "includes", "append", "count", "start", "stop", "emit", "send", "call", "apply", "bind",
    "ok", "err", "default", "build", "init", "exists", "starts_with", "ends_with", "chars", "lines",
];

pub fn build_graph(root: &Path) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let call_re = Regex::new(r"(\.)?\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
    let jsx_re = Regex::new(r"<([A-Z][A-Za-z0-9_]*)").unwrap();
    let mut all_calls: Vec<(String, String, bool)> = Vec::new(); // (caller_node_id, callee_name, via `.`)

    for (rel_path, full_path) in collect_source_files(root) {
        let ext = full_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        let parser = match find_parser(&ext) {
            Some(p) if p.language_name() != "Markdown" => p,
            _ => continue,
        };
        let content = match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let lines: Vec<&str> = content.lines().collect();
        let (types, methods) = parser.parse(&lines);

        let file_id = format!("file:{}", rel_path);
        graph.add_node(file_id.clone(), "file".into(), rel_path.clone(), rel_path.clone(), 1);

        // Types first, so methods can attach to their owning class node.
        for t in &types {
            let type_name = canonical_type(&t.name);
            let class_id = format!("{}:{}", rel_path, type_name);
            // `impl Foo` after `struct Foo` maps to the same node: keep the first definition.
            if graph.nodes.contains_key(&class_id) {
                continue;
            }
            graph.add_node(class_id.clone(), "class".into(), type_name, rel_path.clone(), t.line);
            graph.add_edge(file_id.clone(), class_id, "DEFINES".into());
        }

        // Body of a symbol runs until the next symbol starts (any type or method).
        let mut boundaries: Vec<usize> = types
            .iter()
            .map(|t| t.line)
            .chain(methods.iter().map(|m| m.start_line))
            .collect();
        boundaries.sort_unstable();
        boundaries.dedup();

        let braces = parser.language_name() != "Python";
        let is_web = matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx");

        for m in &methods {
            let (func_id, ntype, owner_id) = match &m.parent {
                Some(p) => {
                    let class_id = format!("{}:{}", rel_path, canonical_type(p));
                    let owner = if graph.nodes.contains_key(&class_id) { class_id.clone() } else { file_id.clone() };
                    (format!("{}.{}", class_id, m.name), "method", owner)
                }
                None => (format!("{}:{}", rel_path, m.name), "function", file_id.clone()),
            };

            if !graph.nodes.contains_key(&func_id) {
                graph.add_node(func_id.clone(), ntype.into(), m.name.clone(), rel_path.clone(), m.start_line);
                graph.add_edge(owner_id, func_id.clone(), "DEFINES".into());
            }

            let start = m.start_line.saturating_sub(1);
            let next_start = boundaries
                .iter()
                .find(|&&b| b > m.start_line)
                .map(|b| b - 1)
                .unwrap_or(lines.len());
            // The real block, nested symbols included: a component's JSX often sits after its
            // inner handlers, so cutting at the next symbol would credit calls to the wrong one.
            let block_end = if braces {
                brace_block_end(&lines, start, next_start)
            } else {
                indent_block_end(&lines, start)
            };
            let end = block_end.unwrap_or(next_start);

            for line in &lines[start..end.min(lines.len())] {
                let trimmed = line.trim();
                if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
                    continue;
                }
                for cap in call_re.captures_iter(line) {
                    let callee = &cap[2];
                    if callee != m.name && !NOT_CALLS.contains(&callee) {
                        all_calls.push((func_id.clone(), callee.to_string(), cap.get(1).is_some()));
                    }
                }
                if is_web {
                    for cap in jsx_re.captures_iter(line) {
                        if &cap[1] != m.name.as_str() {
                            all_calls.push((func_id.clone(), cap[1].to_string(), false));
                        }
                    }
                }
            }
        }
    }

    resolve_calls(&mut graph, all_calls);
    graph
}

/// Last line (exclusive) of the `{ ... }` block that opens before `opening_limit`
/// (the next symbol's line). None when the symbol has no block (e.g. a one-line arrow function).
fn brace_block_end(lines: &[&str], start: usize, opening_limit: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !opened && i >= opening_limit {
            return None;
        }
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some(i + 1);
        }
    }
    None
}

/// Python: the body ends at the next non-blank line indented no deeper than the `def` line.
fn indent_block_end(lines: &[&str], start: usize) -> Option<usize> {
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let base = indent_of(lines.get(start)?);
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if !line.trim().is_empty() && indent_of(line) <= base {
            return Some(i);
        }
    }
    Some(lines.len())
}

/// Turns raw callee names into CALLS edges. Prefers a same-file target; otherwise links only
/// when the name is unique workspace-wide, so common names (`new`, `get`) don't create noise.
fn resolve_calls(graph: &mut CodeGraph, all_calls: Vec<(String, String, bool)>) {
    let mut seen: HashSet<(String, String)> = HashSet::new();
    let mut edges: Vec<(String, String)> = Vec::new();

    for (caller_id, callee_name, dotted) in all_calls {
        let Some(candidates) = graph.symbol_to_nodes.get(&callee_name) else { continue };
        let caller_file = graph.nodes.get(&caller_id).map(|n| n.file.as_str()).unwrap_or("");

        let mut uniq: Vec<&String> = Vec::new();
        for id in candidates {
            let is_symbol = graph.nodes.get(id).map(|n| n.node_type != "file").unwrap_or(false);
            if is_symbol && !uniq.contains(&id) {
                uniq.push(id);
            }
        }

        let target = uniq
            .iter()
            .find(|id| graph.nodes.get(**id).map(|n| n.file == caller_file).unwrap_or(false))
            .or_else(|| {
                let common = dotted && COMMON_METHODS.contains(&callee_name.as_str());
                if uniq.len() == 1 && !common { uniq.first() } else { None }
            });

        if let Some(target) = target {
            if **target != caller_id && seen.insert((caller_id.clone(), (*target).clone())) {
                edges.push((caller_id, (*target).clone()));
            }
        }
    }

    for (src, target) in edges {
        graph.add_edge(src, target, "CALLS".into());
    }
}

fn collect_source_files(root: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = WalkBuilder::new(root)
        .filter_entry(|e| e.file_name().to_str().map_or(true, |n| !SKIP_DIRS.contains(&n)))
        .build()
        .filter_map(|r| r.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let rel = e.path().strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/");
            Some((rel, e.path().to_path_buf()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}
