use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct CppParser;

impl LanguageParser for CppParser {
    fn language_name(&self) -> &'static str {
        "C/C++"
    }

    fn extensions(&self) -> &[&'static str] {
        &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:template\s*<[^>]*>\s*)?(class|struct|namespace|enum(?:\s+class)?)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let func_re = Regex::new(
            r"^\s*(?:template\s*<[^>]*>\s*)?(?:(?:virtual|static|inline|explicit|friend|constexpr)\s+)*(?:(?:const|unsigned|signed|struct|class|enum)\s+)*([A-Za-z0-9_:<>,*&~]+(?:\s*[*&]+)?)\s+([A-Za-z0-9_:]+)\s*\(",
        )
        .unwrap();
        let dtor_re = Regex::new(
            r"^\s*(?:virtual\s+)?~([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();

        let mut current_type: Option<String> = None;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//")
                || trimmed.starts_with('*')
                || trimmed.starts_with("/*")
                || trimmed.starts_with('#')
            {
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
                let ret = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let name = caps.get(2).map_or("", |m| m.as_str()).to_string();

                let is_keyword = matches!(
                    name.as_str(),
                    "if" | "while" | "for" | "switch" | "catch" | "return"
                );
                let is_raii_lock = ret.contains("lock_guard")
                    || ret.contains("unique_lock")
                    || ret.contains("shared_lock")
                    || ret.contains("scoped_lock");

                if !is_keyword && !is_raii_lock {
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
                }
            } else if let Some(caps) = dtor_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: format!("~{}", name),
                    kind: "destructor".to_string(),
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
