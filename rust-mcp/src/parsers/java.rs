use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct JavaParser;

impl LanguageParser for JavaParser {
    fn language_name(&self) -> &'static str {
        "Java"
    }

    fn extensions(&self) -> &[&'static str] {
        &["java"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|abstract|static|final|sealed|non-sealed)\s+)*(class|interface|enum|record|@interface)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let method_re = Regex::new(
            r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*(?:(?:public|private|protected|abstract|static|final|synchronized|native|default)\s+)+(?:<[^>]+>\s+)?([A-Za-z0-9_<>,.?\[\]]+)\s+([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();
        let constructor_re =
            Regex::new(r"^\s*(?:(?:public|private|protected)\s+)([A-Za-z0-9_]+)\s*\(").unwrap();

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

            if let Some(caps) = method_re.captures(trimmed) {
                let ret = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let name = caps.get(2).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');

                methods.push(MethodSignature {
                    name,
                    kind: "method".to_string(),
                    signature: sig,
                    params,
                    return_type: if !ret.is_empty() { Some(ret) } else { None },
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            } else if let Some(caps) = constructor_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                if Some(&name) == current_type.as_ref() {
                    let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                    methods.push(MethodSignature {
                        name,
                        kind: "constructor".to_string(),
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
