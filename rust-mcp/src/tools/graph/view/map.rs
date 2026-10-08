use std::collections::{BTreeMap, HashMap};

use super::Out;
use crate::tools::graph::model::{CodeGraph, Node};
use crate::tools::graph::query::label;

const MAX_LINES: usize = 40;
const NAMES_PER_FILE: usize = 5;

fn norm(path: &str) -> String {
    path.trim().replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string()
}

fn file_nodes(graph: &CodeGraph) -> Vec<&Node> {
    let mut files: Vec<&Node> = graph.nodes.values().filter(|n| n.node_type == "file").collect();
    files.sort_by(|a, b| a.file.cmp(&b.file));
    files
}

/// True when `path` names a file or a directory that contains scanned files.
pub fn matches_path(graph: &CodeGraph, path: &str) -> bool {
    let p = norm(path);
    let dir = format!("{}/", p);
    !p.is_empty() && file_nodes(graph).iter().any(|f| f.file == p || f.file.starts_with(&dir))
}

/// Orientation map. No path: directory rollup. A directory: its children. A file: its symbols.
pub fn render(graph: &CodeGraph, root_name: &str, path: Option<&str>) -> String {
    let files = file_nodes(graph);
    let mut by_file: HashMap<&str, Vec<&Node>> = HashMap::new();
    for n in graph.nodes.values().filter(|n| n.node_type != "file") {
        by_file.entry(n.file.as_str()).or_default().push(n);
    }
    for list in by_file.values_mut() {
        list.sort_by(|a, b| (a.line, &a.id).cmp(&(b.line, &b.id)));
    }

    let p = path.map(norm).unwrap_or_default();
    let mut out = Out::new(MAX_LINES);

    if let Some(syms) = by_file.get(p.as_str()).filter(|_| !p.is_empty()) {
        out.push(format!("{} - {} symbols", p, syms.len()));
        for s in syms {
            if !out.push(format!("  {} {}  :{}", s.node_type, label(s), s.line)) {
                break;
            }
        }
        return out.finish();
    }

    let dir_prefix = if p.is_empty() { String::new() } else { format!("{}/", p) };
    let scoped: Vec<&Node> = files.iter().copied().filter(|f| f.file.starts_with(&dir_prefix)).collect();
    let total_syms: usize = scoped.iter().map(|f| by_file.get(f.file.as_str()).map_or(0, |v| v.len())).sum();

    // Skip single-child directory chains (src/ -> src/Pages/) so the first line shown is useful.
    let mut base = dir_prefix.clone();
    loop {
        let firsts: Vec<&str> = scoped
            .iter()
            .filter_map(|f| f.file[base.len()..].split_once('/').map(|(d, _)| d))
            .collect();
        let all_nested = firsts.len() == scoped.len() && !firsts.is_empty();
        if all_nested && firsts.iter().all(|d| *d == firsts[0]) {
            base = format!("{}{}/", base, firsts[0]);
        } else {
            break;
        }
    }

    let location = if base.is_empty() { root_name.to_string() } else { base.trim_end_matches('/').to_string() };
    out.push(format!("{} - {} files, {} symbols", location, scoped.len(), total_syms));

    // Group by the first path component below `base`.
    let mut dirs: BTreeMap<String, (usize, usize)> = BTreeMap::new(); // name -> (files, symbols)
    let mut direct: Vec<&Node> = Vec::new();
    for f in &scoped {
        let rel = &f.file[base.len()..];
        let syms = by_file.get(f.file.as_str()).map_or(0, |v| v.len());
        match rel.split_once('/') {
            Some((d, _)) => {
                let e = dirs.entry(d.to_string()).or_insert((0, 0));
                e.0 += 1;
                e.1 += syms;
            }
            None => direct.push(f),
        }
    }

    for (name, (nf, ns)) in &dirs {
        if !out.push(format!("  {}{}/  {} files, {} symbols", base, name, nf, ns)) {
            break;
        }
    }
    for f in direct {
        let defined = top_level_names(graph, f);
        let shown: Vec<&str> = defined.iter().take(NAMES_PER_FILE).map(|s| s.as_str()).collect();
        let extra = defined.len().saturating_sub(NAMES_PER_FILE);
        let tail = if extra > 0 { format!(" +{}", extra) } else { String::new() };
        if !out.push(format!("  {}  {}{}", f.file, shown.join(", "), tail)) {
            break;
        }
    }

    out.push("next: mysid graph <dir|file> to drill down, mysid graph <Symbol> for callers/callees");
    out.finish()
}

/// Classes and free functions defined directly in the file (methods excluded), in source order.
fn top_level_names(graph: &CodeGraph, file: &Node) -> Vec<String> {
    let mut defined: Vec<&Node> = graph
        .forward_edges
        .get(&file.id)
        .into_iter()
        .flatten()
        .filter(|(_, rel)| rel == "DEFINES")
        .filter_map(|(id, _)| graph.nodes.get(id))
        .collect();
    defined.sort_by_key(|n| n.line);
    defined.into_iter().map(|n| n.name.clone()).collect()
}
