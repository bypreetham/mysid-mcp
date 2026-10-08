use std::collections::HashMap;
use std::path::Path;

use super::model::CodeGraph;
use crate::index::{Index, SymbolId, SymbolKind};

/// Builds the in-memory graph the views read from. Facts come from the persistent index
/// (`<workspace>/.mysid/index.db`), which refreshes only the files that changed.
pub fn build_graph(root: &Path) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let Ok(snap) = Index::open(root).snapshot() else {
        return graph;
    };

    let file_paths: HashMap<i64, &str> = snap.files.iter().map(|f| (f.id, f.path.as_str())).collect();
    for f in &snap.files {
        graph.add_node(format!("file:{}", f.path), "file".into(), f.path.clone(), f.path.clone(), 1);
    }

    // Node ids stay human-readable (`file:Class.method`); views print them.
    let mut node_ids: HashMap<SymbolId, String> = HashMap::new();
    for s in &snap.symbols {
        let Some(&file) = file_paths.get(&s.file_id) else { continue };
        let (id, node_type) = match (s.kind, &s.owner) {
            (SymbolKind::Method, Some(owner)) => (format!("{}:{}.{}", file, owner, s.name), "method"),
            (SymbolKind::Class, _) => (format!("{}:{}", file, s.name), "class"),
            _ => (format!("{}:{}", file, s.name), "function"),
        };
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
