use std::collections::{BTreeMap, HashMap};

use super::Out;
use crate::tools::graph::model::{CodeGraph, Node};
use crate::tools::graph::query::label;

const MAX_LINES: usize = 40;
const NAMES_PER_FILE: usize = 5;
/// The map opens up folders until about this many rows are shown (never more than MAX_ROWS).
const TARGET_ROWS: usize = 12;
const MAX_ROWS: usize = 22;
/// An opened folder shows this many of its biggest sub-folders, the rest as one row.
const KIDS_SHOWN: usize = 8;
/// Folders with fewer files than this are not opened further.
const MIN_EXPAND_FILES: usize = 12;

fn norm(path: &str) -> String {
    path.trim().replace('\\', "/").trim_start_matches("./").trim_matches('/').to_string()
}

fn file_nodes(graph: &CodeGraph) -> Vec<&Node> {
    let mut files: Vec<&Node> = graph.nodes.values().filter(|n| n.node_type == "file").collect();
    files.sort_by(|a, b| a.file.cmp(&b.file));
    files
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

    // Rows start as the folders directly below the chosen directory; the biggest ones are then opened up,
    // so the first call already shows where the code lives (app/src/main/java/com/x/ui/), not just `app/`.
    let counts = |prefix: &str| -> (BTreeMap<String, (usize, usize)>, Vec<&Node>) {
        let mut dirs: BTreeMap<String, (usize, usize)> = BTreeMap::new();
        let mut direct: Vec<&Node> = Vec::new();
        for f in scoped.iter().filter(|f| f.file.starts_with(prefix)) {
            let rel = &f.file[prefix.len()..];
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
        (dirs, direct)
    };
    // Follow a chain of folders that each hold exactly one folder and no files (src/ -> src/Pages/).
    let collapse = |start: &str| -> String {
        let mut prefix = start.to_string();
        loop {
            let (dirs, direct) = counts(&prefix);
            if dirs.len() == 1 && direct.is_empty() {
                prefix = format!("{}{}/", prefix, dirs.keys().next().unwrap());
            } else {
                return prefix;
            }
        }
    };

    let base = collapse(&dir_prefix);
    let location = if base.is_empty() { root_name.to_string() } else { base.trim_end_matches('/').to_string() };
    out.push(format!("{} - {} files, {} symbols", location, scoped.iter().filter(|f| f.file.starts_with(&base)).count(), {
        scoped.iter().filter(|f| f.file.starts_with(&base)).map(|f| by_file.get(f.file.as_str()).map_or(0, |v| v.len())).sum::<usize>()
    }));

    #[derive(PartialEq)]
    enum Kind {
        Dir,
        /// Files lying directly in a folder that was opened.
        Direct,
        /// The smaller folders of an opened folder, folded into one row (how many).
        Others(usize),
    }
    struct Row {
        prefix: String,
        files: usize,
        syms: usize,
        kind: Kind,
        done: bool,
    }
    let (top_dirs, direct) = counts(&base);
    let mut rows: Vec<Row> = top_dirs
        .iter()
        .map(|(name, (nf, ns))| Row { prefix: collapse(&format!("{}{}/", base, name)), files: *nf, syms: *ns, kind: Kind::Dir, done: false })
        .collect();
    while rows.len() < TARGET_ROWS {
        // The biggest folder that is worth opening and can be.
        let Some(idx) = rows.iter().enumerate().filter(|(_, r)| !r.done && r.files >= MIN_EXPAND_FILES).max_by_key(|(_, r)| r.files).map(|(i, _)| i) else { break };
        let (kids, here) = counts(&rows[idx].prefix);
        if kids.is_empty() {
            // Only files in here: nothing to open, keep the plain row.
            rows[idx].done = true;
            continue;
        }
        let parent = rows[idx].prefix.clone();
        let mut next: Vec<Row> = kids
            .iter()
            .map(|(name, (nf, ns))| Row { prefix: collapse(&format!("{}{}/", parent, name)), files: *nf, syms: *ns, kind: Kind::Dir, done: false })
            .collect();
        // Only the biggest folders get a row of their own; the rest are folded into one.
        if next.len() > KIDS_SHOWN {
            next.sort_by(|a, b| b.files.cmp(&a.files).then(a.prefix.cmp(&b.prefix)));
            let rest = next.split_off(KIDS_SHOWN);
            next.push(Row {
                prefix: parent.clone(),
                files: rest.iter().map(|r| r.files).sum(),
                syms: rest.iter().map(|r| r.syms).sum(),
                kind: Kind::Others(rest.len()),
                done: true,
            });
        }
        // A handful of files without symbols (a build script) is not worth a row.
        let here_syms: usize = here.iter().map(|f| by_file.get(f.file.as_str()).map_or(0, |v| v.len())).sum();
        if !here.is_empty() && (here_syms > 0 || here.len() > 2) {
            next.push(Row { prefix: parent.clone(), files: here.len(), syms: here_syms, kind: Kind::Direct, done: true });
        }
        // A single child is simply followed (it goes one level deeper); only an empty or too-wide result stops here.
        if next.is_empty() || rows.len() - 1 + next.len() > MAX_ROWS {
            rows[idx].done = true;
            continue;
        }
        rows.splice(idx..=idx, next);
    }
    rows.sort_by(|a, b| a.prefix.cmp(&b.prefix).then((a.kind == Kind::Dir).cmp(&(b.kind == Kind::Dir))));
    for r in &rows {
        let line = match r.kind {
            Kind::Direct => format!("  {}  (+{} files directly here, {} symbols)", r.prefix, r.files, r.syms),
            Kind::Others(n) => format!("  {}  (+{} more folders: {} files, {} symbols)", r.prefix, n, r.files, r.syms),
            Kind::Dir => format!("  {}  {} files, {} symbols", r.prefix, r.files, r.syms),
        };
        if !out.push(line) {
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
