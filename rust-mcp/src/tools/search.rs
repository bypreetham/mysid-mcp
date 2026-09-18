use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::Value;
use std::fs;
use std::path::Path;

use super::workspace::resolve_root;
use crate::adapters::detect_adapter;

pub fn execute_search(params: &Value, default_root: Option<&str>) -> String {
    let project_path = match resolve_root(params, "project_path", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let adapter = detect_adapter(Path::new(&project_path));

    let terms = collect_terms(params);
    if terms.is_empty() {
        return "ERROR: search() requires at least one term. Example: search(timer)".to_string();
    }

    let project_dir = Path::new(&project_path);
    if !project_dir.exists() {
        return format!("ERROR: Project directory does not exist: {}", project_path);
    }

    let is_regex = params
        .get("is_regex")
        .and_then(|v| v.as_bool())
        .unwrap_or(terms.len() > 1);
    let max_results = params
        .get("max_results")
        .and_then(|v| v.as_u64())
        .unwrap_or(15)
        .clamp(1, 25) as usize;
    let case_sensitive = params
        .get("case_sensitive")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let context_lines = params
        .get("context_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        .min(2) as usize;
    let file_types: Vec<String> = params
        .get("file_types")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| s.trim_start_matches('.').to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let is_pure_symbol = terms.len() == 1 && !is_regex && terms[0].chars().all(|c| c.is_alphanumeric() || c == '_');
    let pattern = if is_pure_symbol {
        format!(r"\b{}\b", regex::escape(&terms[0]))
    } else if terms.len() == 1 && !is_regex {
        regex::escape(&terms[0])
    } else if terms.len() == 1 {
        terms[0].clone()
    } else {
        terms.iter().map(|t| regex::escape(t)).collect::<Vec<_>>().join("|")
    };

    let regex = match RegexBuilder::new(&pattern)
        .case_insensitive(!case_sensitive)
        .build()
    {
        Ok(r) => r,
        Err(e) => return format!("ERROR: invalid search pattern: {e}"),
    };

    let mut results = Vec::new();
    let skip_dirs: Vec<String> = adapter.skip_dirs().iter().map(|s| s.to_string()).collect();
    let mut walker = WalkBuilder::new(project_dir);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |entry| {
            let name = entry.file_name().to_string_lossy();
            !skip_dirs.iter().any(|d| d == &name.as_ref())
        });

const BINARY_EXTS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "ico", "webp", "svg", "mp3", "wav", "ogg", "flac",
    "mp4", "avi", "mov", "mkv", "zip", "tar", "gz", "7z", "rar", "jar", "aar",
    "apk", "aab", "dex", "so", "dylib", "dll", "exe", "bin", "class", "pyc", "pdf",
    "db", "sqlite", "sqlite3"
];

    for dent in walker.build().filter_map(|e| e.ok()) {
        if results.len() >= max_results {
            break;
        }
        let path = dent.path();
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if BINARY_EXTS.contains(&ext.as_str()) {
            continue;
        }

        // By default, skip documentation/markdown files to avoid dumping docs into context
        if file_types.is_empty() && (ext == "md" || ext == "txt" || ext == "rst") {
            continue;
        }

        if !file_types.is_empty() && !file_types.iter().any(|t| t.eq_ignore_ascii_case(&ext)) {
            continue;
        }

        // Skip files larger than 1.5MB to maintain sub-second speed
        if let Ok(meta) = dent.metadata() {
            if meta.len() > 1_500_000 {
                continue;
            }
        }

        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if results.len() >= max_results {
                break;
            }
            if !regex.is_match(line) {
                continue;
            }
            let rel = path_relative_to(path, project_dir);
            if context_lines == 0 {
                results.push(format!("{}:{}: {}", rel, idx + 1, line.trim_end()));
            } else {
                let start = idx.saturating_sub(context_lines);
                let end = (idx + 1 + context_lines).min(lines.len());
                for (j, ctx) in lines[start..end].iter().enumerate() {
                    let n = start + j + 1;
                    let mark = if start + j == idx { ":" } else { "-" };
                    results.push(format!("{rel}{mark}{n}: {}", ctx.trim_end()));
                    if results.len() >= max_results {
                        break;
                    }
                }
            }
        }
    }

    if results.is_empty() {
        return "No matches found.".to_string();
    }
    results.join("\n")
}

fn collect_terms(params: &Value) -> Vec<String> {
    if let Some(query) = params.get("query").and_then(|v| v.as_str()) {
        let q = query.trim();
        if !q.is_empty() {
            return vec![q.to_string()];
        }
    }
    let mut terms = Vec::new();
    if let Some(args) = params.get("args").and_then(|v| v.as_array()) {
        for arg in args {
            if let Some(s) = arg.as_str() {
                if !s.trim().is_empty() {
                    terms.push(s.trim().to_string());
                }
            }
        }
    }
    terms
}

fn path_relative_to(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}
