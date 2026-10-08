use std::collections::HashSet;

use super::Out;
use crate::tools::graph::model::{CodeGraph, Node};
use crate::tools::graph::query::{label, loc, neighbors, set_neighbors, target_set, Dir};

const MAX_LINES: usize = 60;
const CHILD_CAP: usize = 8;

// ASCII on purpose: box-drawing glyphs garble in some Windows consoles and cost more tokens.
const BRANCH: &str = "+- ";
const LAST: &str = "\\- ";
const PIPE: &str = "|  ";
const GAP: &str = "   ";

/// Same relationships as the card, drawn as trees: UPSTREAM callers, TARGET, DOWNSTREAM callees.
pub fn render(graph: &CodeGraph, root: &Node, depth: usize) -> String {
    let set = target_set(graph, root);
    let mut out = Out::new(MAX_LINES);
    out.push(format!("{} [{}]  {}", label(root), root.node_type, loc(root)));

    out.push("");
    out.push("UPSTREAM");
    section(graph, set_neighbors(graph, &set, Dir::Up), Dir::Up, depth, &set, &mut out);

    out.push("");
    out.push("TARGET");
    out.push(format!("  {}", label(root)));

    out.push("");
    out.push("DOWNSTREAM");
    section(graph, set_neighbors(graph, &set, Dir::Down), Dir::Down, depth, &set, &mut out);

    out.finish()
}

fn section(graph: &CodeGraph, first: Vec<&Node>, dir: Dir, depth: usize, set: &HashSet<String>, out: &mut Out) {
    if first.is_empty() {
        out.push("  none");
        return;
    }
    let mut visited = set.clone();
    tree(graph, first, dir, depth, "  ", &mut visited, out);
}

fn tree(
    graph: &CodeGraph,
    nodes: Vec<&Node>,
    dir: Dir,
    depth_left: usize,
    prefix: &str,
    visited: &mut HashSet<String>,
    out: &mut Out,
) {
    let total = nodes.len();
    let shown = total.min(CHILD_CAP);

    for (i, n) in nodes.iter().take(shown).enumerate() {
        if out.full() {
            return;
        }
        let last = i + 1 == shown && total == shown;
        let already = !visited.insert(n.id.clone());
        let seen = if already { "  (seen)" } else { "" };
        out.push(format!("{}{}{}  {}{}", prefix, if last { LAST } else { BRANCH }, label(n), loc(n), seen));

        if !already && depth_left > 1 {
            let child_prefix = format!("{}{}", prefix, if last { GAP } else { PIPE });
            tree(graph, neighbors(graph, &n.id, dir), dir, depth_left - 1, &child_prefix, visited, out);
        }
    }
    if total > shown {
        out.push(format!("{}{}+{} more", prefix, LAST, total - shown));
    }
}
