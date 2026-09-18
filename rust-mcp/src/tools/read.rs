use ignore::WalkBuilder;
use regex::Regex;
use serde_json::Value;
use std::fs;
use std::path::Path;

use super::workspace::{resolve_root, resolve_file_in_workspace};
use crate::adapters::detect_adapter;

pub fn execute_read(params: &Value, default_root: Option<&str>) -> String {
    // If explicit batch params are present, delegate to execute_read_batch
    if params.get("files").is_some() || params.get("specs").is_some() || params.get("targets").is_some() {
        return execute_read_batch(params, default_root);
    }

    // Check if multiple positional args are passed, or if path contains multiple tokens
    if let Some(args_arr) = params.get("args").and_then(|v| v.as_array()) {
        if args_arr.len() > 1 {
            // Check if second arg is NOT a numeric offset/limit or "end"
            let second_is_number = args_arr.get(1).and_then(|v| {
                v.as_i64()
                    .map(|_| true)
                    .or_else(|| v.as_str().map(|s| s.parse::<i64>().is_ok() || s.eq_ignore_ascii_case("end")))
            }).unwrap_or(false);

            if !second_is_number {
                return execute_read_batch(params, default_root);
            }
        }
    }

    let project_path = match resolve_root(params, "project_path", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let raw_file = match params
        .get("path")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("args")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
        }) {
        Some(f) if !f.trim().is_empty() => f.trim(),
        _ => return "ERROR: read requires a filename argument.".to_string(),
    };

    // If path has multiple whitespace-separated tokens (e.g. "file1#L1-10 file2#L20-30"), route to batch
    let tokens: Vec<&str> = raw_file.split_whitespace().collect();
    if tokens.len() > 1 {
        let mut batch_params = params.clone();
        if let Some(obj) = batch_params.as_object_mut() {
            obj.insert("files".to_string(), serde_json::json!(tokens));
        }
        return execute_read_batch(&batch_params, default_root);
    }

    let rel_file = raw_file;

    // Parse potential #Lstart-end or :start-end in rel_file if offset/limit not provided
    let (clean_rel_file, parsed_start, parsed_end) = parse_line_range_spec(rel_file);

    let full_path = match resolve_file_in_workspace(Path::new(&project_path), clean_rel_file, &project_path) {
        Ok(p) => p,
        Err(_) => {
            // If target does not look like a file path (no dot, no slash), or wasn't found as a file,
            // check if it is a symbol name and inspect its definition and call sites!
            if !clean_rel_file.contains('/') && !clean_rel_file.contains('\\') && (!clean_rel_file.contains('.') || clean_rel_file.contains("::")) {
                let symbol_result = inspect_symbol_in_workspace(Path::new(&project_path), clean_rel_file, 5);
                if !symbol_result.is_empty() {
                    return symbol_result;
                }
            }
            return format!("ERROR: File or symbol not found: {clean_rel_file}");
        }
    };

    if !full_path.is_file() {
        if !clean_rel_file.contains('/') && !clean_rel_file.contains('\\') {
            let symbol_result = inspect_symbol_in_workspace(Path::new(&project_path), clean_rel_file, 5);
            if !symbol_result.is_empty() {
                return symbol_result;
            }
        }
        return format!("ERROR: File not found: {clean_rel_file}");
    }

    let content = match fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) => return format!("ERROR: Could not read file {clean_rel_file}: {e}"),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // ── Markdown outline mode ────────────────────────────────────────────────
    // Triggered when: file is .md/.markdown AND no explicit offset arg given.
    let is_markdown = {
        let lower = clean_rel_file.to_ascii_lowercase();
        lower.ends_with(".md") || lower.ends_with(".markdown")
    };
    let has_explicit_offset = params.get("offset").is_some()
        || parsed_start.is_some()
        || params
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| a.len() >= 2)
            .unwrap_or(false);

    if is_markdown && !has_explicit_offset {
        if total_lines == 0 {
            return "(file is empty)".to_string();
        }
        let headings: Vec<String> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.trim_start().starts_with('#'))
            .map(|(idx, l)| format!("{}:{}", idx + 1, l))
            .collect();
        if headings.is_empty() {
            return format!("(markdown outline: no headings found in {clean_rel_file})");
        }
        return format!(
            "[{}:headings/{}]\n{}",
            clean_rel_file.replace('\\', "/"),
            total_lines,
            headings.join("\n")
        );
    }
    // ── End outline mode ─────────────────────────────────────────────────────

    let start_line = int_arg(params, 1, "offset")
        .or(parsed_start)
        .unwrap_or(1)
        .max(1);
    let start_idx = start_line.saturating_sub(1);

    // "END" as third arg or full:true lifts the cap to 500 (still prevents unbounded reads).
    // Default behaviour (no sentinel) keeps 100-line cap to control context size.
    let wants_full = params
        .get("args")
        .and_then(|v| v.as_array())
        .and_then(|a| a.get(2))
        .and_then(|v| v.as_str())
        .map(|s| s.eq_ignore_ascii_case("end"))
        .unwrap_or(false)
        || params
            .get("full")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

    let hard_cap = if wants_full { 500 } else { 100 };
    let raw_second = int_arg(params, 2, "limit")
        .or_else(|| int_arg(params, 2, "end"))
        .or(parsed_end);

    let end_idx = if wants_full {
        (start_idx + hard_cap).min(total_lines)
    } else {
        match raw_second {
            Some(val) if val > start_line => {
                let target_end = val.min(total_lines);
                if target_end > start_idx + hard_cap {
                    start_idx + hard_cap
                } else {
                    target_end
                }
            }
            Some(limit_count) => (start_idx + limit_count.min(hard_cap)).min(total_lines),
            None => (start_idx + 50).min(total_lines),
        }
    };

    if start_idx >= total_lines {
        if total_lines == 0 {
            return "(file is empty)".to_string();
        }
        return format!(
            "(no lines in range offset={start_line} -- file has {total_lines} lines total)"
        );
    }

    let slice_lines: Vec<String> = lines[start_idx..end_idx]
        .iter()
        .enumerate()
        .map(|(idx, line)| format!("{}:{}", start_idx + idx + 1, line))
        .collect();

    format!(
        "[{}:{}-{}/{}]\n{}",
        clean_rel_file.replace('\\', "/"),
        start_idx + 1,
        end_idx,
        total_lines,
        slice_lines.join("\n")
    )
}

/// Helper to parse specs like "AudioEngine.cpp#L295-325", "DeviceHelper.kt:185-225", or "NativeHiResEngine.kt#L175"
pub fn parse_line_range_spec(input: &str) -> (&str, Option<usize>, Option<usize>) {
    // Check for #L or #
    if let Some((file, rest)) = input.split_once('#') {
        let clean_rest = rest.strip_prefix('L').unwrap_or(rest);
        if let Some((s_str, e_str)) = clean_rest.split_once('-') {
            let start = s_str.parse::<usize>().ok();
            let end = e_str.parse::<usize>().ok();
            return (file, start, end);
        } else if let Ok(s) = clean_rest.parse::<usize>() {
            return (file, Some(s), None);
        }
        return (file, None, None);
    }

    // Check for :start-end (e.g. DeviceHelper.kt:185-225)
    // Be careful not to treat Windows drive letters like C:\ as delimiters
    if let Some(colon_pos) = input.rfind(':') {
        if colon_pos > 1 {
            let file = &input[..colon_pos];
            let rest = &input[colon_pos + 1..];
            let clean_rest = rest.strip_prefix('L').unwrap_or(rest);
            if let Some((s_str, e_str)) = clean_rest.split_once('-') {
                if let (Ok(start), Ok(end)) = (s_str.parse::<usize>(), e_str.parse::<usize>()) {
                    return (file, Some(start), Some(end));
                }
            } else if let Ok(s) = clean_rest.parse::<usize>() {
                return (file, Some(s), None);
            }
        }
    }

    (input, None, None)
}

/// Execute batch reading across multiple files / line ranges in a single invocation.
pub fn execute_read_batch(params: &Value, default_root: Option<&str>) -> String {
    let project_path = match resolve_root(params, "project_path", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let mut specs_to_read: Vec<Value> = Vec::new();

    // 1. Direct array of specs { "specs": [ ... ] }
    if let Some(specs_arr) = params.get("specs").and_then(|v| v.as_array()) {
        for s in specs_arr {
            specs_to_read.push(s.clone());
        }
    }

    // 2. String array { "files": [ "file#L1-20", ... ] } or { "targets": [ ... ] }
    if let Some(files_arr) = params.get("files").or_else(|| params.get("targets")).and_then(|v| v.as_array()) {
        for f in files_arr {
            if let Some(s) = f.as_str() {
                specs_to_read.push(serde_json::json!({ "path": s }));
            } else if f.is_object() {
                specs_to_read.push(f.clone());
            }
        }
    }

    // 3. Positional args array (from CLI or orchestra_cmd: "read file1#L1-50 file2#L10-40")
    if let Some(args_arr) = params.get("args").and_then(|v| v.as_array()) {
        for a in args_arr {
            if let Some(s) = a.as_str() {
                for token in s.split_whitespace() {
                    specs_to_read.push(serde_json::json!({ "path": token }));
                }
            }
        }
    }

    if specs_to_read.is_empty() {
        return "ERROR: read requires at least one file or spec (e.g. read AudioEngine.cpp#L1-50 DeviceHelper.kt#L100-150)".to_string();
    }

    let mut results = Vec::new();
    for spec in specs_to_read {
        let mut single_params = spec;
        if let Some(obj) = single_params.as_object_mut() {
            obj.entry("project_path")
                .or_insert_with(|| serde_json::Value::String(project_path.clone()));
        }
        let output = execute_read(&single_params, Some(&project_path));
        results.push(output);
    }

    let separator = format!("\n\n{}\n\n", "─".repeat(60));
    results.join(&separator)
}

fn int_arg(params: &Value, args_index: usize, key: &str) -> Option<usize> {
    if let Some(v) = params.get(key) {
        if let Some(n) = v.as_u64() {
            return Some(n as usize);
        }
        if let Some(s) = v.as_str() {
            return s.parse().ok();
        }
    }
    let arg = params.get("args").and_then(|v| v.as_array())?.get(args_index)?;
    arg.as_u64()
        .map(|n| n as usize)
        .or_else(|| arg.as_str().and_then(|s| s.parse().ok()))
}

pub fn inspect_symbol_in_workspace(project_dir: &Path, symbol: &str, max_callers: usize) -> String {
    if !project_dir.exists() {
        return String::new();
    }

    let adapter = detect_adapter(project_dir);
    let target_sym = symbol.split("::").last().unwrap_or(symbol).trim();

    let caller_re = match Regex::new(&format!(r"\b{}\s*\(", regex::escape(target_sym))) {
        Ok(r) => r,
        Err(_) => return String::new(),
    };

    let mut definition: Option<(String, crate::adapters::SymbolSpan)> = None;
    let mut callers: Vec<String> = Vec::new();

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

    for dent in walker.build().filter_map(|e| e.ok()) {
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let path = dent.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ext == "md" || ext == "txt" || ext == "json" || ext == "xml" {
            continue;
        }

        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };

        let rel_path = path
            .strip_prefix(project_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

        // 1. Try extracting definition if not found yet
        if definition.is_none() {
            if let Some(span) = adapter.extract_definition(&content, &ext, symbol) {
                definition = Some((rel_path.clone(), span));
            }
        }

        // 2. Find call sites
        if callers.len() < max_callers {
            for (idx, line) in content.lines().enumerate() {
                let line_num = idx + 1;
                if caller_re.is_match(line) {
                    if let Some((ref def_path, ref def_span)) = definition {
                        if def_path == &rel_path && line_num >= def_span.start_line && line_num <= def_span.end_line {
                            continue;
                        }
                    }
                    let snippet = line.trim();
                    callers.push(format!("{}:{}: {}", rel_path, line_num, snippet));
                    if callers.len() >= max_callers {
                        break;
                    }
                }
            }
        }

        if definition.is_some() && callers.len() >= max_callers {
            break;
        }
    }

    if definition.is_none() && callers.is_empty() {
        return String::new();
    }

    let mut out = String::new();
    out.push_str(&format!("[SYMBOL DEFINITION: {}]\n", symbol));

    if let Some((def_file, span)) = definition {
        out.push_str(&format!(
            "Location: {}:{}-{} ({} lines)\n{}\n\n",
            def_file,
            span.start_line,
            span.end_line,
            span.end_line - span.start_line + 1,
            span.body
        ));
    } else {
        out.push_str(&format!("No explicit definition block found for '{}'\n\n", symbol));
    }

    if !callers.is_empty() {
        out.push_str(&format!("[REFERENCES / CALL SITES: {} (Top {})]\n", symbol, callers.len()));
        for c in &callers {
            out.push_str(&format!("• {}\n", c));
        }
    } else {
        out.push_str("[REFERENCES / CALL SITES] None found.\n");
    }

    out
}
