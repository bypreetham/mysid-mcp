use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct CSharpParser;

impl LanguageParser for CSharpParser {
    fn language_name(&self) -> &'static str {
        "C#"
    }

    fn extensions(&self) -> &[&'static str] {
        &["cs"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|abstract|static|sealed|partial)\s+)*(class|interface|struct|record|enum)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let method_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|abstract|static|virtual|override|async|sealed|extern)\s+)+(?:[A-Za-z0-9_<>,.?\[\]]+\s+)+([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();

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

            if let Some(caps) = method_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                if !matches!(name.as_str(), "if" | "while" | "for" | "switch" | "catch") {
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
