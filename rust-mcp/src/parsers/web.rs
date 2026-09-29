use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct WebParser;

impl LanguageParser for WebParser {
    fn language_name(&self) -> &'static str {
        "TypeScript / JavaScript"
    }

    fn extensions(&self) -> &[&'static str] {
        &["ts", "tsx", "js", "jsx", "mjs", "cjs"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?(class|interface|type|enum)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let func_re = Regex::new(
            r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s*(?:\*\s*)?([A-Za-z0-9_]*)\s*\(",
        )
        .unwrap();
        let arrow_re = Regex::new(
            r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Za-z0-9_]+)\s*=\s*(?:async\s*)?\(([^)]*)\)\s*(?::\s*([^=]+))?\s*=>",
        )
        .unwrap();
        let method_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|static|readonly|async|override|get|set)\s+)*([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();

        let mut current_type: Option<String> = None;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
                continue;
            }

            if let Some(caps) = type_re.captures(trimmed) {
                let kind = caps.get(1).map_or("class", |m| m.as_str());
                let name = caps.get(2).map_or("", |m| m.as_str());
                if !name.is_empty() {
                    types.push(TypeInfo {
                        name: name.to_string(),
                        kind: kind.to_string(),
                        line: idx + 1,
                    });
                    current_type = Some(name.to_string());
                }
            }

            if let Some(caps) = func_re.captures(trimmed) {
                let name = caps.get(1).map_or("anonymous", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                let return_type = sig.rsplit(':').next().and_then(|s| {
                    let t = s.trim();
                    if !t.contains('(') && !t.is_empty() {
                        Some(t.to_string())
                    } else {
                        None
                    }
                });

                methods.push(MethodSignature {
                    name,
                    kind: "function".to_string(),
                    signature: sig,
                    params,
                    return_type,
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            } else if let Some(caps) = arrow_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let params = caps.get(2).map_or("", |m| m.as_str()).trim().to_string();
                let ret = caps.get(3).map(|m| m.as_str().trim().to_string());

                methods.push(MethodSignature {
                    name,
                    kind: "arrow_function".to_string(),
                    signature: trimmed.split("=>").next().unwrap_or(trimmed).trim().to_string(),
                    params,
                    return_type: ret,
                    start_line: idx + 1,
                    end_line: idx + 1,
                    parent: current_type.clone(),
                });
            } else if let Some(caps) = method_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                if !matches!(name.as_str(), "if" | "while" | "for" | "switch" | "catch")
                    && current_type.is_some()
                {
                    let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                    methods.push(MethodSignature {
                        name,
                        kind: "method".to_string(),
                        signature: sig,
                        params,
                        return_type: None,
                        start_line: idx + 1,
                        end_line,
                        parent: current_type.clone(),
                    });
                }
            }
        }

        (types, methods)
    }
}
