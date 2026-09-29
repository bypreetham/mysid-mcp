use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct SwiftParser;

impl LanguageParser for SwiftParser {
    fn language_name(&self) -> &'static str {
        "Swift"
    }

    fn extensions(&self) -> &[&'static str] {
        &["swift"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:(?:public|private|fileprivate|internal|open|final)\s+)*(class|struct|protocol|enum|extension)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let func_re = Regex::new(
            r"^\s*(?:(?:public|private|fileprivate|internal|open|override|static|class|mutating)\s+)*func\s+([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();
        let init_re = Regex::new(r"^\s*(?:(?:public|private|internal)\s+)*init\s*\(").unwrap();

        let mut current_type: Option<String> = None;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") {
                continue;
            }

            if let Some(caps) = type_re.captures(trimmed) {
                let kind = caps.get(1).map_or("class", |m| m.as_str());
                let name = caps.get(2).map_or("", |m| m.as_str());
                types.push(TypeInfo {
                    name: name.to_string(),
                    kind: kind.to_string(),
                    line: idx + 1,
                });
                current_type = Some(name.to_string());
            }

            if let Some(caps) = func_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                let return_type = sig.split("->").nth(1).map(|s| s.trim().to_string());

                methods.push(MethodSignature {
                    name,
                    kind: "method".to_string(),
                    signature: sig,
                    params,
                    return_type,
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            } else if init_re.is_match(trimmed) {
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: "init".to_string(),
                    kind: "initializer".to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            }
        }

        (types, methods)
    }
}
