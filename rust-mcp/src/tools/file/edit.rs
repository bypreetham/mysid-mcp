//! `mysid edit <file>#L10-20`: change a file by line numbers (replace, insert after / before, delete).

use serde_json::Value;
use std::fs;
use std::path::Path;

use super::util::*;
use crate::tools::workspace::{resolve_file_in_workspace, resolve_root};

/// `file.kt#L10-20`, `file.kt#L10` -> (file, first, last). A plain name has no range.
fn split_range(spec: &str) -> (String, Option<(usize, usize)>) {
    let Some((file, tail)) = spec.rsplit_once('#') else { return (spec.to_string(), None) };
    let t = tail.trim().trim_start_matches(|c| c == 'L' || c == 'l');
    let (a, b) = match t.split_once('-') {
        Some((a, b)) => (a, b.trim_start_matches(|c| c == 'L' || c == 'l')),
        None => (t, t),
    };
    match (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
        (Ok(a), Ok(b)) if a >= 1 && b >= a => (file.to_string(), Some((a, b))),
        _ => (spec.to_string(), None),
    }
}

fn clipped(line: &str) -> String {
    let t = line.trim_end_matches(|c| c == '\r' || c == '\n');
    if t.chars().count() > 110 { format!("{}...", t.chars().take(110).collect::<String>()) } else { t.to_string() }
}

/// `mysid edit file#L10-20 "new text"`: replace lines 10 to 20 by number. `--insert-after` / `--insert-before` add the text
/// next to the line instead, `--delete` removes the lines, `--expect "text"` refuses unless the first line named contains it
/// (the guard against line numbers that moved since the file was read). All or nothing; the file's line endings are kept.
pub fn execute_edit(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(spec) = path_of(params) else {
        return out(false, "NOT EDITED - say which lines: mysid edit src/Foo.kt#L10-20 --file new.txt   (or --insert-after / --insert-before / --delete)");
    };
    let (name, from_spec) = split_range(&spec);
    let range = from_spec.or_else(|| {
        let lines = params.get("lines").and_then(|v| v.as_str()).map(String::from)?;
        split_range(&format!("x#L{}", lines.trim_start_matches(|c| c == 'L' || c == 'l'))).1
    });
    let range = range.or_else(|| {
        let start = params.get("start").or_else(|| params.get("line")).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))? as usize;
        let end = params.get("end").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map_or(start, |e| e as usize);
        (start >= 1 && end >= start).then_some((start, end))
    });
    let Some((first, last)) = range else {
        return out(false, "NOT EDITED - no line range. Name it as file#L10-20 (or file#L10 for one line), the way mysid read shows it.");
    };
    let mode = if flag(params, &["delete"]) {
        "delete"
    } else if flag(params, &["insert_after", "insert-after", "after"]) {
        "after"
    } else if flag(params, &["insert_before", "insert-before", "before"]) {
        "before"
    } else {
        "replace"
    };
    let content_new = content_of(params);
    if mode != "delete" && content_new.is_none() {
        return out(false, "NOT EDITED - no text received. Pass it as the last argument, pipe it in, or use --file new.txt (or --delete to remove the lines).");
    }
    let root = Path::new(&workspace_root);
    let full = match resolve_file_in_workspace(root, &name, &workspace_root) {
        Ok(p) => p,
        Err(e) => {
            let reason = serde_json::from_str::<Value>(&e).ok().and_then(|v| v.get("error").and_then(|x| x.as_str()).map(String::from)).unwrap_or(e);
            return out(false, &format!("NOT EDITED - {}", reason));
        }
    };
    let shown = full.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| name.clone());
    let existing = match fs::read_to_string(&full) {
        Ok(c) => c,
        Err(e) => return out(false, &format!("NOT EDITED - cannot read {}: {}", shown, e)),
    };
    let crlf = is_crlf(&existing);
    let eol = if crlf { "\r\n" } else { "\n" };
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let total = lines.len();
    let insert = mode == "after" || mode == "before";
    // Inserting after the last line, or before the first, is fine; replacing and deleting need real lines.
    if first > total || (!insert && last > total) {
        return out(false, &format!("NOT EDITED - {} has {} line(s); lines {}-{} are outside it. Read it again: mysid read {}#L1-60", shown, total, first, last, shown));
    }
    let target_last = if insert { first } else { last };
    if let Some(want) = params.get("expect").and_then(|v| v.as_str()).filter(|s| !s.is_empty()) {
        if !lines[first - 1].contains(want) {
            return out(
                false,
                &format!("NOT EDITED - line {} of {} is `{}`, which does not contain `{}`. The lines moved since you read the file: read it again (mysid read {}#L{}-{}).", first, shown, clipped(lines[first - 1]), want, shown, first.saturating_sub(5).max(1), (first + 15).min(total)),
            );
        }
    }
    // New text as whole lines with the file's own line ending.
    let new_block: String = match &content_new {
        Some(t) if mode != "delete" => {
            let body = t.replace("\r\n", "\n");
            let body = body.strip_suffix('\n').unwrap_or(&body);
            let mut b = body.split('\n').collect::<Vec<_>>().join(eol);
            b.push_str(eol);
            b
        }
        _ => String::new(),
    };
    let removed: Vec<&str> = if insert { Vec::new() } else { lines[first - 1..last].to_vec() };
    // A last line without a line ending stays without one when it is the one replaced.
    let had_no_eol = !insert && last == total && !lines[total - 1].ends_with('\n');
    let mut out_text = String::new();
    out_text.push_str(&lines[..first - 1].concat());
    match mode {
        "before" => {
            out_text.push_str(&new_block);
            out_text.push_str(&lines[first - 1..].concat());
        }
        "after" => {
            out_text.push_str(&lines[first - 1..target_last].concat());
            if !lines[target_last - 1].ends_with('\n') {
                out_text.push_str(eol);
            }
            out_text.push_str(&new_block);
            out_text.push_str(&lines[target_last..].concat());
        }
        _ => {
            let mut nb = new_block.clone();
            if had_no_eol && nb.ends_with(eol) {
                nb.truncate(nb.len() - eol.len());
            }
            out_text.push_str(&nb);
            out_text.push_str(&lines[last..].concat());
        }
    }
    if let Err(e) = fs::write(&full, out_text.as_bytes()) {
        return out(false, &format!("NOT EDITED - cannot write {}: {}", shown, e));
    }
    let added = new_block.lines().count();
    let new_total = out_text.split_inclusive('\n').count();
    let new_first = if mode == "after" { target_last + 1 } else { first };
    let head = match mode {
        "delete" => format!("deleted lines {}-{} of {} ({} line(s)); the file has {} lines now", first, last, shown, removed.len(), new_total),
        "before" => format!("inserted {} line(s) before line {} of {}; they are lines {}-{} now ({} lines in all)", added, first, shown, first, first + added - 1, new_total),
        "after" => format!("inserted {} line(s) after line {} of {}; they are lines {}-{} now ({} lines in all)", added, target_last, shown, new_first, new_first + added - 1, new_total),
        _ => format!("replaced lines {}-{} of {} ({} line(s)) with {} line(s); they are lines {}-{} now ({} lines in all)", first, last, shown, removed.len(), added, first, (first + added).saturating_sub(1).max(first), new_total),
    };
    let mut report = vec![head];
    for l in removed.iter().take(6) {
        report.push(format!("- {}", clipped(l)));
    }
    if removed.len() > 6 {
        report.push(format!("- ... {} more removed line(s)", removed.len() - 6));
    }
    for l in new_block.lines().take(6) {
        report.push(format!("+ {}", clipped(l)));
    }
    if added > 6 {
        report.push(format!("+ ... {} more added line(s)", added - 6));
    }
    out(true, &report.join("\n"))
}
