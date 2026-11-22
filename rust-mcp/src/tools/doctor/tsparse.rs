//! Proof of concept (cargo feature `treesitter`): the Kotlin tree-sitter parser next to the regex one.
//!
//! `mysid doctor tsdump <file.kt>`     the syntax tree of a file (node kinds, field names, lines): for writing the queries.
//! `mysid doctor tscompare <folder>`   runs both parsers over every .kt file and lists where they disagree.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

use crate::parsers::kotlin::KotlinParser;
use crate::parsers::kotlin_ts::{kotlin_tree, KotlinTsParser};
use crate::parsers::{LanguageParser, MethodSignature, TypeInfo};

fn resolve(p: &str, base: &Path) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn base_of(params: &Value, default_root: Option<&str>) -> PathBuf {
    params.get("workspace_root").and_then(|v| v.as_str()).or(default_root).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

fn fail(m: String) -> String {
    json!({ "success": false, "error": m }).to_string()
}

fn ok(t: String) -> String {
    json!({ "success": true, "output": t }).to_string()
}

fn dump_node(node: tree_sitter::Node, src: &[u8], field: Option<&str>, depth: usize, max_depth: usize, out: &mut Vec<String>) {
    if !node.is_named() {
        return;
    }
    let text = node.utf8_text(src).unwrap_or("").replace('\n', " ");
    let text: String = text.chars().take(36).collect();
    out.push(format!(
        "{}{}{} [{}:{}-{}:{}] {}",
        "  ".repeat(depth),
        field.map(|f| format!("{}: ", f)).unwrap_or_default(),
        node.kind(),
        node.start_position().row + 1,
        node.start_position().column,
        node.end_position().row + 1,
        node.end_position().column,
        if node.child_count() == 0 || depth >= max_depth { format!("`{}`", text) } else { String::new() }
    ));
    if depth >= max_depth {
        return;
    }
    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            dump_node(cursor.node(), src, cursor.field_name(), depth + 1, max_depth, out);
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

pub fn dump(params: &Value, default_root: Option<&str>) -> String {
    let Some(file) = params.get("a").or_else(|| params.get("path")).and_then(|v| v.as_str()) else {
        return fail("tsdump needs a Kotlin file: mysid doctor tsdump Foo.kt".to_string());
    };
    let path = resolve(file, &base_of(params, default_root));
    let Ok(text) = fs::read_to_string(&path) else { return fail(format!("cannot read {}", path.display())) };
    let max_depth = params.get("depth").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(6) as usize;
    let src = text.lines().collect::<Vec<_>>().join("\n");
    let Some(tree) = kotlin_tree(&src) else { return fail("the Kotlin grammar did not load".to_string()) };
    let mut out = Vec::new();
    dump_node(tree.root_node(), src.as_bytes(), None, 0, max_depth, &mut out);
    // Diagnostics: what the grammar says about errors in this tree (named and unnamed nodes).
    fn count(node: tree_sitter::Node, errors: &mut Vec<String>) {
        if node.is_error() || node.is_missing() {
            errors.push(format!("{}{} at line {}", if node.is_missing() { "MISSING " } else { "ERROR " }, node.kind(), node.start_position().row + 1));
        }
        let mut c = node.walk();
        for ch in node.children(&mut c) {
            count(ch, errors);
        }
    }
    let mut errs = Vec::new();
    count(tree.root_node(), &mut errs);
    out.push(format!("-- root.has_error() = {}, error/missing nodes found by a full walk: {} {:?}", tree.root_node().has_error(), errs.len(), errs.iter().take(5).collect::<Vec<_>>()));
    ok(out.join("\n"))
}

fn kt_files(root: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !matches!(name.as_str(), "build" | ".git" | "node_modules" | ".gradle" | ".cxx" | ".idea" | "external" | "third_party") {
                    stack.push(p);
                }
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("kt") | Some("kts")) {
                v.push(p);
            }
        }
    }
    v.sort();
    v
}

fn type_key(t: &TypeInfo) -> String {
    format!("{} {} @{}", t.kind, t.name, t.line)
}

fn method_key(m: &MethodSignature) -> String {
    format!("{} {}.{} @{}", m.kind, m.parent.as_deref().unwrap_or("-"), m.name, m.start_line)
}

pub fn compare(params: &Value, default_root: Option<&str>) -> String {
    let Some(dir) = params.get("a").or_else(|| params.get("path")).and_then(|v| v.as_str()) else {
        return fail("tscompare needs a folder: mysid doctor tscompare <folder>".to_string());
    };
    let root = resolve(dir, &base_of(params, default_root));
    if !root.is_dir() {
        return fail(format!("not a folder: {}", root.display()));
    }
    let max = params.get("max").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(25) as usize;
    let files = kt_files(&root);
    let (rx, ts) = (KotlinParser, KotlinTsParser);
    let (mut n_types_rx, mut n_types_ts, mut n_methods_rx, mut n_methods_ts) = (0usize, 0usize, 0usize, 0usize);
    let (mut same_files, mut diff_files) = (0usize, 0usize);
    let mut only_rx: Vec<String> = Vec::new();
    let mut only_ts: Vec<String> = Vec::new();
    let mut sig_diffs: Vec<String> = Vec::new();
    let mut error_files: Vec<String> = Vec::new();
    let t0 = std::time::Instant::now();
    let (mut t_rx, mut t_ts) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    for f in &files {
        let Ok(text) = fs::read_to_string(f) else { continue };
        let lines: Vec<&str> = text.lines().collect();
        let a = std::time::Instant::now();
        let (rt, rm) = rx.parse(&lines);
        t_rx += a.elapsed();
        let b = std::time::Instant::now();
        let (tt, tm, had_error) = ts.parse_checked(&lines);
        t_ts += b.elapsed();
        if had_error {
            let (line, what) = crate::parsers::kotlin_ts::first_error(&lines.join("\n")).unwrap_or((0, "?".to_string()));
            error_files.push(format!("{}:{}  {}", f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/"), line, what));
        }
        n_types_rx += rt.len();
        n_types_ts += tt.len();
        n_methods_rx += rm.len();
        n_methods_ts += tm.len();
        let name = f.strip_prefix(&root).unwrap_or(f).to_string_lossy().replace('\\', "/");
        let mut differs = false;
        let rx_keys: Vec<String> = rt.iter().map(type_key).chain(rm.iter().map(method_key)).collect();
        let ts_keys: Vec<String> = tt.iter().map(type_key).chain(tm.iter().map(method_key)).collect();
        for k in &rx_keys {
            if !ts_keys.contains(k) {
                only_rx.push(format!("{}: {}", name, k));
                differs = true;
            }
        }
        for k in &ts_keys {
            if !rx_keys.contains(k) {
                only_ts.push(format!("{}: {}", name, k));
                differs = true;
            }
        }
        // Same method, different parameters or return type.
        for m in &rm {
            if let Some(o) = tm.iter().find(|o| o.name == m.name && o.start_line == m.start_line) {
                let norm = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
                if norm(&m.params) != norm(&o.params) || m.return_type.as_deref().map(norm) != o.return_type.as_deref().map(norm) {
                    sig_diffs.push(format!("{}:{} {}\n      regex: ({}) -> {:?}\n      tree : ({}) -> {:?}", name, m.start_line, m.name, norm(&m.params), m.return_type, norm(&o.params), o.return_type));
                }
            }
        }
        if differs {
            diff_files += 1;
        } else {
            same_files += 1;
        }
    }
    let mut out = vec![
        format!("compare Kotlin parsers over {} ({} files, {:.1}s)", root.display().to_string().replace('\\', "/"), files.len(), t0.elapsed().as_secs_f64()),
        format!("                 regex      tree-sitter"),
        format!("types        {:>9}      {:>9}", n_types_rx, n_types_ts),
        format!("methods      {:>9}      {:>9}", n_methods_rx, n_methods_ts),
        format!("parse time   {:>7.0}ms      {:>7.0}ms", t_rx.as_secs_f64() * 1000.0, t_ts.as_secs_f64() * 1000.0),
        format!("files with identical symbols: {} of {}; with differences: {}", same_files, files.len(), diff_files),
        format!("files the grammar reported a syntax error in (read by the regex parser instead): {} of {}", error_files.len(), files.len()),
        format!("only the regex parser found: {}   only tree-sitter found: {}   same symbol, different signature: {}", only_rx.len(), only_ts.len(), sig_diffs.len()),
    ];
    let list = |out: &mut Vec<String>, title: &str, items: &[String]| {
        if items.is_empty() {
            return;
        }
        out.push(String::new());
        out.push(format!("{} ({}):", title, items.len()));
        out.extend(items.iter().take(max).map(|s| format!("  {}", s)));
        if items.len() > max {
            out.push(format!("  ... +{} more (--max N)", items.len() - max));
        }
    };
    list(&mut out, "syntax errors in the tree (file:line of the first)", &error_files);
    list(&mut out, "only in the regex parser", &only_rx);
    list(&mut out, "only in tree-sitter", &only_ts);
    list(&mut out, "different signatures", &sig_diffs);
    ok(out.join("\n"))
}
