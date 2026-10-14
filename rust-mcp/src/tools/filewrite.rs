//! `mysid write` (create a file with content) and `mysid append` (add text to the end of a file).
//!
//! The text comes from stdin, `--file`, or an MCP parameter, so it never passes through shell quoting.
//! `write` refuses to replace an existing file unless told to; `append` keeps the file's line endings.

use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};

use super::workspace::{resolve_file_in_workspace, resolve_root};

fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

fn content_of(params: &Value) -> Option<String> {
    ["text", "content", "contents"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).map(String::from)
}

fn flag(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false))
}

fn path_of(params: &Value) -> Option<String> {
    ["path", "file", "filepath", "file_path"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()))
        .map(|s| s.trim().to_string())
}

fn is_crlf(content: &str) -> bool {
    content.contains("\r\n") && !content.replace("\r\n", "").contains('\n')
}

fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

fn count_lines(text: &str) -> usize {
    text.lines().count()
}

/// A path for a file that may not exist yet: absolute as given, otherwise inside the workspace (no `..` out of it).
fn target_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let p = Path::new(name);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("'{}' leaves the workspace (..). Use a path inside it, or an absolute path.", name));
    }
    Ok(root.join(p))
}

pub fn execute_write(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(name) = path_of(params) else {
        return out(false, "NOT WRITTEN - a file path is required. Example: mysid write src/New.kt --file content.txt");
    };
    let Some(content) = content_of(params) else {
        return out(false, "NOT WRITTEN - no content received. Pipe it in (mysid write src/New.kt <<'EOF' ... EOF), or use --file content.txt.");
    };
    let overwrite = flag(params, &["overwrite", "force"]);
    let root = Path::new(&workspace_root);
    let full = match target_path(root, &name) {
        Ok(p) => p,
        Err(e) => return out(false, &format!("NOT WRITTEN - {}", e)),
    };
    let shown = full.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| full.to_string_lossy().replace('\\', "/"));

    let existing = fs::read_to_string(&full).ok();
    if full.is_dir() {
        return out(false, &format!("NOT WRITTEN - {} is a folder.", shown));
    }
    if full.exists() && !overwrite {
        let size = fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
        return out(
            false,
            &format!("NOT WRITTEN - {} already exists ({} bytes). To replace it use --overwrite; to change part of it use mysid patch or mysid apply.", shown, size),
        );
    }
    // Replacing a CRLF file keeps it CRLF.
    let data = match &existing {
        Some(old) if is_crlf(old) => to_crlf(&content),
        _ => content.clone(),
    };
    if let Some(dir) = full.parent() {
        if let Err(e) = fs::create_dir_all(dir) {
            return out(false, &format!("NOT WRITTEN - cannot create {}: {}", dir.display(), e));
        }
    }
    if let Err(e) = fs::write(&full, data.as_bytes()) {
        return out(false, &format!("NOT WRITTEN - cannot write {}: {}", shown, e));
    }
    let verb = if existing.is_some() { "replaced" } else { "created" };
    out(true, &format!("{} {} ({} line(s), {} bytes)", verb, shown, count_lines(&data), data.len()))
}

pub fn execute_append(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(name) = path_of(params) else {
        return out(false, "NOT APPENDED - a file name is required. Example: mysid append Notes.md --file more.txt");
    };
    let Some(content) = content_of(params) else {
        return out(false, "NOT APPENDED - no text received. Pipe it in (mysid append Notes.md <<'EOF' ... EOF), or use --file more.txt.");
    };
    let create = flag(params, &["create"]);
    let root = Path::new(&workspace_root);

    let full = match resolve_file_in_workspace(root, &name, &workspace_root) {
        Ok(p) => p,
        Err(e) => {
            if create {
                match target_path(root, &name) {
                    Ok(p) => {
                        return execute_write(&json!({ "workspace_root": workspace_root, "path": p.to_string_lossy(), "text": content }), default_root);
                    }
                    Err(m) => return out(false, &format!("NOT APPENDED - {}", m)),
                }
            }
            let reason = serde_json::from_str::<Value>(&e).ok().and_then(|v| v.get("error").and_then(|x| x.as_str()).map(String::from)).unwrap_or(e);
            return out(false, &format!("NOT APPENDED - {} (add --create to make the file).", reason));
        }
    };
    let existing = match fs::read_to_string(&full) {
        Ok(c) => c,
        Err(e) => return out(false, &format!("NOT APPENDED - cannot read the file: {}", e)),
    };
    let shown = full.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| name.clone());

    let crlf = is_crlf(&existing);
    let mut add = if crlf { to_crlf(&content) } else { content.clone() };
    // The text starts on its own line: add the missing line break at the end of the file.
    let eol = if crlf { "\r\n" } else { "\n" };
    let mut new_content = existing.clone();
    if !new_content.is_empty() && !new_content.ends_with('\n') && !add.starts_with('\n') && !add.starts_with("\r\n") {
        new_content.push_str(eol);
    }
    if !add.is_empty() && !add.ends_with('\n') {
        add.push_str(eol);
    }
    new_content.push_str(&add);
    if let Err(e) = fs::write(&full, new_content.as_bytes()) {
        return out(false, &format!("NOT APPENDED - cannot write {}: {}", shown, e));
    }
    out(true, &format!("appended {} line(s) to {} (now {} lines)", count_lines(&add), shown, count_lines(&new_content)))
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[(&str, &[u8])]) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("mysid-write-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for (f, c) in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c).unwrap();
        }
        let root = dir.to_string_lossy().to_string();
        (dir, root)
    }

    fn call(f: fn(&Value, Option<&str>) -> String, root: &str, extra: Value) -> (bool, String) {
        let mut p = json!({ "workspace_root": root });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        let v: Value = serde_json::from_str(&f(&p, Some(root))).unwrap();
        (v["success"].as_bool().unwrap(), v["output"].as_str().unwrap().to_string())
    }

    #[test]
    fn write_creates_files_and_folders_keeps_the_text_exact_and_refuses_to_overwrite() {
        let (dir, root) = project("write", &[("keep.txt", b"old\n")]);
        let text = "const s = `a ${b}` + 'q' + \"d\" // $5 \\n\n";
        let (ok, o) = call(execute_write, &root, json!({ "path": "src/deep/New.kt", "text": text }));
        assert!(ok && o.starts_with("created src/deep/New.kt (1 line(s)"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("src/deep/New.kt")).unwrap(), text);

        let (ok, o) = call(execute_write, &root, json!({ "path": "keep.txt", "text": "new\n" }));
        assert!(!ok && o.starts_with("NOT WRITTEN") && o.contains("already exists") && o.contains("--overwrite"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("keep.txt")).unwrap(), "old\n");
        let (ok, o) = call(execute_write, &root, json!({ "path": "keep.txt", "text": "new\n", "overwrite": true }));
        assert!(ok && o.starts_with("replaced keep.txt"), "{}", o);

        let (ok, o) = call(execute_write, &root, json!({ "path": "../escape.txt", "text": "x" }));
        assert!(!ok && o.contains("leaves the workspace"), "{}", o);
        assert!(!call(execute_write, &root, json!({ "path": "no_text.txt" })).0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn overwriting_a_crlf_file_keeps_it_crlf() {
        let (dir, root) = project("crlf", &[("a.cs", b"one\r\ntwo\r\n")]);
        call(execute_write, &root, json!({ "path": "a.cs", "text": "x\ny\n", "overwrite": true }));
        assert_eq!(fs::read(dir.join("a.cs")).unwrap(), b"x\r\ny\r\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_adds_on_its_own_line_keeps_endings_finds_files_by_name_and_can_create() {
        let (dir, root) = project("append", &[("docs/Notes.md", b"first"), ("win/Win.txt", b"a\r\nb\r\n")]);
        let (ok, o) = call(execute_append, &root, json!({ "path": "Notes.md", "text": "second\nthird\n" }));
        assert!(ok && o.contains("appended 2 line(s) to docs/Notes.md (now 3 lines)"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("docs/Notes.md")).unwrap(), "first\nsecond\nthird\n");

        call(execute_append, &root, json!({ "path": "Win.txt", "text": "c\nd" }));
        assert_eq!(fs::read(dir.join("win/Win.txt")).unwrap(), b"a\r\nb\r\nc\r\nd\r\n");

        let (ok, o) = call(execute_append, &root, json!({ "path": "Missing.md", "text": "x" }));
        assert!(!ok && o.starts_with("NOT APPENDED") && o.contains("--create"), "{}", o);
        let (ok, o) = call(execute_append, &root, json!({ "path": "Fresh.md", "text": "hello\n", "create": true }));
        assert!(ok && o.starts_with("created Fresh.md"), "{}", o);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_replaces_inserts_and_deletes_by_line_number() {
        let (dir, root) = project("edit", &[("src/A.kt", b"l1\nl2\nl3\nl4\nl5\n")]);
        let (ok, o) = call(execute_edit, &root, json!({ "path": "A.kt#L2-3", "text": "two\nthree\nextra\n" }));
        assert!(ok && o.contains("replaced lines 2-3 of src/A.kt (2 line(s)) with 3 line(s); they are lines 2-4 now (6 lines in all)"), "{}", o);
        assert!(o.contains("- l2") && o.contains("+ extra"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("src/A.kt")).unwrap(), "l1\ntwo\nthree\nextra\nl4\nl5\n");

        let (ok, o) = call(execute_edit, &root, json!({ "path": "A.kt#L1", "text": "top", "insert_before": true }));
        assert!(ok && o.contains("inserted 1 line(s) before line 1") && o.contains("lines 1-1 now"), "{}", o);
        let (ok, _) = call(execute_edit, &root, json!({ "path": "A.kt#L7", "text": "tail", "insert_after": true }));
        assert!(ok);
        assert_eq!(fs::read_to_string(dir.join("src/A.kt")).unwrap(), "top\nl1\ntwo\nthree\nextra\nl4\nl5\ntail\n");

        let (ok, o) = call(execute_edit, &root, json!({ "path": "A.kt#L2-3", "delete": true }));
        assert!(ok && o.starts_with("deleted lines 2-3 of src/A.kt (2 line(s)); the file has 6 lines now"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("src/A.kt")).unwrap(), "top\nthree\nextra\nl4\nl5\ntail\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edit_guards_against_moved_line_numbers_bad_ranges_and_keeps_crlf() {
        let (dir, root) = project("editguard", &[("a.cs", b"one\r\ntwo\r\nthree"), ("b.txt", b"x\ny\n")]);
        // --expect: line 2 is not `three`, nothing changes.
        let (ok, o) = call(execute_edit, &root, json!({ "path": "a.cs#L2", "text": "TWO", "expect": "three" }));
        assert!(!ok && o.starts_with("NOT EDITED") && o.contains("line 2 of a.cs is `two`"), "{}", o);
        assert_eq!(fs::read(dir.join("a.cs")).unwrap(), b"one\r\ntwo\r\nthree");
        // CRLF stays CRLF; the last line had no line ending and still has none.
        let (ok, _) = call(execute_edit, &root, json!({ "path": "a.cs#L2-3", "text": "2\n3", "expect": "two" }));
        assert!(ok);
        assert_eq!(fs::read(dir.join("a.cs")).unwrap(), b"one\r\n2\r\n3");
        // Outside the file / no range / no text.
        let (ok, o) = call(execute_edit, &root, json!({ "path": "b.txt#L5-6", "text": "z" }));
        assert!(!ok && o.contains("has 2 line(s); lines 5-6 are outside it"), "{}", o);
        let (ok, o) = call(execute_edit, &root, json!({ "path": "b.txt", "text": "z" }));
        assert!(!ok && o.contains("no line range"), "{}", o);
        let (ok, o) = call(execute_edit, &root, json!({ "path": "b.txt#L1" }));
        assert!(!ok && o.contains("no text received"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "x\ny\n");
        let _ = fs::remove_dir_all(&dir);
    }
}
