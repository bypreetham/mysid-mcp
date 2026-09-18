use super::{StackAdapter, SymbolInfo, SymbolSpan};
use regex::Regex;

pub struct BaseAdapter {
    func_re: Regex,
    class_re: Regex,
}

impl BaseAdapter {
    pub fn new() -> Self {
        Self {
            func_re: Regex::new(r"^\s*(?:pub\s+|private\s+|protected\s+|async\s+|def\s+|fun\s+|fn\s+|function\s+)([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
            class_re: Regex::new(r"^\s*(?:pub\s+)?(?:class|struct|interface|enum|trait)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
        }
    }
}

impl Default for BaseAdapter {
    fn default() -> Self {
        Self::new()
    }
}

const BASE_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "dist",
    "build",
    "target",
    ".gradle",
    ".idea",
    ".orchestrator",
    "old",
    ".next",
    ".turbo",
    ".cxx",
    "out",
    "bin",
];

const BASE_EXTS: &[&str] = &[
    "rs", "py", "js", "ts", "jsx", "tsx", "java", "kt", "c", "cpp", "cc", "h", "hpp", "go",
];

impl StackAdapter for BaseAdapter {
    fn name(&self) -> &'static str {
        "Base / Hybrid Adapter"
    }

    fn source_dirs(&self) -> &[&'static str] {
        &["src", "app", "lib"]
    }

    fn skip_dirs(&self) -> &[&'static str] {
        BASE_SKIP_DIRS
    }

    fn file_extensions(&self) -> &[&'static str] {
        BASE_EXTS
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
                        end_line: line_num + 15,
                    });
                }
            } else if let Some(caps) = self.class_re.captures(line) {
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
        let target_sym = symbol.split("::").last().unwrap_or(symbol).trim();

        for (idx, line) in lines.iter().enumerate() {
            if line.contains(target_sym) && (line.contains('{') || lines.get(idx + 1).map_or(false, |l| l.contains('{')) || line.trim_start().starts_with("def ")) {
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
                    // Cap max definition extract to 60 lines
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
