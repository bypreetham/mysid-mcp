use serde_json::Value;
use std::fs;
use std::path::Path;

use super::workspace::{resolve_file_in_workspace, resolve_root};
use crate::index::{Index, Located, SymbolId, SymbolKind};
use crate::parsers::{find_parser, is_supported_extension};

/// Lines `mysid read File` returns when no range is given.
const DEFAULT_WINDOW: usize = 120;
/// A source file longer than this is not printed whole by a bare `mysid read File`: the outline and a hint come back.
const OUTLINE_ABOVE: usize = 150;
/// Outline entries shown by that reply.
const OUTLINE_LINES: usize = 90;

/// The reply to a bare `mysid read BigFile.ext`: the outline (what is where, with line numbers) and how to get the code.
fn outline_reply(project_path: &str, shown: &str, total: usize) -> Option<String> {
    let raw = super::overview::execute_overview(
        &serde_json::json!({ "path": shown, "project_path": project_path, "workspace_root": project_path }),
        Some(project_path),
    );
    let text = serde_json::from_str::<Value>(&raw).ok().and_then(|v| v.get("output").and_then(|o| o.as_str()).map(String::from)).unwrap_or(raw);
    let entries: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with("[L")).collect();
    if entries.is_empty() {
        return None;
    }
    let first_line = entries[0].trim_start().trim_start_matches("[L").split(']').next().and_then(|n| n.parse::<usize>().ok()).unwrap_or(1);
    let shown_entries: Vec<&str> = entries.iter().take(OUTLINE_LINES).copied().collect();
    let mut out = format!(
        "[{}: {} lines. Too long to print whole, so this is the outline (line numbers in brackets). Read code with: mysid read {}#L{}-{}]\n{}",
        shown,
        total,
        shown,
        first_line,
        (first_line + 59).min(total),
        shown_entries.join("\n")
    );
    if entries.len() > shown_entries.len() {
        out.push_str(&format!("\n... {} more entries: mysid overview {}", entries.len() - shown_entries.len(), shown));
    }
    out.push_str(&format!("\n[start of the file: mysid read {0}#L1-100   a symbol: mysid read <SymbolName>   search inside: mysid search \"text\" {0}   the file itself, up to 600 lines: mysid read {0} --full]", shown));
    Some(out)
}

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
        .or_else(|| params.get("query"))
        .or_else(|| params.get("file"))
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
    // `--lang kt` / `--lang cpp`: when a name is defined in several languages, pick by language.
    let lang = params.get("lang").and_then(|v| v.as_str());

    // Parse potential #Lstart-end or :start-end in rel_file if offset/limit not provided
    let (clean_rel_file, parsed_start, parsed_end) = parse_line_range_spec(rel_file);

    let full_path = match resolve_file_in_workspace(Path::new(&project_path), clean_rel_file, &project_path) {
        Ok(p) => p,
        Err(resolve_err) => {
            // Not a file: it may be a symbol (`Name`, `Class.method`, `path:Name`) - ask the index.
            let symbol_result = inspect_symbol_in_workspace(Path::new(&project_path), clean_rel_file, 5, lang);
            if !symbol_result.is_empty() {
                return symbol_result;
            }
            // Several files share the name: list them (with the shortest folder that tells them apart), not "not found".
            let message = super::workspace::resolution_message(&resolve_err);
            if message.contains("ambiguous") {
                return format!("ERROR: {}", message);
            }
            return format!("ERROR: File or symbol not found: {clean_rel_file}");
        }
    };

    if !full_path.is_file() {
        if full_path.is_dir() {
            return format!(
                "ERROR: '{0}' is a folder, not a file. Its entries: mysid list {0}   Where the code is: mysid graph {0}",
                clean_rel_file
            );
        }
        let symbol_result = inspect_symbol_in_workspace(Path::new(&project_path), clean_rel_file, 5, lang);
        if !symbol_result.is_empty() {
            return symbol_result;
        }
        return format!("ERROR: File not found: {clean_rel_file}");
    }

    let content = match fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) => return format!("ERROR: Could not read file {clean_rel_file}: {e}"),
    };

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    // `read` always returns code. The outline (types and signatures) is `mysid overview`.
    let ext = full_path.extension().and_then(|s| s.to_str()).unwrap_or("");

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

    let hard_cap = if wants_full { 600 } else { 200 };
    let raw_second = int_arg(params, 2, "limit")
        .or_else(|| int_arg(params, 2, "end"))
        .or(parsed_end);

    // A bare read of a long source file: the outline and a hint instead of its first lines.
    let bare = parsed_start.is_none()
        && parsed_end.is_none()
        && raw_second.is_none()
        && int_arg(params, 1, "offset").is_none()
        && !wants_full;
    if bare && total_lines > OUTLINE_ABOVE && is_supported_extension(ext) {
        if let Some(reply) = outline_reply(&project_path, &clean_rel_file.replace('\\', "/"), total_lines) {
            return reply;
        }
    }
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
            None if bare && total_lines <= OUTLINE_ABOVE => total_lines,
            None => (start_idx + DEFAULT_WINDOW).min(total_lines),
        }
    };

    if start_idx >= total_lines {
        if total_lines == 0 {
            let (bytes, age) = fs::metadata(&full_path)
                .ok()
                .map(|m| {
                    let age = m.modified().ok().and_then(|t| t.elapsed().ok()).map_or(0, |d| d.as_secs());
                    (m.len(), age)
                })
                .unwrap_or((0, 0));
            return format!("(file is empty: {} bytes, last modified {}s ago - it may still be being written)", bytes, age);
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

    let shown = clean_rel_file.replace('\\', "/");
    let mut out = format!("[{}:{}-{}/{}]\n{}", shown, start_idx + 1, end_idx, total_lines, slice_lines.join("\n"));
    // No end was asked for and the file goes on: say how to get the rest.
    if raw_second.is_none() && !wants_full && end_idx < total_lines {
        let more = format!("mysid read {}#L{}-{}", shown, end_idx + 1, (end_idx + DEFAULT_WINDOW).min(total_lines));
        let outline = if is_supported_extension(ext) { format!("; signatures only: mysid overview {}", shown) } else { String::new() };
        out.push_str(&format!("\n[showing {}-{} of {}. More: {}{}]", start_idx + 1, end_idx, total_lines, more, outline));
    }
    out
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

/// Longest definition printed; the rest is pointed to with a ready-made range.
const MAX_DEFINITION_LINES: usize = 150;
const MAX_LISTED: usize = 8;

/// `--lang` takes an extension (`kt`, `h`) or a language name (`cpp`, `kotlin`): a file matches when it
/// has that extension or is read by the same parser, so `cpp` also selects `.h` and `.cc` files.
fn matches_lang(path: &str, wanted: &str) -> bool {
    let wanted = wanted.trim().trim_start_matches('.').to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if ext == wanted {
        return true;
    }
    match (find_parser(&ext), find_parser(&wanted)) {
        (Some(file), Some(want)) => file.language_name() == want.language_name(),
        (Some(file), None) => file.language_name().to_ascii_lowercase().contains(&wanted),
        _ => false,
    }
}

/// Class-like symbols first, then free functions, then methods.
fn kind_rank(l: &Located) -> u8 {
    match l.symbol.kind {
        SymbolKind::Class => 0,
        SymbolKind::Function => 1,
        SymbolKind::Method => 2,
    }
}

fn describe(l: &Located) -> String {
    let ext = l.path.rsplit('.').next().unwrap_or("");
    let name = match &l.symbol.owner {
        Some(owner) => format!("{}.{}", owner, l.symbol.name),
        None => l.symbol.name.clone(),
    };
    format!("{}:{}  {} [{}, {}]", l.path, l.symbol.start_line, name, l.symbol.detail, ext)
}

/// Definition and callers of a symbol, from the code index (the same lookup `graph` uses):
/// exact names, `Class.method`, `path:Name`. Classes win over functions over methods; if several
/// symbols tie - the same name in two languages, or two modules - they are listed instead of one
/// being picked arbitrarily. `lang` (a file extension) narrows the choice.
pub fn inspect_symbol_in_workspace(project_dir: &Path, symbol: &str, max_callers: usize, lang: Option<&str>) -> String {
    if !project_dir.exists() {
        return String::new();
    }
    let index = Index::open(project_dir);
    let mut found: Vec<Located> = index.candidates(symbol).unwrap_or_default();
    if let Some(l) = lang {
        found.retain(|c| matches_lang(&c.path, l));
    }
    if found.is_empty() {
        return String::new();
    }
    found.sort_by(|a, b| (kind_rank(a), &a.path, a.symbol.start_line).cmp(&(kind_rank(b), &b.path, b.symbol.start_line)));

    let top = kind_rank(&found[0]);
    let tied = found.iter().filter(|c| kind_rank(c) == top).count();
    if tied > 1 {
        let mut out = format!("'{}' matches {} symbols - pick one:\n", symbol, tied);
        for c in found.iter().take(MAX_LISTED) {
            out.push_str(&format!("  {}\n", describe(c)));
        }
        if found.len() > MAX_LISTED {
            out.push_str(&format!("  +{} more\n", found.len() - MAX_LISTED));
        }
        out.push_str(&format!("repeat as: mysid read <path>:{}   or   mysid read {} --lang <ext>", symbol, symbol));
        return out;
    }

    let best = &found[0];
    let mut out = format!("[SYMBOL DEFINITION: {}]\n", symbol);
    out.push_str(&definition_text(project_dir, best));

    // Callers: of the symbol, or of any of a class's methods.
    let mut seeds: Vec<SymbolId> = vec![best.symbol.id];
    if best.symbol.kind == SymbolKind::Class {
        seeds.extend(index.members(best).unwrap_or_default().iter().map(|m| m.symbol.id));
    }
    let callers: Vec<Located> = match index.neighborhood(&seeds, 1) {
        Ok(hood) => {
            let mut callers: Vec<Located> = hood
                .symbols
                .into_iter()
                .filter(|s| !seeds.contains(&s.symbol.id) && hood.edges.iter().any(|e| e.from == s.symbol.id && seeds.contains(&e.to)))
                .collect();
            callers.sort_by(|a, b| (&a.path, a.symbol.start_line).cmp(&(&b.path, b.symbol.start_line)));
            callers
        }
        Err(_) => Vec::new(),
    };
    if callers.is_empty() {
        out.push_str("\n[CALLERS] none found in the index.\n");
    } else {
        out.push_str(&format!("\n[CALLERS: {}]\n", callers.len()));
        for c in callers.iter().take(max_callers) {
            out.push_str(&format!("  {}\n", describe(c)));
        }
        if callers.len() > max_callers {
            out.push_str(&format!("  +{} more (mysid graph {})\n", callers.len() - max_callers, symbol));
        }
    }

    if found.len() > 1 {
        out.push_str(&format!("\n[OTHER MATCHES: {}]\n", found.len() - 1));
        for c in found.iter().skip(1).take(MAX_LISTED) {
            out.push_str(&format!("  {}\n", describe(c)));
        }
    }
    out
}

/// `Location: path:start-end (N lines)` and the source of the symbol, capped.
fn definition_text(project_dir: &Path, l: &Located) -> String {
    let start = l.symbol.start_line as usize;
    let end = (l.symbol.end_line as usize).max(start);
    let Ok(content) = fs::read_to_string(project_dir.join(&l.path)) else {
        return format!("Location: {}:{}-{} (file could not be read)\n", l.path, start, end);
    };
    let lines: Vec<&str> = content.lines().collect();
    let end = end.min(lines.len().max(start));
    let shown_end = end.min(start + MAX_DEFINITION_LINES - 1);

    let mut out = format!("Location: {}:{}-{} ({} lines)\n", l.path, start, end, end - start + 1);
    for (i, line) in lines.iter().enumerate().take(shown_end).skip(start - 1) {
        out.push_str(&format!("{}:{}\n", i + 1, line));
    }
    if shown_end < end {
        out.push_str(&format!("... +{} more lines: mysid read {}#L{}-{}\n", end - shown_end, l.path, shown_end + 1, end));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_returns_code_with_a_window_and_a_footer_and_sends_folders_to_list() {
        let dir = std::env::temp_dir().join(format!("mysid_read_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        let body: String = (1..=300).map(|i| format!("fun f{}() {{}}\n", i)).collect();
        fs::write(dir.join("src/Big.kt"), &body).unwrap();
        fs::write(dir.join("src/Small.kt"), "class S\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let read = |q: &str| execute_read(&json!({ "workspace_root": root, "project_path": root, "path": q }), Some(&root));

        // A bare read of a long source file is the outline plus a hint, not its first lines.
        let big = read("Big.kt");
        assert!(big.starts_with("[Big.kt: 300 lines. Too long to print whole"), "{}", &big[..big.len().min(120)]);
        assert!(big.contains("mysid read Big.kt#L1-60") && big.contains("[L1]") && big.contains("mysid read Big.kt#L1-100"), "{}", big);
        assert!(!big.contains("1:fun f1() {}"), "no code in the outline reply: {}", big);
        // Asking for a range (or --full) returns code.
        assert!(read("Big.kt#L1-100").starts_with("[Big.kt:1-100/300]\n1:fun f1() {}"));
        let full = execute_read(&json!({ "workspace_root": root, "project_path": root, "path": "Big.kt", "full": true }), Some(&root));
        assert!(full.starts_with("[Big.kt:1-300/300]") || full.starts_with("[Big.kt:1-"), "{}", &full[..60]);
        // A text file the parsers do not know keeps the plain window and its footer.
        fs::write(dir.join("src/notes.log"), (1..=300).map(|i| format!("line {}\n", i)).collect::<String>()).unwrap();
        let log = read("notes.log");
        assert!(log.starts_with("[notes.log:1-120/300]") && log.contains("[showing 1-120 of 300. More: mysid read notes.log#L121-240"), "{}", log);

        let slice = read("Big.kt#L10-12");
        assert_eq!(slice, "[Big.kt:10-12/300]\n10:fun f10() {}\n11:fun f11() {}\n12:fun f12() {}");
        assert!(!read("Small.kt").contains("showing"), "a short file needs no footer");

        // A slice is up to 200 lines; a name shared by two files lists them instead of "not found".
        let wide = read("Big.kt#L1-300");
        assert!(wide.starts_with("[Big.kt:1-200/300]") && wide.contains("200:fun f200() {}") && !wide.contains("201:fun"), "{}", &wide[..60]);
        fs::create_dir_all(dir.join("a")).unwrap();
        fs::create_dir_all(dir.join("b")).unwrap();
        fs::write(dir.join("a/types.ts"), "export type A = 1\n").unwrap();
        fs::write(dir.join("b/types.ts"), "export type B = 2\n").unwrap();
        let dup = read("types.ts");
        assert!(dup.starts_with("ERROR: 'types.ts' is ambiguous - 2 files") && dup.contains("a/types.ts") && dup.contains("b/types.ts"), "{}", dup);
        // In a batch only that name fails; the others are still read.
        let batch = execute_read_batch(&json!({ "workspace_root": root, "project_path": root, "files": ["types.ts", "Small.kt"] }), Some(&root));
        assert!(batch.contains("ambiguous") && batch.contains("[Small.kt:1-1/1]"), "{}", batch);

        let folder = read("src");
        assert!(folder.starts_with("ERROR: 'src' is a folder, not a file") && folder.contains("mysid list src"), "{}", folder);
        let _ = fs::remove_dir_all(&dir);
    }
}
