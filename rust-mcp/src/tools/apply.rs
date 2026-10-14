//! `mysid apply`: several edits, in one call, from text that never passes through shell argument parsing
//! (stdin, a file, or an MCP parameter).
//!
//! Format (SEARCH/REPLACE blocks; the file name is the line before `<<<<<<< SEARCH`):
//!
//! ```text
//! src/Foo.kt
//! <<<<<<< SEARCH
//! val retries = 3
//! =======
//! val retries = 5
//! >>>>>>> REPLACE
//! ```
//!
//! Every block is matched with the same engine as `mysid patch` (exact, then line-ending tolerant, then
//! whitespace tolerant; refused on 0 or 2+ matches). Blocks for one file apply in order, each to the result of
//! the previous. Nothing is written unless every block applies: all files or none.

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::patch::{apply as splice, diff_preview, line_of, list_lines, plan};
use super::workspace::{resolve_file_in_workspace, resolve_root};

const SHAPE: &str = "Expected SEARCH/REPLACE blocks:\n\
<file name on its own line>\n\
<<<<<<< SEARCH\n\
<exact text to find>\n\
=======\n\
<replacement text>\n\
>>>>>>> REPLACE";

/// One block of the input.
#[derive(Debug, PartialEq)]
struct Edit {
    path: Option<String>,
    old: String,
    new: String,
}

fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

fn not_applied(reason: &str) -> String {
    out(false, &format!("NOT APPLIED - nothing was changed.\n{}", reason))
}

fn is_marker(line: &str, open: char, word: &str) -> bool {
    let t = line.trim();
    t.chars().take_while(|c| *c == open).count() >= 7 && t.trim_start_matches(open).trim().eq_ignore_ascii_case(word)
}

fn is_separator(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 7 && t.chars().all(|c| c == '=')
}

/// The file name from the line above a SEARCH marker: `src/Foo.kt`, `**src/Foo.kt**`, `File: src/Foo.kt`, `# src/Foo.kt`.
fn file_name_from(line: &str) -> Option<String> {
    let t = line.trim();
    if t.is_empty() || t.starts_with("```") {
        return None;
    }
    let t = t.trim_matches(|c| matches!(c, '*' | '`' | '#' | ' ' | ':' | '"' | '\''));
    let t = ["File:", "file:", "Path:", "path:", "FILE:"].iter().fold(t, |acc, p| acc.strip_prefix(p).unwrap_or(acc)).trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

fn parse_blocks(text: &str) -> Result<Vec<Edit>, String> {
    let text = text.replace("\r\n", "\n");
    let lines: Vec<&str> = text.lines().collect();
    let mut edits = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_marker(lines[i], '<', "SEARCH") {
            i += 1;
            continue;
        }
        // The line above is a file name, unless it closes the previous block (consecutive blocks for one `--to` file).
        let path = if i > 0 && !is_marker(lines[i - 1], '>', "REPLACE") && !is_separator(lines[i - 1]) { file_name_from(lines[i - 1]) } else { None };
        let n = edits.len() + 1;
        let (mut old, mut new) = (Vec::new(), Vec::new());
        i += 1;
        let mut closed_search = false;
        while i < lines.len() {
            if is_separator(lines[i]) {
                closed_search = true;
                i += 1;
                break;
            }
            old.push(lines[i]);
            i += 1;
        }
        if !closed_search {
            return Err(format!("block {}: no ======= line after the SEARCH text", n));
        }
        let mut closed = false;
        while i < lines.len() {
            if is_marker(lines[i], '>', "REPLACE") {
                closed = true;
                i += 1;
                break;
            }
            new.push(lines[i]);
            i += 1;
        }
        if !closed {
            return Err(format!("block {}: no >>>>>>> REPLACE line after the replacement text", n));
        }
        edits.push(Edit { path, old: old.join("\n"), new: new.join("\n") });
    }
    if edits.is_empty() {
        let hint = if text.contains("*** Begin Patch") || text.contains("\n@@ ") || text.starts_with("--- ") || text.starts_with("diff --git") {
            "This looks like an apply_patch or unified-diff patch: those formats are not supported yet.\n"
        } else {
            ""
        };
        return Err(format!("no edit blocks found in the input.\n{}{}", hint, SHAPE));
    }
    Ok(edits)
}

struct FileState {
    shown: String,
    full: PathBuf,
    original: Option<String>,
    current: String,
    changes: usize,
    lines: Vec<usize>,
    strategies: Vec<&'static str>,
}

pub fn execute_apply(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&workspace_root);
    let text = ["text", "patch", "input", "edits", "query"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()))
        .unwrap_or("")
        .to_string();
    let replace_all = ["replace_all", "all"].iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false));
    let dry_run = ["dry_run", "dry-run", "dryrun"].iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false));
    let default_file = params.get("to").and_then(|v| v.as_str()).map(String::from);
    // `--show`: after writing, list what changed (the removed and added lines of each block).
    let show = params.get("show").and_then(|v| v.as_bool()).unwrap_or(false);

    // Shortest form (`mysid patch File.kt --stdin`): the old text, a line `=======`, the new text; no markers needed.
    let text = if default_file.is_some() && !text.contains("<<<<<<<") && text.lines().any(|l| l.trim_end() == "=======") {
        let (mut old, mut new, mut after) = (Vec::new(), Vec::new(), false);
        for l in text.lines() {
            if !after && l.trim_end() == "=======" {
                after = true;
            } else if after {
                new.push(l);
            } else {
                old.push(l);
            }
        }
        format!("<<<<<<< SEARCH\n{}\n=======\n{}\n>>>>>>> REPLACE\n", old.join("\n"), new.join("\n"))
    } else {
        text
    };
    let edits = match parse_blocks(&text) {
        Ok(e) => e,
        Err(e) => return not_applied(&e),
    };

    let mut files: Vec<FileState> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let mut preview: Vec<String> = Vec::new();

    for (n, edit) in edits.iter().enumerate() {
        let n = n + 1;
        let Some(name) = edit.path.clone().or_else(|| default_file.clone()) else {
            return not_applied(&format!("block {}: no file name above <<<<<<< SEARCH (or pass --to <file>).\n{}", n, SHAPE));
        };
        // Same file named two ways resolves to the same state.
        let fi = match index_of.get(&name) {
            Some(&i) => i,
            None => {
                let state = match resolve_file_in_workspace(root, &name, &workspace_root) {
                    Ok(full) => {
                        let original = match fs::read_to_string(&full) {
                            Ok(c) => c,
                            Err(e) => return not_applied(&format!("block {} ({}): cannot read the file: {}", n, name, e)),
                        };
                        let shown = full.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| name.clone());
                        FileState { shown, full, current: original.clone(), original: Some(original), changes: 0, lines: Vec::new(), strategies: Vec::new() }
                    }
                    Err(_) if edit.old.trim().is_empty() => {
                        // An empty SEARCH on a file that does not exist creates it.
                        let full = root.join(&name);
                        FileState { shown: name.replace('\\', "/"), full, original: None, current: String::new(), changes: 0, lines: Vec::new(), strategies: Vec::new() }
                    }
                    Err(e) => {
                        let reason = serde_json::from_str::<Value>(&e).ok().and_then(|v| v.get("error").and_then(|x| x.as_str()).map(String::from)).unwrap_or(e);
                        return not_applied(&format!("block {} ({}): {}", n, name, reason));
                    }
                };
                files.push(state);
                index_of.insert(name.clone(), files.len() - 1);
                files.len() - 1
            }
        };
        let st = &mut files[fi];

        if edit.old.trim().is_empty() {
            if st.original.is_some() {
                return not_applied(&format!("block {} ({}): the SEARCH text is empty, which only creates a file that does not exist yet.", n, st.shown));
            }
            st.current = edit.new.clone();
            st.changes += 1;
            st.strategies.push("create");
            continue;
        }
        if edit.old == edit.new {
            return not_applied(&format!("block {} ({}): SEARCH and REPLACE are identical.", n, st.shown));
        }
        let Some(found) = plan(&st.current, &edit.old, &edit.new) else {
            let first = edit.old.lines().next().unwrap_or("").trim();
            return not_applied(&format!(
                "block {} ({}): the SEARCH text was not found. First line searched: `{}`.\n{}\nRe-read the file (mysid read) and copy the text exactly, or use less text.",
                n, st.shown, first, super::patch::not_found_hint(&st.current, &edit.old)
            ));
        };
        let lines: Vec<usize> = found.ranges.iter().map(|&(s, _)| line_of(&st.current, s)).collect();
        if found.ranges.len() > 1 && !replace_all {
            return not_applied(&format!(
                "block {} ({}): the SEARCH text matches {} places (lines {}). Add surrounding context to make it unique, or pass --replace_all.",
                n,
                st.shown,
                found.ranges.len(),
                list_lines(&lines)
            ));
        }
        let chosen: &[(usize, usize)] = if replace_all { &found.ranges } else { &found.ranges[..1] };
        if dry_run || show {
            preview.push(format!("{} block {}:", st.shown, n));
            preview.extend(diff_preview(&st.current, chosen, &found.replacement));
        }
        st.current = splice(&st.current, chosen, &found.replacement);
        st.changes += chosen.len();
        st.lines.extend(lines.iter().take(chosen.len()));
        st.strategies.push(found.strategy);
    }

    let total: usize = files.iter().map(|f| f.changes).sum();
    let summary: Vec<String> = files
        .iter()
        .map(|f| {
            let how = if f.original.is_none() { "created".to_string() } else { format!("{} change(s) at line {}", f.changes, list_lines(&f.lines)) };
            let mut strategies = f.strategies.clone();
            strategies.dedup();
            format!("  {}: {} ({})", f.shown, how, strategies.join(", "))
        })
        .collect();

    if dry_run {
        let mut o = vec![format!("dry-run: {} change(s) in {} file(s), nothing written", total, files.len())];
        o.extend(summary);
        o.extend(preview);
        return out(true, &o.join("\n"));
    }

    // Write everything, or put back what was already written.
    let mut written: Vec<&FileState> = Vec::new();
    for f in &files {
        if let Some(dir) = f.full.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Err(e) = fs::write(&f.full, f.current.as_bytes()) {
            for w in &written {
                match &w.original {
                    Some(o) => {
                        let _ = fs::write(&w.full, o.as_bytes());
                    }
                    None => {
                        let _ = fs::remove_file(&w.full);
                    }
                }
            }
            return not_applied(&format!("could not write {}: {}. Files already written were put back.", f.shown, e));
        }
        written.push(f);
    }
    let mut o = vec![format!("applied {} change(s) in {} file(s)", total, files.len())];
    o.extend(summary);
    if show {
        o.extend(preview);
    }
    out(true, &o.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str, files: &[(&str, &str)]) -> (PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("mysid-apply-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for (f, c) in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, c.as_bytes()).unwrap();
        }
        let root = dir.to_string_lossy().to_string();
        (dir, root)
    }

    fn run(root: &str, text: &str, extra: Value) -> (bool, String) {
        let mut p = json!({ "workspace_root": root, "text": text });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        let v: Value = serde_json::from_str(&execute_apply(&p, Some(root))).unwrap();
        (v["success"].as_bool().unwrap(), v["output"].as_str().unwrap().to_string())
    }

    #[test]
    fn consecutive_snippets_without_file_names_are_not_mistaken_for_names() {
        // patch_batch <file> / apply --to <file>: the line above the second SEARCH is the first block's closing marker.
        let text = "<<<<<<< SEARCH\none\n=======\n1\n>>>>>>> REPLACE\n<<<<<<< SEARCH\ntwo\n=======\n2\n>>>>>>> REPLACE\n";
        let edits = parse_blocks(text).unwrap();
        assert_eq!(edits.len(), 2);
        assert!(edits[0].path.is_none() && edits[1].path.is_none(), "{:?}", edits);
        let (dir, root) = project("batch", &[("f.txt", "one\ntwo\n")]);
        let (ok, o) = run(&root, text, json!({ "to": "f.txt", "show": true }));
        assert!(ok && o.contains("applied 2 change(s) in 1 file(s)") && o.contains("- one") && o.contains("+ 2"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("f.txt")).unwrap(), "1\n2\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn blocks_are_parsed_with_file_names_fences_and_prose() {
        let text = "Here is the change:\n\n**src/A.kt**\n<<<<<<< SEARCH\nval a = `x` + \"$y\"\n=======\nval a = 'z'\n>>>>>>> REPLACE\n\n```kotlin\nFile: B.kt\n<<<<<<< SEARCH\nold\n=======\nnew\n>>>>>>> REPLACE\n```\n";
        let edits = parse_blocks(text).unwrap();
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0], Edit { path: Some("src/A.kt".into()), old: "val a = `x` + \"$y\"".into(), new: "val a = 'z'".into() });
        assert_eq!(edits[1].path.as_deref(), Some("B.kt"));
        assert!(parse_blocks("nothing here").unwrap_err().contains("Expected SEARCH/REPLACE"));
        assert!(parse_blocks("f\n<<<<<<< SEARCH\nx\n").unwrap_err().contains("no ======="));
        assert!(parse_blocks("f\n<<<<<<< SEARCH\nx\n=======\ny\n").unwrap_err().contains("no >>>>>>> REPLACE"));
    }

    #[test]
    fn several_blocks_in_several_files_apply_together_keeping_shell_sensitive_text_intact() {
        let (dir, root) = project("multi", &[("ui/A.kt", "val a = 1\r\nval b = 2\r\n"), ("lib/B.ts", "const s = 'x'\n")]);
        let text = "A.kt\n<<<<<<< SEARCH\nval a = 1\n=======\nval a = \"${'$'}x\" // `tick`\n>>>>>>> REPLACE\nA.kt\n<<<<<<< SEARCH\nval b = 2\n=======\nval b = 3\n>>>>>>> REPLACE\nB.ts\n<<<<<<< SEARCH\nconst s = 'x'\n=======\nconst s = `t ${1}`\\n\n>>>>>>> REPLACE\n";
        let (ok, o) = run(&root, text, json!({}));
        assert!(ok, "{}", o);
        assert!(o.starts_with("applied 3 change(s) in 2 file(s)"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("ui/A.kt")).unwrap(), "val a = \"${'$'}x\" // `tick`\r\nval b = 3\r\n", "CRLF kept");
        assert_eq!(fs::read_to_string(dir.join("lib/B.ts")).unwrap(), "const s = `t ${1}`\\n\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn one_bad_block_means_nothing_is_written() {
        let (dir, root) = project("atomic", &[("A.kt", "one\n"), ("B.kt", "two\n")]);
        let text = "A.kt\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>> REPLACE\nB.kt\n<<<<<<< SEARCH\nmissing line\n=======\nx\n>>>>>>> REPLACE\n";
        let (ok, o) = run(&root, text, json!({}));
        assert!(!ok);
        assert!(o.starts_with("NOT APPLIED - nothing was changed."), "{}", o);
        assert!(o.contains("block 2 (B.kt)") && o.contains("`missing line`"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("A.kt")).unwrap(), "one\n", "the first block was not written");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ambiguous_and_ambiguous_file_names_are_refused_and_replace_all_works() {
        let (dir, root) = project("amb", &[("a/Foo.kt", "x = 1\nx = 1\n"), ("b/Foo.kt", "y\n")]);
        let (ok, o) = run(&root, "Foo.kt\n<<<<<<< SEARCH\ny\n=======\nz\n>>>>>>> REPLACE\n", json!({}));
        assert!(!ok && o.contains("ambiguous") && o.contains("b/Foo.kt"), "{}", o);
        let (ok, o) = run(&root, "a/Foo.kt\n<<<<<<< SEARCH\nx = 1\n=======\nx = 2\n>>>>>>> REPLACE\n", json!({}));
        assert!(!ok && o.contains("matches 2 places (lines 1, 2)"), "{}", o);
        let (ok, _) = run(&root, "a/Foo.kt\n<<<<<<< SEARCH\nx = 1\n=======\nx = 2\n>>>>>>> REPLACE\n", json!({ "replace_all": true }));
        assert!(ok);
        assert_eq!(fs::read_to_string(dir.join("a/Foo.kt")).unwrap(), "x = 2\nx = 2\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dry_run_previews_and_an_empty_search_creates_a_new_file() {
        let (dir, root) = project("dry", &[("A.kt", "one\n")]);
        let text = "A.kt\n<<<<<<< SEARCH\none\n=======\nONE\n>>>>>>> REPLACE\nnew/N.kt\n<<<<<<< SEARCH\n=======\nclass N\n>>>>>>> REPLACE\n";
        let (ok, o) = run(&root, text, json!({ "dry_run": true }));
        assert!(ok && o.contains("dry-run: 2 change(s)") && o.contains("- one") && o.contains("+ ONE"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("A.kt")).unwrap(), "one\n");
        assert!(!dir.join("new/N.kt").exists());
        let (ok, o) = run(&root, text, json!({}));
        assert!(ok && o.contains("new/N.kt: created"), "{}", o);
        assert_eq!(fs::read_to_string(dir.join("new/N.kt")).unwrap(), "class N");
        let (ok, o) = run(&root, "A.kt\n<<<<<<< SEARCH\n=======\nx\n>>>>>>> REPLACE\n", json!({}));
        assert!(!ok && o.contains("only creates a file that does not exist"), "{}", o);
        let _ = fs::remove_dir_all(&dir);
    }
}
