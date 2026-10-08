mod build;
mod model;
mod query;
mod view;

use serde_json::{json, Value};
use std::path::Path;

use super::workspace::resolve_existing_root;
use build::build_graph;
use model::Node;
use query::{label, loc, resolve, Resolved};

const MAX_DEPTH: u64 = 5;

/// `mysid graph [symbol] [--depth N] [--flow]`
///
/// - no symbol:  orientation map of the workspace (or of a directory/file path)
/// - symbol:     callers + symbol + callees (1 hop by default, `--depth N` for more)
/// - `--flow`:   same relationships, drawn as UPSTREAM / TARGET / DOWNSTREAM trees
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
    let graph = build_graph(root);
    let root_name = root.file_name().and_then(|n| n.to_str()).unwrap_or("workspace");

    let output = match target {
        None => view::map::render(&graph, root_name, None),
        Some(q) => match resolve(&graph, &q) {
            Resolved::One(node) if flow => view::flow::render(&graph, node, depth),
            Resolved::One(node) => view::card::render(&graph, node, depth),
            Resolved::Many(nodes) => ambiguous(&q, &nodes),
            // Not a symbol: a directory or file path drills into the map instead.
            Resolved::Similar(_) | Resolved::NotFound if view::map::matches_path(&graph, &q) => {
                view::map::render(&graph, root_name, Some(&q))
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
        },
    };

    json!({ "success": true, "output": output }).to_string()
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
