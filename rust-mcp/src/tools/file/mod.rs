//! File operations, one file per command: new, write, append, edit, mkdir, mv (rename), cp (copy), rm (delete), ls (list).
//! `execute_file` is the dispatcher behind `mysid file <action>`.

mod append;
mod cp;
mod delete;
mod edit;
mod list;
mod mkdir;
mod new;
mod rename;
mod util;
mod write;

pub use append::execute_append;
pub use cp::execute_copy;
pub use delete::execute_delete;
pub use edit::execute_edit;
pub use list::execute_list_dir;
pub use mkdir::execute_mkdir;
pub use new::execute_create_file;
pub use rename::execute_rename;
pub use write::execute_write;

use serde_json::{json, Value};

pub fn execute_file(params: &Value, default_root: Option<&str>) -> String {
    let action = params
        .get("action")
        .and_then(|v| v.as_str())
        .or_else(|| {
            params
                .get("args")
                .and_then(|v| v.as_array())
                .and_then(|a| a.get(0))
                .and_then(|v| v.as_str())
        })
        .unwrap_or("")
        .to_ascii_lowercase();

    match action.as_str() {
        "" | "help" | "-h" | "--help" => {
            "mysid file: Filesystem management utilities\n\n\
Usage:\n  \
  mysid file new <path> <content>         Create or overwrite file (auto-creates parent dirs)\n  \
  mysid file new <path> --stdin | -       ... with the content from stdin (heredoc / pipe), no shell quoting\n  \
  mysid file new <path> --from <file>     ... copied from a file (piped input also works)\n  \
                                          (mysid write <path> is the same but refuses to replace an existing file)\n  \
  mysid file mkdir <path>                 Recursively create directory\n  \
  mysid file rename <old_path> <new_path> Rename or move file/directory\n  \
  mysid file copy <src>... <dest>         Copy files or folders (many at once; --overwrite to replace)\n  \
  mysid file delete <path> [--recursive]  Delete file or directory\n  \
  mysid file list [path] [-L<depth>]      List files (defaults to -L1, respects .gitignore)\n".to_string()
        }
        "new" | "create" | "write" => execute_create_file(params, default_root),
        "mkdir" | "new_folder" | "create_folder" => execute_mkdir(params, default_root),
        "rename" | "mv" | "move" => execute_rename(params, default_root),
        "copy" | "cp" => execute_copy(params, default_root),
        "delete" | "remove" | "rm" => execute_delete(params, default_root),
        "list" | "ls" | "list_dir" | "dir" => execute_list_dir(params, default_root),
        _ => json!({
            "success": false,
            "error": format!("Unknown file action '{}'. Run 'mysid file --help' for usage.", action)
        }).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

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
