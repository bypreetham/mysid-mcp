use std::collections::HashMap;

use super::model::CodeGraph;
use crate::index::{Edge, Index, Located, SymbolId, SymbolKind};

/// A graph holding only part of the workspace, plus the symbol id behind each node id.
pub struct Partial {
    pub graph: CodeGraph,
    pub ids: HashMap<String, SymbolId>,
}

/// Human-readable node id (`file:Class.method`) and node type for a symbol.
fn node_of(l: &Located) -> (String, &'static str) {
    let s = &l.symbol;
    match (s.kind, &s.owner) {
        (SymbolKind::Method, Some(owner)) => (format!("{}:{}.{}", l.path, owner, s.name), "method"),
        (SymbolKind::Class, _) => (format!("{}:{}", l.path, s.name), "class"),
        _ => (format!("{}:{}", l.path, s.name), "function"),
    }
}

/// Builds a graph from just these symbols and the CALLS edges among them (what a symbol
/// lookup needs). Edges whose endpoints are not in the set are dropped.
pub fn partial_graph(symbols: &[Located], edges: &[Edge]) -> Partial {
    let mut graph = CodeGraph::new();
    let mut ids: HashMap<String, SymbolId> = HashMap::new();
    let mut node_ids: HashMap<SymbolId, String> = HashMap::new();

    for l in symbols {
        let (id, node_type) = node_of(l);
        graph.add_node(id.clone(), node_type.into(), l.symbol.name.clone(), l.path.clone(), l.symbol.start_line as usize);
        ids.insert(id.clone(), l.symbol.id);
        node_ids.insert(l.symbol.id, id);
    }
    for e in edges {
        if let (Some(from), Some(to)) = (node_ids.get(&e.from), node_ids.get(&e.to)) {
            graph.add_edge(from.clone(), to.clone(), e.kind.as_str().into());
        }
    }
    Partial { graph, ids }
}

/// The whole workspace as a graph, for the views that need every file (the map). Costs a full read
/// of the index, so symbol lookups don't use it.
pub fn full_graph(index: &Index) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let Ok(snap) = index.snapshot() else {
        return graph;
    };

    let file_paths: HashMap<i64, &str> = snap.files.iter().map(|f| (f.id, f.path.as_str())).collect();
    for f in &snap.files {
        graph.add_node(format!("file:{}", f.path), "file".into(), f.path.clone(), f.path.clone(), 1);
    }

    let mut node_ids: HashMap<SymbolId, String> = HashMap::new();
    for s in &snap.symbols {
        let Some(&file) = file_paths.get(&s.file_id) else { continue };
        let located = Located { symbol: s.clone(), path: file.to_string() };
        let (id, node_type) = node_of(&located);
        graph.add_node(id.clone(), node_type.into(), s.name.clone(), file.to_string(), s.start_line as usize);
        node_ids.insert(s.id, id);
    }

    // DEFINES: a method hangs off its class when the class is in the same file, else off the file.
    for s in &snap.symbols {
        let (Some(&file), Some(id)) = (file_paths.get(&s.file_id), node_ids.get(&s.id)) else { continue };
        let file_node = format!("file:{}", file);
        let parent = match &s.owner {
            Some(owner) => {
                let class_id = format!("{}:{}", file, owner);
                if graph.nodes.contains_key(&class_id) { class_id } else { file_node }
            }
            None => file_node,
        };
        graph.add_edge(parent, id.clone(), "DEFINES".into());
    }

    for e in &snap.edges {
        if let (Some(from), Some(to)) = (node_ids.get(&e.from), node_ids.get(&e.to)) {
            graph.add_edge(from.clone(), to.clone(), e.kind.as_str().into());
        }
    }
    graph
}
