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

        // String literals are blanked before counting braces so `"{"` can't skew the depth.
        let string_re = Regex::new(r#""[^"]*"|'[^']*'|`[^`]*`"#).unwrap();

        let mut current_type: Option<String> = None;
        let mut current_kind = String::new();
        // Brace depth at which `current_type` was declared, so it ends with its own block.
        let mut depth: i32 = 0;
        let mut type_depth: i32 = 0;
        let mut type_block_seen = false;

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with('*') || trimmed.starts_with("/*") {
                continue;
            }

            // A column-0 declaration after a block-less type (`type A = B`) ends that type.
            if current_type.is_some()
                && !type_block_seen
                && !line.starts_with(char::is_whitespace)
                && !trimmed.starts_with('{')
                && !trimmed.is_empty()
                && !type_re.is_match(trimmed)
            {
                current_type = None;
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
                    current_kind = kind.to_string();
                    type_depth = depth;
                    type_block_seen = false;
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
                let is_member_level = depth == type_depth + 1
                    && matches!(current_kind.as_str(), "class" | "interface");
                if !matches!(name.as_str(), "if" | "while" | "for" | "switch" | "catch")
                    && current_type.is_some()
                    && is_member_level
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

            let code = string_re.replace_all(line, "");
            let opens = code.matches('{').count() as i32;
            let closes = code.matches('}').count() as i32;
            if opens > 0 && current_type.is_some() {
                type_block_seen = true;
            }
            depth += opens - closes;
            if current_type.is_some()
                && ((type_block_seen && depth <= type_depth)
                    || (!type_block_seen && trimmed.ends_with(';')))
            {
                current_type = None;
            }
        }

        (types, methods)
    }
}
