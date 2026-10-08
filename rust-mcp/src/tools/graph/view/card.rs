use crate::tools::graph::model::{CodeGraph, Node};
use crate::tools::graph::query::{collect, label, loc, members, target_set, Dir, Hit};

const LIST_CAP: usize = 12;
const MEMBER_CAP: usize = 8;

/// Compact context for one symbol: callers, the symbol itself, callees (nearest hops first).
pub fn render(graph: &CodeGraph, root: &Node, depth: usize) -> String {
    let set = target_set(graph, root);
    let mut lines = vec![format!("{} [{}]", label(root), root.node_type), loc(root)];

    if root.node_type == "class" {
        let names: Vec<String> = members(graph, root).iter().map(|m| m.name.clone()).collect();
        if !names.is_empty() {
            let shown: Vec<&str> = names.iter().take(MEMBER_CAP).map(|s| s.as_str()).collect();
            let extra = names.len().saturating_sub(MEMBER_CAP);
            let tail = if extra > 0 { format!(" +{}", extra) } else { String::new() };
            lines.push(format!("MEMBERS ({}): {}{}", names.len(), shown.join(", "), tail));
        }
    }

    push_section(&mut lines, "CALLERS", &collect(graph, &set, Dir::Up, depth));
    push_section(&mut lines, "CALLEES", &collect(graph, &set, Dir::Down, depth));
    lines.join("\n")
}

fn push_section(lines: &mut Vec<String>, title: &str, hits: &[Hit]) {
    lines.push(String::new());
    lines.push(format!("{} ({})", title, hits.len()));
    if hits.is_empty() {
        lines.push("  none".to_string());
        return;
    }
    for h in hits.iter().take(LIST_CAP) {
        let hop = if h.hop > 1 { format!("  [hop {}]", h.hop) } else { String::new() };
        lines.push(format!("  {}  {}{}", label(h.node), loc(h.node), hop));
    }
    if hits.len() > LIST_CAP {
        lines.push(format!("  +{} more", hits.len() - LIST_CAP));
    }
}
