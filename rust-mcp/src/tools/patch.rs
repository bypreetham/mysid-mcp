use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::{resolve_file_in_workspace, resolve_root};

/// Changes / lines per side a dry-run diff prints before summarizing the rest.
const DIFF_CHANGES_SHOWN: usize = 3;
const DIFF_LINES_PER_SIDE: usize = 12;
/// Line numbers listed in a result or an ambiguity error.
const LINES_LISTED: usize = 8;

fn get_param(params: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()))
}

fn flag(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false))
}

/// Every `patch` failure starts with the same words, so a caller cannot mistake it for success.
/// Character pairs of a line without whitespace or case: the cheap similarity measure for "closest line".
fn bigrams(s: &str) -> Vec<(char, char)> {
    let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).flat_map(|c| c.to_lowercase()).collect();
    chars.windows(2).map(|w| (w[0], w[1])).collect()
}

/// Dice coefficient of two bigram multisets: 1.0 identical, 0.0 nothing in common.
fn dice(a: &[(char, char)], b: &[(char, char)]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<(char, char), i32> = std::collections::HashMap::new();
    for p in a {
        *counts.entry(*p).or_insert(0) += 1;
    }
    let mut common = 0;
    for p in b {
        if let Some(c) = counts.get_mut(p) {
            if *c > 0 {
                *c -= 1;
                common += 1;
            }
        }
    }
    2.0 * common as f64 / (a.len() + b.len()) as f64
}

fn clip_line(s: &str) -> String {
    let t: String = s.chars().take(100).collect();
    if s.chars().count() > 100 { format!("{}...", t) } else { t }
}

/// Why a block may not have matched, for the "not found" answers of `patch` and `apply`: the closest line of the
/// file (with where it first differs), mixed line endings, and the reminder that a shell can alter inline text.
pub(crate) fn not_found_hint(content: &str, old: &str) -> String {
    let mut notes: Vec<String> = Vec::new();
    let crlf = content.matches("\r\n").count();
    let bare = content.matches('\n').count().saturating_sub(crlf);
    if crlf > 0 && bare > 0 {
        notes.push(format!(
            "this file has MIXED line endings ({} CRLF lines, {} LF lines): a block that spans both kinds cannot match as written; patch the lines of one kind at a time",
            crlf, bare
        ));
    }
    let probe = old.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if probe.chars().count() >= 4 {
        let pb = bigrams(probe);
        let plen = probe.chars().count() as f64;
        let mut best: (f64, usize, &str) = (0.0, 0, "");
        for (i, line) in content.lines().enumerate() {
            let t = line.trim();
            let tl = t.chars().count() as f64;
            if t.is_empty() || tl < plen * 0.5 || tl > plen * 2.0 {
                continue;
            }
            let d = dice(&pb, &bigrams(t));
            if d > best.0 {
                best = (d, i + 1, t);
            }
        }
        if best.0 >= 0.6 {
            let col = probe.chars().zip(best.2.chars()).position(|(a, b)| a != b).unwrap_or(probe.chars().count().min(best.2.chars().count()));
            notes.push(format!("closest line {}: `{}` (searched for `{}`; they first differ at column {})", best.1, clip_line(best.2), clip_line(probe), col + 1));
        } else {
            notes.push("no line of the file resembles the first line of the searched text: it may belong to another file, or the file changed (mysid read it again)".to_string());
        }
    }
    notes.push("text typed on a command line can be altered by the shell (quotes, backslashes, %, $): --old-file/--new-file, --stdin or mysid apply send it untouched".to_string());
    notes.join("\n")
}

fn fail(error: impl Into<String>) -> String {
    json!({ "success": false, "error": format!("NOT PATCHED - {}", error.into()) }).to_string()
}

/// Where and how `old_string` matched, and the text that replaces each match.
pub(crate) struct Plan {
    pub strategy: &'static str,
    pub ranges: Vec<(usize, usize)>,
    pub replacement: String,
}

/// True when every line break in `content` is CRLF (and there is at least one).
fn is_crlf_file(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut crlf = 0usize;
    let mut bare_lf = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            if i > 0 && bytes[i - 1] == b'\r' { crlf += 1 } else { bare_lf += 1 }
        }
    }
    crlf > 0 && bare_lf == 0
}

fn with_endings(text: &str, crlf: bool) -> String {
    let unix = text.replace("\r\n", "\n");
    if crlf { unix.replace('\n', "\r\n") } else { unix }
}

pub(crate) fn line_of(content: &str, offset: usize) -> usize {
    content.as_bytes()[..offset].iter().filter(|&&b| b == b'\n').count() + 1
}

/// Finds every place `old` occurs, trying the strictest strategy first:
/// exact text, then the same text ignoring CRLF/LF differences, then ignoring surrounding whitespace.
/// The first strategy that matches anywhere decides; later ones are never mixed in.
pub(crate) fn plan(content: &str, old: &str, new: &str) -> Option<Plan> {
    let crlf_file = is_crlf_file(content);

    let exact: Vec<(usize, usize)> = content.match_indices(old).map(|(i, m)| (i, i + m.len())).collect();
    if !exact.is_empty() {
        return Some(Plan { strategy: "exact_match", ranges: exact, replacement: with_endings(new, crlf_file) });
    }

    // Line-ending tolerant: match the old text line by line with `\r?\n` between lines, directly in
    // the original content, so every byte outside the match (including mixed endings) is untouched.
    if old.contains('\n') {
        let pattern = old.replace("\r\n", "\n").split('\n').map(regex::escape).collect::<Vec<_>>().join(r"\r?\n");
        if let Ok(re) = Regex::new(&pattern) {
            let ranges: Vec<(usize, usize)> = re.find_iter(content).map(|m| (m.start(), m.end())).collect();
            if let Some(&(s, e)) = ranges.first() {
                let matched_crlf = content[s..e].contains("\r\n");
                return Some(Plan { strategy: "crlf_normalized", ranges, replacement: with_endings(new, matched_crlf) });
            }
        }
    }

    let trimmed = old.trim();
    if !trimmed.is_empty() {
        let ranges: Vec<(usize, usize)> = content.match_indices(trimmed).map(|(i, m)| (i, i + m.len())).collect();
        if !ranges.is_empty() {
            return Some(Plan { strategy: "trimmed_match", ranges, replacement: with_endings(new.trim(), crlf_file) });
        }
    }
    None
}

/// Splices `replacement` into the original text at the chosen ranges.
pub(crate) fn apply(content: &str, ranges: &[(usize, usize)], replacement: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut cursor = 0;
    for &(s, e) in ranges {
        out.push_str(&content[cursor..s]);
        out.push_str(replacement);
        cursor = e;
    }
    out.push_str(&content[cursor..]);
    out
}

fn push_side(out: &mut Vec<String>, sign: char, text: &str) {
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    for l in lines.iter().take(DIFF_LINES_PER_SIDE) {
        out.push(format!("{} {}", sign, l));
    }
    if lines.len() > DIFF_LINES_PER_SIDE {
        out.push(format!("{} ... +{} more line(s)", sign, lines.len() - DIFF_LINES_PER_SIDE));
    }
}

/// Changed lines before/after, as the file would look; nothing is written.
pub(crate) fn diff_preview(content: &str, ranges: &[(usize, usize)], replacement: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (shown, &(s, e)) in ranges.iter().enumerate() {
        if shown >= DIFF_CHANGES_SHOWN {
            out.push(format!("... +{} more change(s)", ranges.len() - shown));
            break;
        }
        let line_start = content[..s].rfind('\n').map_or(0, |p| p + 1);
        let line_end = content[e..].find('\n').map_or(content.len(), |p| e + p);
        let before = &content[line_start..line_end];
        let after = format!("{}{}{}", &content[line_start..s], replacement, &content[e..line_end]);
        out.push(format!("@@ line {}", line_of(content, s)));
        push_side(&mut out, '-', before);
        push_side(&mut out, '+', &after);
    }
    out
}

pub(crate) fn list_lines(lines: &[usize]) -> String {
    let shown: Vec<String> = lines.iter().take(LINES_LISTED).map(|l| l.to_string()).collect();
    let more = lines.len().saturating_sub(LINES_LISTED);
    if more > 0 { format!("{}, +{} more", shown.join(", "), more) } else { shown.join(", ") }
}

pub fn execute_patch(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    // Accept "path", "file", or "filepath" — "file" is the most common mistake
    let path = match ["path", "file", "filepath"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()))
    {
        Some(p) => p.to_string(),
        None => return fail("Missing file path. Use 'path', 'file', or 'filepath' key."),
    };
    let old_string = match get_param(params, &["old_string", "search", "find", "target", "from"]) {
        Some(s) => s,
        None => return fail("Missing `old_string` (or search/find/target/from) parameter."),
    };
    let new_string = match get_param(params, &["new_string", "replace", "replacement", "with", "to", "content"]) {
        Some(s) => s,
        None => return fail("Missing `new_string` (or replace/replacement/with/to/content) parameter."),
    };
    let replace_all = flag(params, &["replace_all", "all"]);
    let dry_run = flag(params, &["dry_run", "dry-run", "dryrun"]);
    let show = flag(params, &["show"]);

    if old_string.is_empty() {
        return fail("`old_string` is empty; it would match everywhere.");
    }
    if old_string == new_string {
        return fail("`old_string` and `new_string` are identical; nothing to change.");
    }

    let root = Path::new(&workspace_root);
    let full_path = match resolve_file_in_workspace(root, &path, &workspace_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let content = match fs::read_to_string(&full_path) {
        Ok(c) => c,
        Err(e) => return fail(format!("Failed to read file: {}", e)),
    };

    let Some(plan) = plan(&content, &old_string, &new_string) else {
        return fail(format!("old_string block could not be found in target file.\n{}", not_found_hint(&content, &old_string)));
    };

    let lines: Vec<usize> = plan.ranges.iter().map(|&(s, _)| line_of(&content, s)).collect();
    if plan.ranges.len() > 1 && !replace_all {
        return json!({
            "success": false,
            "matches": plan.ranges.len(),
            "lines": lines,
            "error": format!(
                "NOT PATCHED - old_string matches {} places in {} (lines {}). Nothing was changed: add surrounding context to make it unique, or pass replace_all.",
                plan.ranges.len(), path, list_lines(&lines)
            )
        })
        .to_string();
    }

    let chosen: &[(usize, usize)] = if replace_all { &plan.ranges } else { &plan.ranges[..1] };
    let chosen_lines: Vec<usize> = lines.iter().take(chosen.len()).copied().collect();

    if dry_run {
        let mut out = vec![format!(
            "dry-run {} ({}): {} change(s) at line {}, nothing written",
            path, plan.strategy, chosen.len(), list_lines(&chosen_lines)
        )];
        out.extend(diff_preview(&content, chosen, &plan.replacement));
        return json!({
            "success": true,
            "dry_run": true,
            "path": path,
            "strategy": plan.strategy,
            "changes": chosen.len(),
            "lines": chosen_lines,
            "output": out.join("\n")
        })
        .to_string();
    }

    let new_content = apply(&content, chosen, &plan.replacement);
    match fs::write(&full_path, new_content.as_bytes()) {
        Ok(_) => {
            let mut text = format!("patched {} ({}): {} change(s) at line {}", path, plan.strategy, chosen.len(), list_lines(&chosen_lines));
            // `--show`: the removed (-) and added (+) lines, so the file need not be read again.
            if show {
                text.push('\n');
                text.push_str(&diff_preview(&content, chosen, &plan.replacement).join("\n"));
            }
            json!({
                "success": true,
                "path": path,
                "strategy": plan.strategy,
                "changes": chosen.len(),
                "lines": chosen_lines,
                "output": text
            })
            .to_string()
        }
        Err(e) => fail(format!("Failed to write modified file: {}", e)),
    }
}

/// Apply an ordered list of patches to one or more files in a single call.
/// Params: { "patches": [ { "path": "...", "old_string": "...", "new_string": "...", "replace_all"?: bool }, ... ],
///           "dry_run"?: bool }
/// Stops at the first failed patch and returns which index failed. With `dry_run` nothing is written.
pub fn execute_patch_batch(params: &Value, default_root: Option<&str>) -> String {
    // One file, several snippets (`path` + `text` of SEARCH/REPLACE blocks): all or nothing, through apply.
    if params.get("patches").is_none() {
        if let (Some(path), Some(_)) = (get_param(params, &["path", "file"]), get_param(params, &["text", "edits"])) {
            let mut p = params.clone();
            if let Some(o) = p.as_object_mut() {
                o.insert("to".to_string(), json!(path));
            }
            return super::apply::execute_apply(&p, default_root);
        }
    }
    let patches = match params.get("patches").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => return fail("Missing `patches` array, or `path` + `text` (SEARCH/REPLACE blocks for one file)."),
    };
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let dry_run = flag(params, &["dry_run", "dry-run", "dryrun"]);
    let mut results = Vec::new();
    let mut outputs: Vec<String> = Vec::new();
    for (i, patch) in patches.iter().enumerate() {
        let mut merged = patch.clone();
        if let Some(obj) = merged.as_object_mut() {
            obj.entry("workspace_root")
                .or_insert_with(|| serde_json::Value::String(workspace_root.clone()));
            if dry_run {
                obj.insert("dry_run".to_string(), json!(true));
            }
        }
        let result_str = execute_patch(&merged, Some(&workspace_root));
        let result_val: Value = serde_json::from_str(&result_str)
            .unwrap_or_else(|_| json!({ "success": false, "error": result_str }));
        let success = result_val.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        if let Some(text) = result_val.get("output").and_then(|v| v.as_str()) {
            outputs.push(text.to_string());
        }
        results.push(json!({
            "index": i,
            "path": patch.get("path").and_then(|v| v.as_str()).unwrap_or("?"),
            "success": success,
            "strategy": result_val.get("strategy"),
            "error": result_val.get("error"),
            "lines": result_val.get("lines"),
        }));
        if !success {
            return json!({
                "success": false,
                "applied": i,
                "failed_at": i,
                "results": results,
            }).to_string();
        }
    }
    json!({
        "success": true,
        "applied": results.len(),
        "dry_run": dry_run,
        "results": results,
        "output": outputs.join("\n")
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, file: &str, content: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("mysid-patch-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file), content.as_bytes()).unwrap();
        let root = dir.to_string_lossy().to_string();
        (dir, root)
    }

    fn run(root: &str, old: &str, new: &str, extra: Value) -> Value {
        let mut p = json!({ "workspace_root": root, "path": "a.txt", "old_string": old, "new_string": new });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        serde_json::from_str(&execute_patch(&p, Some(root))).unwrap()
    }

    #[test]
    fn unique_match_is_replaced_and_everything_else_is_untouched() {
        let (dir, root) = project("unique", "a.txt", "alpha\r\nversionCode = 44\r\nomega\r\n");
        let r = run(&root, "versionCode = 44", "versionCode = 45", json!({}));
        assert_eq!(r["success"], true);
        assert_eq!(r["strategy"], "exact_match");
        assert_eq!(r["lines"], json!([2]));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "alpha\r\nversionCode = 45\r\nomega\r\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguous_match_is_refused_with_count_and_lines_and_changes_nothing() {
        let original = "x = 1\nfoo()\nx = 1\nbar()\nx = 1\n";
        let (dir, root) = project("ambiguous", "a.txt", original);
        let r = run(&root, "x = 1", "x = 2", json!({}));
        assert_eq!(r["success"], false);
        assert_eq!(r["matches"], 3);
        assert_eq!(r["lines"], json!([1, 3, 5]));
        assert!(r["error"].as_str().unwrap().contains("matches 3 places"), "{}", r["error"]);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), original, "file untouched");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_match_names_the_closest_line_and_mixed_line_endings() {
        let content = "int main() {\r\n    return compute(1, 2);\r\n}\n#define OBJS \"a.o b.o\"\n";
        // A near miss (the shell ate a quote): the closest line and the column where they differ.
        let hint = not_found_hint(content, "    return compute(1, 3);\n");
        assert!(hint.contains("closest line 2:") && hint.contains("first differ at column"), "{}", hint);
        assert!(hint.contains("MIXED line endings (2 CRLF lines, 2 LF lines)"), "{}", hint);
        assert!(hint.contains("--old-file"), "{}", hint);
        // Nothing alike: says so instead of guessing.
        let none = not_found_hint("alpha\nbeta\n", "completely different statement here");
        assert!(none.contains("no line of the file resembles"), "{}", none);
        // The patch answer itself carries the hint.
        let (dir, root) = project("hint", "a.txt", content);
        let r = run(&root, "return compute(9, 9);", "x", json!({}));
        assert!(r["error"].as_str().unwrap().starts_with("NOT PATCHED - old_string block could not be found") && r["error"].as_str().unwrap().contains("closest line 2"), "{}", r["error"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_all_changes_every_match() {
        let (dir, root) = project("all", "a.txt", "x = 1\ny\nx = 1\n");
        let r = run(&root, "x = 1", "x = 2", json!({ "replace_all": true }));
        assert_eq!((r["success"].clone(), r["changes"].clone()), (json!(true), json!(2)));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "x = 2\ny\nx = 2\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_shows_a_diff_and_writes_nothing() {
        let original = "one\nversionCode = 44\nthree\n";
        let (dir, root) = project("dry", "a.txt", original);
        let r = run(&root, "versionCode = 44", "versionCode = 45", json!({ "dry_run": true }));
        assert_eq!(r["success"], true);
        let out = r["output"].as_str().unwrap();
        assert!(out.starts_with("dry-run a.txt (exact_match): 1 change(s) at line 2, nothing written"), "{}", out);
        assert!(out.contains("@@ line 2") && out.contains("- versionCode = 44") && out.contains("+ versionCode = 45"), "{}", out);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), original);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn crlf_normalized_matches_lf_text_and_keeps_other_lines_byte_exact() {
        // CRLF file with one stray LF-only line elsewhere: that line must stay LF.
        let original = "head\r\nfun a() {\r\n    x()\r\n}\r\nstray\ntail\r\n";
        let (dir, root) = project("crlf", "a.txt", original);
        let r = run(&root, "fun a() {\n    x()\n}", "fun a() {\n    y()\n}", json!({}));
        assert_eq!(r["success"], true);
        assert_eq!(r["strategy"], "crlf_normalized");
        assert_eq!(
            fs::read_to_string(dir.join("a.txt")).unwrap(),
            "head\r\nfun a() {\r\n    y()\r\n}\r\nstray\ntail\r\n"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn single_line_edit_inserting_lines_follows_the_files_line_endings() {
        let (dir, root) = project("endings", "a.txt", "a\r\nb\r\n");
        let r = run(&root, "b", "b1\nb2", json!({}));
        assert_eq!(r["success"], true);
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "a\r\nb1\r\nb2\r\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn trimmed_match_still_works_and_empty_or_identical_input_is_rejected() {
        let (dir, root) = project("trim", "a.txt", "keep value here\n");
        let r = run(&root, "  value  ", "VALUE", json!({}));
        assert_eq!((r["success"].clone(), r["strategy"].clone()), (json!(true), json!("trimmed_match")));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), "keep VALUE here\n");
        assert_eq!(run(&root, "", "x", json!({}))["success"], false);
        assert_eq!(run(&root, "VALUE", "VALUE", json!({}))["success"], false);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn batch_dry_run_writes_nothing() {
        let original = "one\ntwo\n";
        let (dir, root) = project("batch", "a.txt", original);
        let p = json!({ "workspace_root": root, "dry_run": true, "patches": [
            { "path": "a.txt", "old_string": "one", "new_string": "1" },
            { "path": "a.txt", "old_string": "two", "new_string": "2" } ] });
        let r: Value = serde_json::from_str(&execute_patch_batch(&p, Some(&root))).unwrap();
        assert_eq!((r["success"].clone(), r["applied"].clone(), r["dry_run"].clone()), (json!(true), json!(2), json!(true)));
        assert_eq!(fs::read_to_string(dir.join("a.txt")).unwrap(), original);
        let _ = fs::remove_dir_all(&dir);
    }
}
