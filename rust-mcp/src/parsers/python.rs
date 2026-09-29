use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct PythonParser;

impl LanguageParser for PythonParser {
    fn language_name(&self) -> &'static str {
        "Python"
    }

    fn extensions(&self) -> &[&'static str] {
        &["py", "pyw"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let class_re = Regex::new(r"^\s*class\s+([A-Za-z0-9_]+)").unwrap();
        let def_re = Regex::new(r"^\s*(?:async\s+)?def\s+([A-Za-z0-9_]+)\s*\(").unwrap();

        let mut current_class: Option<String> = None;
        let mut current_indent = 0;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                continue;
            }

            let indent = line.len() - line.trim_start().len();

            if let Some(caps) = class_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                types.push(TypeInfo {
                    name: name.clone(),
                    kind: "class".to_string(),
                    line: idx + 1,
                });
                current_class = Some(name);
                current_indent = indent;
            }

            if let Some(caps) = def_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, ':');
                let return_type = sig
                    .split("->")
                    .nth(1)
                    .map(|s| s.trim().trim_end_matches(':').trim().to_string());

                let parent = if indent > current_indent {
                    current_class.clone()
                } else {
                    current_class = None;
                    None
                };

                methods.push(MethodSignature {
                    name,
                    kind: if parent.is_some() {
                        "method".to_string()
                    } else {
                        "function".to_string()
                    },
                    signature: sig,
                    params,
                    return_type,
                    start_line: idx + 1,
                    end_line,
                    parent,
                });
            }
        }

        (types, methods)
    }
}
