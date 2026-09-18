use super::{StackAdapter, SymbolInfo, SymbolSpan};
use regex::Regex;

pub struct ReactTsAdapter {
    func_re: Regex,
    type_re: Regex,
}

impl ReactTsAdapter {
    pub fn new() -> Self {
        Self {
            func_re: Regex::new(r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?(?:function\*?\s+([a-zA-Z_][a-zA-Z0-9_]*)|const\s+([a-zA-Z_][a-zA-Z0-9_]*)\s*[:=])").unwrap(),
            type_re: Regex::new(r"^\s*(?:export\s+)?(?:type|interface|class|enum)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
        }
    }
}

impl Default for ReactTsAdapter {
    fn default() -> Self {
        Self::new()
    }
}

const REACT_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".next",
    ".turbo",
    "dist",
    "build",
    "out",
    "coverage",
    ".storybook",
];

const REACT_EXTS: &[&str] = &["tsx", "ts", "jsx", "js", "mjs", "json", "css"];

impl StackAdapter for ReactTsAdapter {
    fn name(&self) -> &'static str {
        "React / TypeScript / Vite / Next.js"
    }

    fn source_dirs(&self) -> &[&'static str] {
        &["src", "components", "pages", "app", "lib", "hooks"]
    }

    fn skip_dirs(&self) -> &[&'static str] {
        REACT_SKIP_DIRS
    }

    fn file_extensions(&self) -> &[&'static str] {
        REACT_EXTS
    }

    fn extract_symbols(&self, content: &str, _ext: &str) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            if let Some(caps) = self.func_re.captures(line) {
                let name = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str());
                if let Some(n) = name {
                    symbols.push(SymbolInfo {
                        symbol_name: n.to_string(),
                        symbol_type: "component/function".to_string(),
                        start_line: line_num,
                        end_line: line_num + 25,
                    });
                    continue;
                }
            }
            if let Some(caps) = self.type_re.captures(line) {
                if let Some(m) = caps.get(1) {
                    symbols.push(SymbolInfo {
                        symbol_name: m.as_str().to_string(),
                        symbol_type: "type/interface".to_string(),
                        start_line: line_num,
                        end_line: line_num + 15,
                    });
                }
            }
        }
        symbols
    }

    fn extract_definition(&self, content: &str, _ext: &str, symbol: &str) -> Option<SymbolSpan> {
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if line.contains(symbol) && (line.contains("function") || line.contains("const") || line.contains("interface") || line.contains("type") || line.contains("class")) {
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
