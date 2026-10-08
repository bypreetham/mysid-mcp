use std::collections::HashSet;

use super::model::{CodeGraph, Node};

#[derive(Clone, Copy, PartialEq)]
pub enum Dir {
    /// Callers of a symbol.
    Up,
    /// Callees of a symbol.
    Down,
}

pub enum Resolved<'a> {
    One(&'a Node),
    Many(Vec<&'a Node>),
    Similar(Vec<&'a Node>),
    NotFound,
}

pub struct Hit<'a> {
    pub node: &'a Node,
    pub hop: usize,
}

/// `Class.method` / `file:Name` style label: the node id without its file prefix.
pub fn label(n: &Node) -> String {
    n.id
        .strip_prefix(&format!("{}:", n.file))
        .unwrap_or(&n.name)
        .to_string()
}

pub fn loc(n: &Node) -> String {
    format!("{}:{}", n.file, n.line)
}

fn sort_nodes(nodes: &mut Vec<&Node>) {
    nodes.sort_by(|a, b| (&a.file, a.line, &a.id).cmp(&(&b.file, b.line, &b.id)));
    nodes.dedup_by(|a, b| a.id == b.id);
}

/// Finds the symbol a user typed: exact name, `Class.method` / `Class::method`, or `file:Name`.
pub fn resolve<'a>(graph: &'a CodeGraph, query: &str) -> Resolved<'a> {
    let q = query.trim().replace("::", ".");
    let ql = q.to_lowercase();

    let mut found: Vec<&Node> = graph
        .symbol_to_nodes
        .get(&q)
        .or_else(|| graph.symbol_to_nodes.get(&ql))
        .into_iter()
        .flatten()
        .filter_map(|id| graph.nodes.get(id))
        .filter(|n| n.node_type != "file")
        .collect();

    if found.is_empty() && (q.contains('.') || q.contains(':') || q.contains('/')) {
        found = graph
            .nodes
            .values()
            .filter(|n| n.node_type != "file")
            .filter(|n| {
                let id = n.id.to_lowercase();
                id == ql || id.ends_with(&format!(":{}", ql)) || id.ends_with(&format!("/{}", ql))
            })
            .collect();
    }
    sort_nodes(&mut found);

    // A class plus its same-named constructor is one symbol, not an ambiguity.
    if found.len() > 1 {
        if let Some(class) = found.iter().find(|n| n.node_type == "class") {
            let prefix = format!("{}.", class.id);
            if found.iter().all(|n| n.id == class.id || n.id.starts_with(&prefix)) {
                return Resolved::One(class);
            }
        }
    }

    match found.len() {
        1 => Resolved::One(found[0]),
        0 => {
            let mut similar: Vec<&Node> = graph
                .nodes
                .values()
                .filter(|n| n.node_type != "file" && n.name.to_lowercase().contains(&ql))
                .collect();
            sort_nodes(&mut similar);
            similar.truncate(8);
            if similar.is_empty() {
                Resolved::NotFound
            } else {
                Resolved::Similar(similar)
            }
        }
        _ => Resolved::Many(found),
    }
}

/// The symbol itself, plus all of its methods when it is a class.
pub fn target_set(graph: &CodeGraph, root: &Node) -> HashSet<String> {
    let mut set = HashSet::new();
    set.insert(root.id.clone());
    if root.node_type == "class" {
        let prefix = format!("{}.", root.id);
        for n in graph.nodes.values() {
            if n.node_type == "method" && n.id.starts_with(&prefix) {
                set.insert(n.id.clone());
            }
        }
    }
    set
}

pub fn members<'a>(graph: &'a CodeGraph, root: &Node) -> Vec<&'a Node> {
    let prefix = format!("{}.", root.id);
    let mut out: Vec<&Node> = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "method" && n.id.starts_with(&prefix))
        .collect();
    sort_nodes(&mut out);
    out
}

/// Direct CALLS neighbors of one node.
pub fn neighbors<'a>(graph: &'a CodeGraph, id: &str, dir: Dir) -> Vec<&'a Node> {
    let edges = match dir {
        Dir::Up => graph.reverse_edges.get(id),
        Dir::Down => graph.forward_edges.get(id),
    };
    let mut out: Vec<&Node> = edges
        .into_iter()
        .flatten()
        .filter(|(_, rel)| rel == "CALLS")
        .filter_map(|(other, _)| graph.nodes.get(other))
        .collect();
    sort_nodes(&mut out);
    out
}

/// Direct CALLS neighbors of every member of `set`, excluding the members themselves.
pub fn set_neighbors<'a>(graph: &'a CodeGraph, set: &HashSet<String>, dir: Dir) -> Vec<&'a Node> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<&Node> = Vec::new();
    for id in set {
        for n in neighbors(graph, id, dir) {
            if !set.contains(&n.id) && seen.insert(n.id.as_str()) {
                out.push(n);
            }
        }
    }
    sort_nodes(&mut out);
    out
}

/// Breadth-first neighbors up to `depth` hops, nearest first.
pub fn collect<'a>(graph: &'a CodeGraph, set: &HashSet<String>, dir: Dir, depth: usize) -> Vec<Hit<'a>> {
    let mut visited: HashSet<String> = set.clone();
    let mut out = Vec::new();
    let mut frontier = set_neighbors(graph, set, dir);
    for hop in 1..=depth {
        sort_nodes(&mut frontier);
        let mut next: Vec<&Node> = Vec::new();
        for n in frontier {
            if visited.insert(n.id.clone()) {
                out.push(Hit { node: n, hop });
                next.extend(neighbors(graph, &n.id, dir));
            }
        }
        frontier = next;
    }
    out
}
