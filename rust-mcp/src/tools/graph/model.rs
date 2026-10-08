use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub node_type: String, // "file", "class", "function", "method"
    pub name: String,
    pub file: String,
    pub line: usize,
}

pub struct CodeGraph {
    pub nodes: HashMap<String, Node>,
    pub forward_edges: HashMap<String, Vec<(String, String)>>, // src -> Vec<(target, relation)>
    pub reverse_edges: HashMap<String, Vec<(String, String)>>, // target -> Vec<(src, relation)>
    pub symbol_to_nodes: HashMap<String, Vec<String>>,         // symbol_name -> Vec<node_id>
}

impl CodeGraph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            forward_edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            symbol_to_nodes: HashMap::new(),
        }
    }

    pub fn add_node(&mut self, id: String, node_type: String, name: String, file: String, line: usize) {
        let node = Node {
            id: id.clone(),
            node_type,
            name: name.clone(),
            file,
            line,
        };
        self.symbol_to_nodes
            .entry(name.clone())
            .or_default()
            .push(id.clone());
        self.symbol_to_nodes
            .entry(name.to_lowercase())
            .or_default()
            .push(id.clone());
        self.nodes.insert(id, node);
    }

    pub fn add_edge(&mut self, src: String, target: String, rel: String) {
        self.forward_edges
            .entry(src.clone())
            .or_default()
            .push((target.clone(), rel.clone()));
        self.reverse_edges
            .entry(target)
            .or_default()
            .push((src, rel));
    }
}
