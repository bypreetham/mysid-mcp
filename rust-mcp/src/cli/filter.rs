//! `--head/--tail/--grep/--grep-v/--errors`: trims the printed output so nothing needs piping.

use serde_json::Value;


/// `--head N`, `--tail N`, `--grep <regex>`, `--grep-v <regex>` on any command: trims the printed output, so nothing
/// needs piping through head, tail or grep. Smart case: a lower-case pattern ignores case. Not for MCP calls.
pub(crate) struct OutputFilter {
    head: Option<usize>,
    tail: Option<usize>,
    grep: Option<regex::Regex>,
    grep_v: Option<regex::Regex>,
    described: Vec<String>,
}

/// What `--errors` keeps: error and fatal lines, MSVC linker/compiler codes, undefined references, failures, exceptions,
/// and mysid's own `[exit N]` / `[timed out` lines.
pub(crate) const ERRORS_PATTERN: &str = r"\berror\b|\bfatal\b|\bLNK\d+|undefined reference|cannot (open|find)|\bfailed\b|exception|warning C\d+|^\[(exit|timed out)";

pub(crate) fn smart_regex(pattern: &str) -> regex::Regex {
    let ignore_case = !pattern.chars().any(|c| c.is_uppercase());
    regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .unwrap_or_else(|_| regex::RegexBuilder::new(&regex::escape(pattern)).case_insensitive(ignore_case).build().unwrap())
}

impl OutputFilter {
    pub(crate) fn take(params: &mut Value) -> Result<OutputFilter, String> {
        let mut f = OutputFilter { head: None, tail: None, grep: None, grep_v: None, described: Vec::new() };
        let Some(obj) = params.as_object_mut() else { return Ok(f) };
        let num = |key: &str, obj: &mut serde_json::Map<String, Value>| -> Result<Option<usize>, String> {
            match obj.remove(key) {
                None => Ok(None),
                Some(v) => v
                    .as_u64()
                    .map(|n| n as usize)
                    .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
                    .map(Some)
                    .ok_or_else(|| format!("--{} needs a number of lines, for example --{} 20", key, key)),
            }
        };
        f.head = num("head", obj)?;
        f.tail = num("tail", obj)?;
        for (key, slot) in [("grep", 0), ("grep-v", 1), ("grep_v", 1)] {
            if let Some(v) = obj.remove(key) {
                let p = v.as_str().map(String::from).ok_or_else(|| format!("--{} needs a pattern, for example --{} error", key, key))?;
                f.described.push(format!("--{} {}", key.replace('_', "-"), p));
                if slot == 0 { f.grep = Some(smart_regex(&p)) } else { f.grep_v = Some(smart_regex(&p)) }
            }
        }
        // `--errors`: only the lines that say something went wrong (and the exit line), whatever the tool.
        if let Some(v) = obj.remove("errors") {
            let on = match &v {
                Value::Bool(b) => *b,
                Value::String(s) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no"),
                _ => true,
            };
            if on && f.grep.is_none() {
                f.grep = regex::RegexBuilder::new(ERRORS_PATTERN).case_insensitive(true).build().ok();
                f.described.push("--errors".to_string());
            }
        }
        if let Some(n) = f.head {
            f.described.push(format!("--head {}", n));
        }
        if let Some(n) = f.tail {
            f.described.push(format!("--tail {}", n));
        }
        Ok(f)
    }

    pub(crate) fn apply(&self, text: &str) -> String {
        if self.head.is_none() && self.tail.is_none() && self.grep.is_none() && self.grep_v.is_none() {
            return text.to_string();
        }
        let all: Vec<&str> = text.lines().collect();
        let mut kept: Vec<&str> = all.clone();
        if let Some(re) = &self.grep {
            kept.retain(|l| re.is_match(l));
        }
        if let Some(re) = &self.grep_v {
            kept.retain(|l| !re.is_match(l));
        }
        if let Some(n) = self.head {
            kept.truncate(n);
        }
        if let Some(n) = self.tail {
            let skip = kept.len().saturating_sub(n);
            kept = kept.split_off(skip);
        }
        let mut out = kept.join("\n");
        if kept.len() != all.len() {
            out.push_str(&format!("\n[{} of {} lines: {}]", kept.len(), all.len(), self.described.join(" ")));
        }
        out
    }
}
