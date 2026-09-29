use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct GoParser;

impl LanguageParser for GoParser {
    fn language_name(&self) -> &'static str {
        "Go"
    }

    fn extensions(&self) -> &[&'static str] {
        &["go"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(r"^\s*type\s+([A-Za-z0-9_]+)\s+(struct|interface)").unwrap();
        let method_re = Regex::new(r"^\s*func\s*\(([^)]+)\)\s*([A-Za-z0-9_]+)\s*\(").unwrap();
        let func_re = Regex::new(r"^\s*func\s+([A-Za-z0-9_]+)\s*\(").unwrap();

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }

            if let Some(caps) = type_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let kind = caps.get(2).map_or("struct", |m| m.as_str());
                types.push(TypeInfo {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    line: idx + 1,
                });
            }

            if let Some(caps) = method_re.captures(trimmed) {
                let recv = caps.get(1).map_or("", |m| m.as_str()).trim();
                let name = caps.get(2).map_or("", |m| m.as_str());
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');

                methods.push(MethodSignature {
                    name: name.to_string(),
                    kind: "method".to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: Some(recv.to_string()),
                });
            } else if let Some(caps) = func_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str());
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');

                methods.push(MethodSignature {
                    name: name.to_string(),
                    kind: "function".to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: None,
                });
            }
        }

        (types, methods)
    }
}
