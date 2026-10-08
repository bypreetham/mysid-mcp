use regex::Regex;
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use super::model::SymbolKind;
use crate::parsers::find_parser;

/// Identifiers followed by `(` that are language constructs or logging macros, never call targets.
const NOT_CALLS: &[&str] = &[
    "if", "for", "while", "switch", "match", "return", "catch", "fn", "def", "function", "class",
    "sizeof", "typeof", "new", "await", "async", "else", "loop", "when", "with", "assert", "super",
    "this", "self", "LOGD", "LOGE", "LOGI", "LOGW",
];

/// A symbol as found in a file, before it has a database id.
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub owner: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    /// Distinct `(callee, written as x.callee())` pairs found in the body.
    pub calls: Vec<(String, bool)>,
}

pub struct ParsedFile {
    pub lines: u32,
    pub symbols: Vec<ParsedSymbol>,
}

fn call_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\.)?\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap())
}

fn jsx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<([A-Z][A-Za-z0-9_]*)").unwrap())
}

/// The Rust parser reports `impl Foo` / `impl<T> Trait for Foo<T>`; the owning type is `Foo`.
fn canonical_type(name: &str) -> String {
    match name.strip_prefix("impl ") {
        Some(rest) => {
            let target = rest.rsplit(" for ").next().unwrap_or(rest);
            target.split('<').next().unwrap_or(target).trim().to_string()
        }
        None => name.to_string(),
    }
}

/// Parses one file into symbols plus the calls found in each symbol's body.
/// None when the file has no parser or cannot be read as text.
pub fn parse_file(path: &Path) -> Option<ParsedFile> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let parser = find_parser(&ext)?;
    if parser.language_name() == "Markdown" {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let (types, methods) = parser.parse(&lines);

    let braces = parser.language_name() != "Python";
    let is_web = matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx");

    // A symbol with no block of its own is bounded by the next symbol's start.
    let mut boundaries: Vec<usize> = types
        .iter()
        .map(|t| t.line)
        .chain(methods.iter().map(|m| m.start_line))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let body_end = |start_line: usize| -> usize {
        let start = start_line.saturating_sub(1);
        let next_start = boundaries
            .iter()
            .find(|&&b| b > start_line)
            .map(|b| b - 1)
            .unwrap_or(lines.len());
        // The real block, nested symbols included: a component's JSX often sits after its inner
        // handlers, so cutting at the next symbol would credit calls to the wrong function.
        let block = if braces {
            brace_block_end(&lines, start, next_start)
        } else {
            indent_block_end(&lines, start)
        };
        block.unwrap_or(next_start).min(lines.len()).max(start_line)
    };

    let mut symbols = Vec::new();

    for t in &types {
        symbols.push(ParsedSymbol {
            name: canonical_type(&t.name),
            kind: SymbolKind::Class,
            detail: t.kind.clone(),
            owner: None,
            start_line: t.line as u32,
            end_line: body_end(t.line) as u32,
            calls: Vec::new(),
        });
    }

    for m in &methods {
        let owner = m.parent.as_deref().map(canonical_type);
        let end = body_end(m.start_line);
        let start = m.start_line.saturating_sub(1);

        let mut seen: HashSet<(String, bool)> = HashSet::new();
        let mut calls: Vec<(String, bool)> = Vec::new();
        let mut add = |name: &str, dotted: bool| {
            if name != m.name && seen.insert((name.to_string(), dotted)) {
                calls.push((name.to_string(), dotted));
            }
        };

        for line in &lines[start..end] {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
                continue;
            }
            for cap in call_re().captures_iter(line) {
                let callee = &cap[2];
                if !NOT_CALLS.contains(&callee) {
                    add(callee, cap.get(1).is_some());
                }
            }
            if is_web {
                for cap in jsx_re().captures_iter(line) {
                    add(&cap[1], false);
                }
            }
        }

        symbols.push(ParsedSymbol {
            name: m.name.clone(),
            kind: if owner.is_some() { SymbolKind::Method } else { SymbolKind::Function },
            detail: m.kind.clone(),
            owner,
            start_line: m.start_line as u32,
            end_line: end as u32,
            calls,
        });
    }

    Some(ParsedFile { lines: lines.len() as u32, symbols })
}

/// Last line (exclusive) of the `{ ... }` block that opens before `opening_limit`
/// (the next symbol's line). None when the symbol has no block (e.g. a one-line arrow function).
fn brace_block_end(lines: &[&str], start: usize, opening_limit: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !opened && i >= opening_limit {
            return None;
        }
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some(i + 1);
        }
    }
    None
}

/// Python: the body ends at the next non-blank line indented no deeper than the `def` line.
fn indent_block_end(lines: &[&str], start: usize) -> Option<usize> {
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let base = indent_of(lines.get(start)?);
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if !line.trim().is_empty() && indent_of(line) <= base {
            return Some(i);
        }
    }
    Some(lines.len())
}
