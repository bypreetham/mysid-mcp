use super::{StackAdapter, SymbolInfo, SymbolSpan};
use regex::Regex;

pub struct AndroidAdapter {
    func_re: Regex,
    class_re: Regex,
}

impl AndroidAdapter {
    pub fn new() -> Self {
        Self {
            func_re: Regex::new(r"^\s*(?:(?:public|private|protected|virtual|static|inline|explicit|friend)\s+)*(?:[\w:<>&*]+\s+)+([a-zA-Z_][a-zA-Z0-9_]*)\s*\([^)]*\)\s*(?:const)?\s*(?:override)?\s*\{?").unwrap(),
            class_re: Regex::new(r"^\s*(?:class|struct|interface|enum)\s+([a-zA-Z_][a-zA-Z0-9_]*)").unwrap(),
        }
    }
}

impl Default for AndroidAdapter {
    fn default() -> Self {
        Self::new()
    }
}

const ANDROID_SKIP_DIRS: &[&str] = &[
    ".git",
    ".gradle",
    ".idea",
    ".cxx",
    "build",
    "intermediates",
    "generated",
    "outputs",
    "obj",
    "captures",
];

const ANDROID_EXTS: &[&str] = &[
    "cpp", "cc", "c", "h", "hpp", "kt", "java", "gradle", "cmake", "txt", "xml",
];

impl StackAdapter for AndroidAdapter {
    fn name(&self) -> &'static str {
        "Android Native (C++ / Oboe / Kotlin / JNI)"
    }

    fn source_dirs(&self) -> &[&'static str] {
        &["app/src/main/cpp", "app/src/main/java", "app/src/main/kotlin"]
    }

    fn skip_dirs(&self) -> &[&'static str] {
        ANDROID_SKIP_DIRS
    }

    fn file_extensions(&self) -> &[&'static str] {
        ANDROID_EXTS
    }

    fn extract_symbols(&self, content: &str, ext: &str) -> Vec<SymbolInfo> {
        let mut symbols = Vec::new();
        let is_cpp = matches!(ext, "cpp" | "cc" | "c" | "h" | "hpp");

        for (idx, line) in content.lines().enumerate() {
            let line_num = idx + 1;
            if is_cpp {
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
            }
            if let Some(caps) = self.class_re.captures(line) {
                if let Some(m) = caps.get(1) {
                    symbols.push(SymbolInfo {
                        symbol_name: m.as_str().to_string(),
                        symbol_type: "class".to_string(),
                        start_line: line_num,
                        end_line: line_num + 40,
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
            // Check for Class::Method or Method signature
            if (line.contains(symbol) || (line.contains(target_sym) && (line.contains("::") || line.contains("void ") || line.contains("int ") || line.contains("bool ") || line.contains("auto "))))
                && !line.trim_start().starts_with("//")
                && !line.trim_start().starts_with("/*")
            {
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
                    if j - idx >= 70 {
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
