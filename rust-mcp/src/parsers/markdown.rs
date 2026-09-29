use super::{LanguageParser, MethodSignature, TypeInfo};

pub struct MarkdownParser;

impl LanguageParser for MarkdownParser {
    fn language_name(&self) -> &'static str {
        "Markdown"
    }

    fn extensions(&self) -> &[&'static str] {
        &["md", "markdown"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types: Vec<TypeInfo> = Vec::new();
        let mut methods: Vec<MethodSignature> = Vec::new();
        let mut in_code_block = false;
        let mut heading_stack: Vec<(usize, String)> = Vec::new();

        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();

            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_code_block = !in_code_block;
                continue;
            }

            if in_code_block {
                continue;
            }

            if trimmed.starts_with('#') {
                let level = trimmed.chars().take_while(|c| *c == '#').count();
                if (1..=6).contains(&level) {
                    let rest = trimmed[level..].trim();
                    if !rest.is_empty() {
                        let title = rest.to_string();

                        while let Some((last_lvl, _)) = heading_stack.last() {
                            if *last_lvl >= level {
                                heading_stack.pop();
                            } else {
                                break;
                            }
                        }
                        let parent = heading_stack.last().map(|(_, p)| p.clone());
                        heading_stack.push((level, title.clone()));

                        if level <= 2 {
                            types.push(TypeInfo {
                                name: title.clone(),
                                kind: format!("h{}", level),
                                line: idx + 1,
                            });
                        }

                        if let Some(prev) = methods.last_mut() {
                            prev.end_line = idx;
                        }

                        methods.push(MethodSignature {
                            name: title,
                            kind: format!("h{}", level),
                            signature: trimmed.to_string(),
                            params: String::new(),
                            return_type: None,
                            start_line: idx + 1,
                            end_line: lines.len(),
                            parent,
                        });
                    }
                }
            }
        }

        (types, methods)
    }
}
