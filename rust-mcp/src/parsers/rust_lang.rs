use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct RustParser;

impl LanguageParser for RustParser {
    fn language_name(&self) -> &'static str {
        "Rust"
    }

    fn extensions(&self) -> &[&'static str] {
        &["rs"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:pub(?:\([^)]+\))?\s+)?(struct|enum|trait|union)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let impl_re = Regex::new(
            r"^\s*impl(?:<[^>]+>)?\s+(?:([A-Za-z0-9_:<>, ]+)\s+for\s+)?([A-Za-z0-9_:<>, ]+)",
        )
        .unwrap();
        let fn_re = Regex::new(
            r#"^\s*(?:pub(?:\([^)]+\))?\s+)?(?:(?:async|const|unsafe|extern(?:\s+"[^"]+")?)\s+)*fn\s+([A-Za-z0-9_]+)"#,
        )
        .unwrap();

        let mut current_type: Option<String> = None;
        // Brace depth at which `current_type` was declared, so it ends with its own block.
        let mut depth: i32 = 0;
        let mut type_depth: Option<i32> = None;
        let mut type_block_seen = false;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') {
                continue;
            }

            if let Some(caps) = type_re.captures(trimmed) {
                let kind = caps.get(1).map_or("struct", |m| m.as_str());
                let name = caps.get(2).map_or("", |m| m.as_str());
                if !name.is_empty() {
                    types.push(TypeInfo {
                        name: name.to_string(),
                        kind: kind.to_string(),
                        line: idx + 1,
                    });
                    current_type = Some(name.to_string());
                    type_depth = Some(depth);
                    type_block_seen = false;
                }
            } else if let Some(caps) = impl_re.captures(trimmed) {
                let tr = caps.get(1).map(|m| m.as_str().trim().to_string());
                let target = caps
                    .get(2)
                    .map(|m| m.as_str().trim().to_string())
                    .unwrap_or_default();
                let label = if let Some(t) = tr {
                    format!("impl {} for {}", t, target)
                } else {
                    format!("impl {}", target)
                };
                types.push(TypeInfo {
                    name: label.clone(),
                    kind: "impl".to_string(),
                    line: idx + 1,
                });
                current_type = Some(label);
                type_depth = Some(depth);
                type_block_seen = false;
            }

            if let Some(caps) = fn_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                let return_type = sig.split("->").nth(1).map(|s| s.trim().to_string());

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
            }

            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            if opens > 0 {
                type_block_seen = true;
            }
            depth += opens - closes;
            if let Some(d) = type_depth {
                // `struct Foo;` / `struct Foo(u32);` have no block; others end when depth returns.
                if (type_block_seen && depth <= d) || (!type_block_seen && trimmed.ends_with(';')) {
                    current_type = None;
                    type_depth = None;
                }
            }
        }

        (types, methods)
    }
}
