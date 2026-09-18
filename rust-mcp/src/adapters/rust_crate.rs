use super::{StackAdapter, SymbolInfo, SymbolSpan};
use regex::Regex;

pub struct RustCrateAdapter {
    func_re: Regex,
    type_re: Regex,
}

impl RustCrateAdapter {
    pub fn new() -> Self {
        Self {
            func_re: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:async\s+)?fn\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            type_re: Regex::new(r"^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:struct|enum|trait|impl(?:\s+<[^>]+>)?)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
        }
    }
}

impl Default for RustCrateAdapter {
    fn default() -> Self {
        Self::new()
    }
}

const RUST_SKIP_DIRS: &[&str] = &[".git", "target", ".cargo"];
const RUST_EXTS: &[&str] = &["rs", "toml"];

impl StackAdapter for RustCrateAdapter {
    fn name(&self) -> &'static str {
        "Rust Crate / Workspace"
    }

    fn source_dirs(&self) -> &[&'static str] {
        &["src", "tests", "benches", "examples"]
    }

    fn skip_dirs(&self) -> &[&'static str] {
        RUST_SKIP_DIRS
    }

    fn file_extensions(&self) -> &[&'static str] {
        RUST_EXTS
    }

    fn extract_symbols(&self, content: &str, _ext: &str) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = self.func_re.captures(line) {
                if let Some(m) = caps.get(1) {
                    symbols.push(SymbolInfo {
                        symbol_name: m.as_str().to_string(),
                        symbol_type: "function".to_string(),
                        start_line: line_num,
                        end_line: line_num + 20,
                    });
                    continue;
                }
            }
            if let Some(caps) = self.type_re.captures(line) {
                if let Some(m) = caps.get(1) {
                    symbols.push(SymbolInfo {
                        symbol_name: m.as_str().to_string(),
                        symbol_type: "type".to_string(),
                        start_line: line_num,
                        end_line: line_num + 30,
                    });
                }
            }
        }
        symbols
    }

    fn extract_definition(&self, content: &str, _ext: &str, symbol: &str) -> Option<SymbolSpan> {
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if line.contains(symbol) && (line.contains("fn ") || line.contains("struct ") || line.contains("enum ") || line.contains("impl ")) {
                let start_line = idx + 1;
                let mut brace_depth: i32 = 0;
                let mut found_open = false;
                let mut end_idx = idx;

                for (j, cur_line) in lines.iter().enumerate().skip(idx) {
                    end_idx = j;
                    for c in cur_line.chars() {
                        if c == '{' {
                            brace_depth += 1;
                            found_open = true;
                        } else if c == '}' {
                            brace_depth -= 1;
                        }
                    }
                    if found_open && brace_depth <= 0 {
                        break;
                    }
                    if j - idx >= 60 {
                        break;
                    }
                }

                let end_line = end_idx + 1;
                let body = lines[idx..=end_idx].join("\n");

                return Some(SymbolSpan {
                    symbol_name: symbol.to_string(),
                    symbol_type: "definition".to_string(),
                    start_line,
                    end_line,
                    signature: line.trim().to_string(),
                    body,
                });
            }
        }
        None
    }
}
