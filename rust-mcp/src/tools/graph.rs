use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use super::workspace::resolve_existing_root;


#[derive(Clone, Debug)]
struct Node {
    id: String,
    node_type: String, // "file", "class", "function", "method"
    name: String,
    file: String,
    line: usize,
}

struct CodeGraph {
    nodes: HashMap<String, Node>,
    forward_edges: HashMap<String, Vec<(String, String)>>, // src -> Vec<(target, relation)>
    reverse_edges: HashMap<String, Vec<(String, String)>>, // target -> Vec<(src, relation)>
    symbol_to_nodes: HashMap<String, Vec<String>>,         // symbol_name -> Vec<node_id>
}

impl CodeGraph {
    fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            forward_edges: HashMap::new(),
            reverse_edges: HashMap::new(),
            symbol_to_nodes: HashMap::new(),
        }
    }

    fn add_node(&mut self, id: String, node_type: String, name: String, file: String, line: usize) {
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

    fn add_edge(&mut self, src: String, target: String, rel: String) {
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

pub fn execute_graph(params: &Value, default_root: Option<&str>) -> String {
    let mode = params
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("overview")
        .to_string();

    if mode == "help" || mode == "--help" || mode == "-h" || params.get("help").is_some() || params.get("h").is_some() {
        return "mysid graph: Architectural code graph & call-flow intelligence\n\n\
Usage:\n  \
  mysid graph [--mode overview] [--filter <pkg>]  Generate package/module dependency overview\n  \
  mysid graph --mode flow --query <symbol>        Trace downstream call graph\n  \
  mysid graph --mode impact --query <symbol>      Trace upstream callers / blast radius\n  \
  mysid graph --mode symbol --query <symbol>      Inspect node metadata and connections\n".to_string();
    }

    let workspace_root = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let query = params
        .get("query")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let depth = params
        .get("depth")
        .and_then(|v| v.as_u64())
        .unwrap_or(3) as usize;

    let root = Path::new(&workspace_root);
    let graph = build_graph(root);

    let filter = ["dir", "package", "path", "filter"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .or_else(|| {
            if mode == "overview" && !query.is_empty() {
                Some(query.clone())
            } else {
                None
            }
        });

    match mode.as_str() {
        "overview" => {
            let out = generate_overview(&graph, root, filter.as_deref());
            json!({
                "success": true,
                "mode": "overview",
                "output": out
            })
            .to_string()
        }
        "symbol" => {
            if query.is_empty() {
                return json!({
                    "success": false,
                    "error": "query (symbol name) is required for 'symbol' mode"
                })
                .to_string();
            }
            let out = query_symbol(&graph, &query);
            json!({
                "success": true,
                "mode": "symbol",
                "symbols": out
            })
            .to_string()
        }
        "flow" => {
            if query.is_empty() {
                return json!({
                    "success": false,
                    "error": "query (function name) is required for 'flow' mode"
                })
                .to_string();
            }
            let out = trace_call_flow(&graph, &query, depth);
            json!({
                "success": true,
                "mode": "flow",
                "output": out
            })
            .to_string()
        }
        "impact" => {
            if query.is_empty() {
                return json!({
                    "success": false,
                    "error": "query (symbol name) is required for 'impact' mode"
                })
                .to_string();
            }
            let out = trace_impact_analysis(&graph, &query, depth);
            json!({
                "success": true,
                "mode": "impact",
                "output": out
            })
            .to_string()
        }
        _ => json!({
            "success": false,
            "error": format!("Unknown mode '{}'. Choose from 'overview', 'symbol', 'flow', 'impact'", mode)
        })
        .to_string(),
    }
}

fn build_graph(root: &Path) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let mut files_to_scan = Vec::new();
    walk_dir_collect(root, root, &mut files_to_scan);

    let func_call_regex = Regex::new(r"\b([a-zA-Z_][a-zA-Z0-9_]*)\s*\(").unwrap();
    let mut all_calls: Vec<(String, String)> = Vec::new(); // (caller_node_id, callee_name)

    for (rel_path, full_path) in files_to_scan {
        let file_node_id = format!("file:{}", rel_path);
        graph.add_node(
            file_node_id.clone(),
            "file".to_string(),
            rel_path.clone(),
            rel_path.clone(),
            1,
        );

        let content = match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let ext = full_path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();

        let (func_re, class_re) = match ext.as_str() {
            "py" => (
                Regex::new(r"^\s*(?:async\s+)?def\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
                Regex::new(r"^\s*class\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
            "kt" | "java" => (
                Regex::new(r"^\s*(?:(?:public|private|protected|internal|override|final|abstract|suspend|fun|inline)\s+)*fun\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
                Regex::new(r"^\s*(?:(?:public|private|protected|internal|abstract|sealed|data|enum|open)\s+)*(?:class|interface|object|enum\s+class)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
            "rs" => (
                Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
                Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:struct|enum|trait|impl(?:\s+<[^>]+>)?)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
            "js" | "ts" | "jsx" | "tsx" => (
                Regex::new(r"^\s*(?:export\s+)?(?:async\s+)?(?:function\*?\s+([a-zA-Z_][a-zA-Z0-9_]*)|const\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*=\s*(?:async\s+)?\([^)]*\)\s*=>|(?:get|set)?\s*([a-zA-Z_][a-zA-Z0-9_]*)\s*\([^)]*\)\s*\{)").unwrap(),
                Regex::new(r"^\s*(?:export\s+)?(?:abstract\s+)?(?:class|interface|type|enum)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
            "cpp" | "cc" | "cxx" | "c" | "h" | "hpp" => (
                // Matches C/C++ methods/functions:
                // 1) Class::Method(...)
                // 2) [virtual|static|inline|explicit] [Type] [Method](...)
                // 3) Constructor / Destructor ~Class(...) or Class(...)
                Regex::new(r"^\s*(?:(?:virtual|static|inline|explicit|const|constexpr|friend)\s+)*(?:(?:[a-zA-Z_][a-zA-Z0-9_:<>\*&]*)\s+)?(?:([a-zA-Z_][a-zA-Z0-9_]*)::)?(~?[a-zA-Z_][a-zA-Z0-9_]*)\s*\([^;\{]*\)\s*(?:const\s*)?(?:override\s*)?(?:noexcept\s*)?(?:=\s*0\s*)?(?:\{|;)").unwrap(),
                // Matches class/struct/namespace definitions:
                Regex::new(r"^\s*(?:class|struct|interface)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
            _ => (
                Regex::new(r"^\s*(?:def|fun|fn|function)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
                Regex::new(r"^\s*(?:class|struct|interface)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            ),
        };

        let mut current_class: Option<String> = None;
        let mut brace_depth: i32 = 0;
        let is_cpp = matches!(ext.as_str(), "cpp" | "cc" | "cxx" | "c" | "h" | "hpp");

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            let trimmed = line.trim();

            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
                continue;
            }

            if let Some(caps) = class_re.captures(line) {
                let name = caps
                    .iter()
                    .skip(1)
                    .find_map(|m| m)
                    .map(|m| m.as_str())
                    .unwrap_or("unknown");

                // Skip forward declarations like "class AudioEngine;"
                if !trimmed.ends_with(';') {
                    let class_id = format!("{}:{}", rel_path, name);
                    graph.add_node(
                        class_id.clone(),
                        "class".to_string(),
                        name.to_string(),
                        rel_path.clone(),
                        line_num,
                    );
                    graph.add_edge(file_node_id.clone(), class_id.clone(), "DEFINES".to_string());
                    current_class = Some(class_id);
                    brace_depth = 0;
                    continue;
                }
            }

            if is_cpp {
                let open_b = line.matches('{').count() as i32;
                let close_b = line.matches('}').count() as i32;
                brace_depth += open_b - close_b;
                if brace_depth <= 0 && open_b != close_b && current_class.is_some() {
                    current_class = None;
                }
            }

            if let Some(caps) = func_re.captures(line) {
                if is_cpp {
                    let qualified_class = caps.get(1).map(|m| m.as_str());
                    let method_name = caps.get(2).map(|m| m.as_str()).unwrap_or("");

                    if method_name.is_empty() || method_name == "if" || method_name == "while" || method_name == "for" || method_name == "switch" {
                        continue;
                    }

                    let (func_id, ntype, parent) = if let Some(qc) = qualified_class {
                        let cid = format!("{}:{}", rel_path, qc);
                        (format!("{}.{}", cid, method_name), "method".to_string(), cid)
                    } else if let Some(ref cid) = current_class {
                        (format!("{}.{}", cid, method_name), "method".to_string(), cid.clone())
                    } else {
                        (format!("{}:{}", rel_path, method_name), "function".to_string(), file_node_id.clone())
                    };

                    graph.add_node(
                        func_id.clone(),
                        ntype,
                        method_name.to_string(),
                        rel_path.clone(),
                        line_num,
                    );
                    graph.add_edge(parent, func_id.clone(), "DEFINES".to_string());

                    for c_cap in func_call_regex.captures_iter(line) {
                        let callee = c_cap.get(1).unwrap().as_str();
                        if callee != method_name && callee != "LOGD" && callee != "LOGE" && callee != "LOGI" && callee != "LOGW" {
                            all_calls.push((func_id.clone(), callee.to_string()));
                        }
                    }
                } else {
                    let name = caps
                        .iter()
                        .skip(1)
                        .find_map(|m| m)
                        .map(|m| m.as_str())
                        .unwrap_or("unknown");

                    let (func_id, ntype) = if let Some(ref cid) = current_class {
                        (format!("{}.{}", cid, name), "method".to_string())
                    } else {
                        (format!("{}:{}", rel_path, name), "function".to_string())
                    };

                    graph.add_node(
                        func_id.clone(),
                        ntype,
                        name.to_string(),
                        rel_path.clone(),
                        line_num,
                    );
                    let parent = current_class.clone().unwrap_or_else(|| file_node_id.clone());
                    graph.add_edge(parent, func_id.clone(), "DEFINES".to_string());

                    for c_cap in func_call_regex.captures_iter(line) {
                        let callee = c_cap.get(1).unwrap().as_str();
                        if callee != name && callee != "def" && callee != "fn" && callee != "class" {
                            all_calls.push((func_id.clone(), callee.to_string()));
                        }
                    }
                }
            }
        }
    }

    // Resolve call edges
    for (caller_id, callee_name) in all_calls {
        if let Some(target_ids) = graph.symbol_to_nodes.get(&callee_name) {
            let caller_file = graph.nodes.get(&caller_id).map(|n| n.file.as_str()).unwrap_or("");
            let best_target = target_ids
                .iter()
                .find(|tid| graph.nodes.get(*tid).map(|n| n.file.as_str() == caller_file).unwrap_or(false))
                .unwrap_or(&target_ids[0]);

            graph.add_edge(caller_id, best_target.clone(), "CALLS".to_string());
        }
    }

    graph
}

fn walk_dir_collect(root: &Path, current: &Path, out: &mut Vec<(String, PathBuf)>) {
    let entries = match fs::read_dir(current) {
        Ok(e) => e,
        Err(_) => return,
    };

    let ignore_names: HashSet<&str> = [
        ".git", "node_modules", "venv", ".venv", "__pycache__", "target", "build", "dist", ".gradle", ".idea", ".vscode"
    ]
    .iter()
    .cloned()
    .collect();

    let valid_exts: HashSet<&str> = [
        "py", "rs", "kt", "java", "js", "ts", "jsx", "tsx", "go", "cpp", "c", "h", "hpp"
    ]
    .iter()
    .cloned()
    .collect();

    for entry in entries.filter_map(|e| e.ok()) {
        let p = entry.path();
        let fname = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };

        if ignore_names.contains(fname) {
            continue;
        }

        if p.is_dir() {
            walk_dir_collect(root, &p, out);
        } else if p.is_file() {
            if let Some(ext) = p.extension().and_then(|s| s.to_str()) {
                if valid_exts.contains(ext.to_lowercase().as_str()) {
                    if let Ok(rel) = p.strip_prefix(root) {
                        out.push((rel.to_string_lossy().replace('\\', "/"), p.clone()));
                    }
                }
            }
        }
    }
}

fn generate_overview(graph: &CodeGraph, root: &Path, filter_path: Option<&str>) -> String {
    let file_count = graph.nodes.values().filter(|n| n.node_type == "file").count();
    let class_count = graph.nodes.values().filter(|n| n.node_type == "class").count();
    let func_count = graph
        .nodes
        .values()
        .filter(|n| n.node_type == "function" || n.node_type == "method")
        .count();

    let root_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("Project");
    let mut lines = vec![
        format!("Codebase Topology Graph for: {}", root_name),
        format!("  Total in Workspace -> Files: {} | Classes: {} | Functions/Methods: {}", file_count, class_count, func_count),
    ];

    if let Some(filt) = filter_path {
        lines.push(format!("  Filter active: '{}'", filt));
    }
    lines.push("=".repeat(65));

    let mut files_map: HashMap<&str, Vec<&Node>> = HashMap::new();
    let norm_filter = filter_path.map(|f| f.replace('\\', "/").to_lowercase());

    for node in graph.nodes.values() {
        if node.node_type != "file" {
            if let Some(ref filt) = norm_filter {
                let norm_file = node.file.replace('\\', "/").to_lowercase();
                if !norm_file.contains(filt) {
                    continue;
                }
            }
            files_map.entry(&node.file).or_default().push(node);
        }
    }

    if files_map.is_empty() {
        if let Some(filt) = filter_path {
            lines.push(format!("\nNo files or symbols matched filter '{}'.", filt));
        } else {
            lines.push("\nNo code symbols found in workspace.".to_string());
        }
        return lines.join("\n");
    }

    let mut sorted_files: Vec<&&str> = files_map.keys().collect();
    sorted_files.sort();

    let filtered_symbol_count: usize = files_map.values().map(|v| v.len()).sum();
    lines.push(format!("  Showing: {} files ({} symbols)", sorted_files.len(), filtered_symbol_count));

    for fpath in sorted_files {
        let symbols = &files_map[*fpath];
        lines.push(format!("\n[FILE] {} ({} symbols)", fpath, symbols.len()));
        for s in symbols.iter().take(6) {
            let tag = if s.node_type == "class" { "[CLASS]" } else { "[FUNC]" };
            lines.push(format!("   {} {} (line {})", tag, s.name, s.line));
        }
        if symbols.len() > 6 {
            lines.push(format!("   ... and {} more", symbols.len() - 6));
        }
    }

    lines.join("\n")
}

fn query_symbol(graph: &CodeGraph, query: &str) -> Vec<Value> {
    let target_ids = match graph.symbol_to_nodes.get(query).or_else(|| graph.symbol_to_nodes.get(&query.to_lowercase())) {
        Some(ids) => ids.clone(),
        None => {
            graph
                .nodes
                .iter()
                .filter(|(_, n)| n.node_type != "file" && n.name.to_lowercase().contains(&query.to_lowercase()))
                .map(|(id, _)| id.clone())
                .take(10)
                .collect()
        }
    };

    let mut results = Vec::new();
    for nid in target_ids.iter().take(10) {
        if let Some(node) = graph.nodes.get(nid) {
            let forward: Vec<Value> = graph
                .forward_edges
                .get(nid)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|(tid, rel)| {
                    let target = graph.nodes.get(tid);
                    json!({
                        "relation": rel,
                        "target": target.map(|n| n.name.as_str()).unwrap_or(tid.as_str()),
                        "file": target.map(|n| n.file.as_str()).unwrap_or("?"),
                    })
                })
                .collect();

            let reverse: Vec<Value> = graph
                .reverse_edges
                .get(nid)
                .unwrap_or(&Vec::new())
                .iter()
                .map(|(sid, rel)| {
                    let src = graph.nodes.get(sid);
                    json!({
                        "relation": rel,
                        "source": src.map(|n| n.name.as_str()).unwrap_or(sid.as_str()),
                        "file": src.map(|n| n.file.as_str()).unwrap_or("?"),
                    })
                })
                .collect();

            results.push(json!({
                "id": node.id,
                "name": node.name,
                "type": node.node_type,
                "file": node.file,
                "line": node.line,
                "outgoing_edges": forward,
                "incoming_edges": reverse,
            }));
        }
    }

    results
}

fn trace_call_flow(graph: &CodeGraph, symbol: &str, depth: usize) -> String {
    // Check if query is Class.method or Class::method
    let normalized_symbol = symbol.replace("::", ".");
    let target_ids = if let Some(ids) = graph.symbol_to_nodes.get(symbol).or_else(|| graph.symbol_to_nodes.get(&symbol.to_lowercase())) {
        ids.clone()
    } else if let Some((cls, mth)) = normalized_symbol.split_once('.') {
        // Look for node where id contains :cls.mth or name is mth and id contains cls
        graph
            .nodes
            .iter()
            .filter(|(id, n)| {
                (n.node_type == "method" || n.node_type == "function")
                    && n.name.eq_ignore_ascii_case(mth)
                    && id.to_lowercase().contains(&cls.to_lowercase())
            })
            .map(|(id, _)| id.clone())
            .collect()
    } else {
        Vec::new()
    };

    if target_ids.is_empty() {
        return format!("Symbol '{}' not found in code graph.", symbol);
    }

    let root_node = match target_ids.iter().find_map(|id| graph.nodes.get(id).map(|n| (id, n))) {
        Some((id, n)) => (id, n),
        None => return format!("Symbol '{}' not found in code graph.", symbol),
    };
    let (root_id, root_node) = root_node;

    let mut lines = vec![format!("Call Flow Trace for '{}' (depth: {}):", root_node.name, depth)];
    let mut visited = HashSet::new();

    fn trace_rec(
        graph: &CodeGraph,
        curr_id: &str,
        curr_depth: usize,
        max_depth: usize,
        indent: usize,
        visited: &mut HashSet<String>,
        lines: &mut Vec<String>,
    ) {
        if visited.contains(curr_id) || curr_depth > max_depth {
            return;
        }
        visited.insert(curr_id.to_string());

        let node = match graph.nodes.get(curr_id) {
            Some(n) => n,
            None => return,
        };

        let prefix = "  ".repeat(indent);
        let arrow = if indent > 0 { "-> " } else { "" };
        lines.push(format!("{}{}{}{} ({}:{})", prefix, arrow, node.name, "", node.file, node.line));

        if let Some(edges) = graph.forward_edges.get(curr_id) {
            for (target_id, rel) in edges {
                if rel == "CALLS" {
                    trace_rec(graph, target_id, curr_depth + 1, max_depth, indent + 1, visited, lines);
                }
            }
        }
    }

    if root_node.node_type == "class" {
        // If it's a class, find all methods defined by this class
        let class_prefix = format!("{}.", root_id);
        let class_dot = format!(":{}.", root_node.name);
        let mut class_methods: Vec<(&String, &Node)> = graph
            .nodes
            .iter()
            .filter(|(id, n)| {
                n.node_type == "method" && (id.starts_with(&class_prefix) || id.contains(&class_dot))
            })
            .collect();
        class_methods.sort_by_key(|(_, n)| (&n.file, n.line));

        if class_methods.is_empty() {
            lines.push(format!("  [CLASS] {} ({}:{}) - no methods detected", root_node.name, root_node.file, root_node.line));
        } else {
            lines.push(format!("  [CLASS] {} ({}:{}) - tracing {} method(s):", root_node.name, root_node.file, root_node.line, class_methods.len()));
            for (m_id, m_node) in class_methods {
                lines.push(format!("\n  Method '{}' (line {}):", m_node.name, m_node.line));
                let mut method_visited = HashSet::new();
                trace_rec(graph, m_id, 0, depth, 2, &mut method_visited, &mut lines);
            }
        }
    } else {
        trace_rec(graph, root_id, 0, depth, 0, &mut visited, &mut lines);
    }

    lines.join("\n")
}

fn trace_impact_analysis(graph: &CodeGraph, symbol: &str, depth: usize) -> String {
    let target_ids = match graph.symbol_to_nodes.get(symbol).or_else(|| graph.symbol_to_nodes.get(&symbol.to_lowercase())) {
        Some(ids) => ids,
        None => return format!("Symbol '{}' not found in code graph.", symbol),
    };

    let root_node = match target_ids.iter().find_map(|id| graph.nodes.get(id).map(|n| (id, n))) {
        Some((id, n)) => (id, n),
        None => return format!("Symbol '{}' not found in code graph.", symbol),
    };
    let (root_id, root_node) = root_node;

    let mut lines = vec![format!("Impact Analysis (Upstream Callers) for '{}':", root_node.name)];
    let mut visited = HashSet::new();

    fn trace_up_rec(
        graph: &CodeGraph,
        curr_id: &str,
        curr_depth: usize,
        max_depth: usize,
        indent: usize,
        visited: &mut HashSet<String>,
        lines: &mut Vec<String>,
    ) {
        if visited.contains(curr_id) || curr_depth > max_depth {
            return;
        }
        visited.insert(curr_id.to_string());

        let node = match graph.nodes.get(curr_id) {
            Some(n) => n,
            None => return,
        };

        let prefix = "  ".repeat(indent);
        let arrow = if indent > 0 { "^ called by: " } else { "" };
        lines.push(format!("{}{}{}{} ({}:{})", prefix, arrow, node.name, "", node.file, node.line));

        if let Some(edges) = graph.reverse_edges.get(curr_id) {
            for (src_id, rel) in edges {
                if rel == "CALLS" {
                    trace_up_rec(graph, src_id, curr_depth + 1, max_depth, indent + 1, visited, lines);
                }
            }
        }
    }

    trace_up_rec(graph, root_id, 0, depth, 0, &mut visited, &mut lines);
    lines.join("\n")
}
