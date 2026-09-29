use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct KotlinParser;

impl LanguageParser for KotlinParser {
    fn language_name(&self) -> &'static str {
        "Kotlin"
    }

    fn extensions(&self) -> &[&'static str] {
        &["kt", "kts"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|abstract|open|sealed|data|enum|value|inline)\s+)*(class|interface|object)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let fun_re = Regex::new(
            r"^\s*(?:(?:public|private|protected|internal|override|suspend|inline|open|abstract|tailrec|operator|infix)\s+)*fun\s+(?:<[^>]+>\s+)?(?:[A-Za-z0-9_<>,.? ]+\.)?([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();
        let constructor_re =
            Regex::new(r"^\s*(?:(?:public|private|protected|internal)\s+)*constructor\s*\(").unwrap();

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

            if let Some(caps) = fun_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                let return_type = if let Some(close_idx) = sig.rfind(')') {
                    let after_paren = sig[close_idx + 1..].trim();
                    if after_paren.starts_with(':') {
                        Some(after_paren[1..].trim().trim_end_matches('{').trim().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };

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
            } else if constructor_re.is_match(trimmed) {
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: "constructor".to_string(),
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

        (types, methods)
    }
}
