//! Turning `mysid <command> ...` words into a verb and its parameters.

use serde_json::{json, Value};
use std::io::{self, IsTerminal};


pub(crate) fn coerce_value(val: &str) -> Value {
    if val.eq_ignore_ascii_case("true") {
        json!(true)
    } else if val.eq_ignore_ascii_case("false") {
        json!(false)
    } else if let Ok(num) = val.parse::<i64>() {
        json!(num)
    } else {
        json!(val)
    }
}

/// `mysid exec <command...>`: everything after the verb is the command line, so `-A`, `-m`, `|` and
/// quoted words reach the shell untouched. Only mysid's own flags are taken out.
pub(crate) fn parse_exec_args(rest: &[String]) -> Value {
    let mut params = serde_json::Map::new();
    let mut parts: Vec<String> = Vec::new();
    let mut i = 0;
    let mut after_separator = false;
    while i < rest.len() {
        let a = rest[i].as_str();
        // `--` ends mysid's flags: everything after it is the command, whatever it looks like.
        if after_separator {
            parts.push(a.to_string());
            i += 1;
            continue;
        }
        // Before the command, or after a whole quoted command line (`exec "cmd -x" --tail 20`): the flags are mysid's.
        let own = parts.is_empty() || (parts.len() == 1 && parts[0].contains(char::is_whitespace));
        match a {
            "--" => {
                after_separator = true;
                i += 1;
            }
            "--errors" if own => {
                params.insert("errors".to_string(), json!(true));
                i += 1;
            }
            "--env" if own && i + 1 < rest.len() => {
                params.insert("env".to_string(), json!(rest[i + 1]));
                i += 2;
            }
            "--workspace_root" | "--project_path" | "--workspace" if i + 1 < rest.len() => {
                let key = if a == "--workspace" { "workspace_root" } else { &a[2..] };
                params.insert(key.to_string(), json!(rest[i + 1]));
                i += 2;
            }
            "--out" if i + 1 < rest.len() => {
                params.insert("out".to_string(), json!(rest[i + 1]));
                i += 2;
            }
            "--head" | "--tail" | "--grep" | "--grep-v" if own && i + 1 < rest.len() => {
                params.insert(a[2..].to_string(), json!(rest[i + 1]));
                i += 2;
            }
            "--shell" if i + 1 < rest.len() => {
                params.insert("shell".to_string(), json!(rest[i + 1]));
                i += 2;
            }
            "--timeout" | "--timeout_secs" if i + 1 < rest.len() => {
                if let Ok(n) = rest[i + 1].parse::<u64>() {
                    params.insert("timeout_secs".to_string(), json!(n));
                }
                i += 2;
            }
            _ => {
                parts.push(a.to_string());
                i += 1;
            }
        }
    }
    // One argument is already a full command line. Several were split by the caller's shell:
    // words containing spaces get their quotes back; operators such as | and && stay as they are.
    let command = if parts.len() == 1 {
        parts.remove(0)
    } else {
        parts
            .iter()
            .map(|a| if a.chars().any(char::is_whitespace) && !a.starts_with('"') { format!("\"{}\"", a) } else { a.clone() })
            .collect::<Vec<_>>()
            .join(" ")
    };
    if !command.is_empty() {
        params.insert("command".to_string(), json!(command));
    }
    Value::Object(params)
}

/// Text for a command from a file (`-` is stdin). `one_line` drops a single trailing newline, the usual
/// editor artifact in a file that holds a snippet. A file that cannot be read ends the command with its error.
pub(crate) fn read_text_input(path: &str, one_line: bool) -> String {
    use std::io::Read;
    let mut text = String::new();
    let result = if path == "-" {
        // Read on a thread so a caller that leaves stdin open without sending anything cannot hang the command.
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut buf = String::new();
            let r = io::stdin().read_to_string(&mut buf).map(|_| buf);
            let _ = tx.send(r);
        });
        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(Ok(t)) => {
                text = t;
                Ok(())
            }
            Ok(Err(e)) => Err(e),
            Err(_) => Err(io::Error::new(io::ErrorKind::TimedOut, "no input arrived within 10 s (pass --file, or pipe the text in)")),
        }
    } else {
        std::fs::read_to_string(path).map(|t| text = t)
    };
    if let Err(e) = result {
        println!("ERROR: cannot read {}: {}", if path == "-" { "stdin" } else { path }, e);
        std::process::exit(1);
    }
    if one_line {
        if text.ends_with("\r\n") {
            text.truncate(text.len() - 2);
        } else if text.ends_with('\n') {
            text.truncate(text.len() - 1);
        }
    }
    text
}

pub(crate) fn parse_cli_args(args: &[String]) -> (String, Value) {
    let mut verb = args[0].clone();
    if verb == "exec" || verb == "run" {
        return (verb, parse_exec_args(&args[1..]));
    }
    // `mysid msvc <cmd>` / `mysid emsdk <cmd>` (also `cl`, `wasm`): exec with the toolchain's environment set up first.
    if matches!(verb.as_str(), "msvc" | "cl" | "emsdk" | "wasm") {
        let env = if matches!(verb.as_str(), "msvc" | "cl") { "msvc" } else { "emsdk" };
        let mut p = parse_exec_args(&args[1..]);
        p["env"] = json!(env);
        return ("exec".to_string(), p);
    }
    let mut params = serde_json::Map::new();
    let mut positionals = Vec::new();
    let mut i = 1;

    // Check for --json <file> or --json '<raw_json>'
    while i < args.len() {
        let a = &args[i];
        if a == "--json" && i + 1 < args.len() && verb != "android" && verb != "adb" {
            let json_arg = &args[i + 1];
            if let Ok(content) = std::fs::read_to_string(json_arg) {
                if let Ok(parsed) = serde_json::from_str::<Value>(&content) {
                    return (verb, parsed);
                }
            } else if let Ok(parsed) = serde_json::from_str::<Value>(json_arg) {
                return (verb, parsed);
            }
            i += 2;
        } else if a.starts_with("-L") && a.len() > 2 {
            if let Ok(lvl) = a[2..].parse::<u64>() {
                params.insert("max_depth".to_string(), json!(lvl));
            }
            i += 1;
        } else if a == "-full" || a == "-all" {
            params.insert("full".to_string(), json!(true));
            i += 1;
        } else if a.starts_with("--") {
            let key = &a[2..];
            if let Some((k, v)) = key.split_once('=') {
                params.insert(k.to_string(), coerce_value(v));
                i += 1;
            } else if i + 1 < args.len() && !args[i + 1].starts_with("--") {
                params.insert(key.to_string(), coerce_value(&args[i + 1]));
                i += 2;
            } else {
                params.insert(key.to_string(), json!(true));
                i += 1;
            }
        } else {
            positionals.push(a.clone());
            i += 1;
        }
    }

    if verb == "set_workspace" || verb == "setworkspace" || verb == "set" {
        params.insert("path".to_string(), json!(positionals.join(" ")));
    } else if verb == "list_dir" || verb == "ls" || verb == "list" || verb == "dir" {
        if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
        }
    } else if verb == "patch" {
        // mysid patch <file> <old> <new> [--dry-run] [--replace_all]
        // A flag placed before the file would swallow it as its value (`patch --dry-run a.txt ...`).
        for flag in ["dry-run", "dry_run", "dryrun", "replace_all", "all", "stdin", "show"] {
            if let Some(Value::String(v)) = params.get(flag).cloned() {
                positionals.insert(0, v);
                params.insert(flag.replace('-', "_"), json!(true));
            }
        }
        if let Some(file) = positionals.first() {
            params.insert("path".to_string(), json!(file));
        }
        // `patch <file> --stdin`: the SEARCH/REPLACE block(s) arrive on stdin and `apply` does the work.
        if params.contains_key("stdin") {
            let target = positionals.first().cloned();
            params.remove("stdin");
            params.remove("path");
            if let Some(t) = target {
                params.insert("to".to_string(), json!(t));
            }
            params.insert("text".to_string(), json!(read_text_input("-", false)));
            verb = "apply".to_string();
        }
        if positionals.len() > 1 {
            params.insert("old_string".to_string(), json!(positionals[1]));
        }
        if positionals.len() > 2 {
            params.insert("new_string".to_string(), json!(positionals[2..].join(" ")));
        }
        // `--old-file a.txt --new-file b.txt`: the text comes from files, so the shell never sees it.
        for (flag, key) in [("old-file", "old_string"), ("old_file", "old_string"), ("old-from", "old_string"), ("old_from", "old_string"), ("new-file", "new_string"), ("new_file", "new_string"), ("new-from", "new_string"), ("new_from", "new_string")] {
            if let Some(Value::String(path)) = params.get(flag).cloned() {
                params.insert(key.to_string(), json!(read_text_input(&path, true)));
            }
        }
    } else if verb == "patch_batch" && !params.contains_key("json") && !params.contains_key("patches") {
        // mysid patch_batch <file> --stdin | --file edits.txt : several SEARCH/REPLACE snippets for ONE file, all or nothing.
        for flag in ["dry-run", "dry_run", "dryrun", "replace_all", "all", "show", "stdin"] {
            if let Some(Value::String(v)) = params.get(flag).cloned() {
                positionals.insert(0, v);
                params.insert(flag.replace('-', "_"), json!(true));
            }
        }
        params.remove("stdin");
        if !positionals.is_empty() {
            let target = positionals.remove(0);
            params.insert("to".to_string(), json!(target));
        }
        let text = if let Some(Value::String(path)) = params.get("file").or_else(|| params.get("from")).cloned() {
            read_text_input(&path, false)
        } else if !positionals.is_empty() {
            positionals.join("\n")
        } else {
            read_text_input("-", false)
        };
        params.remove("file");
        params.insert("text".to_string(), json!(text));
        verb = "apply".to_string();
    } else if verb == "apply" {
        // mysid apply [--file edit.txt | -] [--dry-run] [--replace_all] [--to File.kt]; or the blocks on stdin.
        for flag in ["dry-run", "dry_run", "dryrun", "replace_all", "all", "show"] {
            if let Some(Value::String(v)) = params.get(flag).cloned() {
                positionals.insert(0, v);
                params.insert(flag.replace('-', "_"), json!(true));
            }
        }
        let text = if let Some(Value::String(path)) = params.get("file").or_else(|| params.get("from")).cloned() {
            read_text_input(&path, false)
        } else if !positionals.is_empty() {
            positionals.join("\n")
        } else if !io::stdin().is_terminal() {
            read_text_input("-", false)
        } else {
            String::new()
        };
        params.remove("file");
        params.insert("text".to_string(), json!(text));
    } else if verb == "search" {
        // Bare flags would otherwise swallow the query as their value (`search --case foo`).
        for flag in ["case", "regex", "group", "all", "vendor", "count"] {
            if let Some(Value::String(q)) = params.get(flag).cloned() {
                positionals.insert(0, q);
                params.insert(flag.to_string(), json!(true));
            }
        }
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals[0]));
            // Extra words naming files or directories scope the search; others join the query (search.rs).
            if positionals.len() > 1 {
                params.insert("extra".to_string(), json!(positionals[1..].to_vec()));
            }
        }
    } else if verb == "replace" {
        if positionals.len() >= 3 {
            let p0 = &positionals[0];
            let looks_like_file = p0.contains('.') || p0.contains('/') || p0.contains('\\');
            if looks_like_file {
                params.insert("file".to_string(), json!(p0));
                params.insert("old_string".to_string(), json!(positionals[1]));
                params.insert("new_string".to_string(), json!(positionals[2..].join(" ")));
            } else {
                params.insert("old_string".to_string(), json!(p0));
                params.insert("new_string".to_string(), json!(positionals[1..].join(" ")));
            }
        } else if positionals.len() == 2 {
            params.insert("old_string".to_string(), json!(positionals[0]));
            params.insert("new_string".to_string(), json!(positionals[1]));
        } else if positionals.len() == 1 {
            params.insert("old_string".to_string(), json!(positionals[0]));
        }
    } else if verb == "exec" || verb == "run" {
        if !positionals.is_empty() {
            params.insert("command".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "android" || verb == "adb" {
        if params.contains_key("help") || params.contains_key("h") || positionals.is_empty() || (positionals.len() == 1 && (positionals[0] == "help" || positionals[0] == "--help" || positionals[0] == "-h")) {
            params.insert("action".to_string(), json!("help"));
        } else {
            params.insert("action".to_string(), json!(positionals[0]));
            if positionals.len() > 1 && !params.contains_key("package_name") {
                params.insert("package_name".to_string(), json!(positionals[1]));
            }
            // Touch commands take bare words too: `tap 540 1200`, `type hello`, `keyevent back`.
            if positionals.len() > 1 {
                params.insert("args".to_string(), json!(positionals[1..].to_vec()));
            }
            // A value-less flag would swallow the next word (`tap --grid 540 1200`).
            for name in ["grid", "no-shot", "no_shot", "focus", "list", "no-ok", "no_ok", "no-diff", "no_diff", "fast"] {
                if let Some(Value::String(v)) = params.get(name).cloned() {
                    if !v.is_empty() {
                        let args = params.entry("args".to_string()).or_insert_with(|| json!([]));
                        if let Some(a) = args.as_array_mut() {
                            a.insert(0, json!(v));
                        }
                    }
                    params.insert(name.to_string(), json!(true));
                }
            }
        }
    } else if verb == "clean" {
        // mysid clean [folder] [--yes]: a value-less --yes written before the folder would swallow it.
        for name in ["yes", "include-tools", "include_tools"] {
            if let Some(Value::String(v)) = params.get(name).cloned() {
                if !v.is_empty() && v != "true" {
                    positionals.insert(0, v);
                }
                params.insert(name.to_string(), json!(true));
            }
        }
        if let Some(p) = positionals.first() {
            params.insert("path".to_string(), json!(p));
        }
    } else if verb == "health" {
        // mysid health [folder] [--top n] [--depth n]
        if let Some(p) = positionals.first() {
            params.insert("path".to_string(), json!(p));
        }
    } else if verb == "tree" {
        // mysid tree [folder] [--depth n] [--all]
        let mut words: Vec<String> = positionals.clone();
        if let Some(Value::String(v)) = params.get("all").cloned() {
            if !v.is_empty() && v != "true" {
                words.insert(0, v);
            }
            params.insert("all".to_string(), json!(true));
        }
        if let Some(p) = words.first() {
            params.insert("path".to_string(), json!(p));
        }
    } else if verb == "doctor" {
        // mysid doctor compare <old> <new> [--max n] [--brief] [--everything]
        // A value-less flag written before the folders would swallow one of them: give it back.
        let mut words: Vec<String> = positionals.clone();
        for name in ["brief", "everything"] {
            if let Some(Value::String(v)) = params.get(name).cloned() {
                if !v.is_empty() && v != "true" {
                    words.push(v);
                }
                params.insert(name.to_string(), json!(true));
            }
        }
        if let Some(sub) = words.first() {
            params.insert("sub".to_string(), json!(sub));
        }
        if let Some(a) = words.get(1) {
            params.insert("a".to_string(), json!(a));
        }
        if let Some(b) = words.get(2) {
            params.insert("b".to_string(), json!(b));
        }
    } else if verb == "mkdir" {
        if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
        }
    } else if verb == "overview" || verb == "signatures" {
        if !positionals.is_empty() {
            if positionals.len() == 1 {
                params.insert("path".to_string(), json!(positionals[0]));
            } else {
                params.insert("files".to_string(), json!(positionals));
            }
        }
    } else if verb == "rename" || verb == "mv" {
        if positionals.len() > 0 {
            params.insert("old_path".to_string(), json!(positionals[0]));
        }
        if positionals.len() > 1 {
            params.insert("new_path".to_string(), json!(positionals[1]));
        }
    } else if verb == "edit" {
        // mysid edit <file>#L10-20 ["new text" | --file f | stdin] [--insert-after | --insert-before | --delete] [--expect "text"]
        // A value-less flag swallows the word after it: the file spec when the flag came first (`--delete f#L3`), the new
        // text when it came after the file (`f#L3 --insert-after "text"`). Give it back where it belongs.
        for flag in ["delete", "insert-after", "insert_after", "after", "insert-before", "insert_before", "before"] {
            if let Some(Value::String(v)) = params.get(flag).cloned() {
                if v.contains("#L") || v.contains("#l") || positionals.is_empty() {
                    positionals.insert(0, v);
                } else {
                    positionals.push(v);
                }
                params.insert(flag.to_string(), json!(true));
            }
        }
        if let Some(name) = positionals.first() {
            params.insert("path".to_string(), json!(name));
        }
        let deleting = matches!(params.get("delete"), Some(Value::Bool(true)));
        let text = if let Some(Value::String(path)) = params.get("file").or_else(|| params.get("from")).cloned() {
            Some(read_text_input(&path, false))
        } else if positionals.len() > 1 {
            Some(positionals[1..].join(" "))
        } else if !deleting && !io::stdin().is_terminal() {
            Some(read_text_input("-", false))
        } else {
            None
        };
        params.remove("file");
        if let Some(t) = text {
            params.insert("text".to_string(), json!(t));
        }
    } else if verb == "write" || verb == "append" {
        // mysid write <file> [--file content.txt | <<'EOF' on stdin | inline text] [--overwrite]; append likewise [--create]
        for flag in ["overwrite", "force", "create"] {
            if let Some(Value::String(v)) = params.get(flag).cloned() {
                positionals.insert(0, v);
                params.insert(flag.to_string(), json!(true));
            }
        }
        if let Some(name) = positionals.first() {
            params.insert("path".to_string(), json!(name));
        }
        let text = if let Some(Value::String(path)) = params.get("file").or_else(|| params.get("from")).cloned() {
            Some(read_text_input(&path, false))
        } else if positionals.len() > 1 {
            Some(positionals[1..].join(" "))
        } else if !io::stdin().is_terminal() {
            Some(read_text_input("-", false))
        } else {
            None
        };
        params.remove("file");
        if let Some(t) = text {
            params.insert("text".to_string(), json!(t));
        }
    } else if verb == "create_file" {
        if positionals.len() > 0 {
            params.insert("path".to_string(), json!(positionals[0]));
        }
        if positionals.len() > 1 {
            params.insert("content".to_string(), json!(positionals[1..].join(" ")));
        }
    } else if verb == "file" {
        if params.contains_key("help") || params.contains_key("h") || positionals.is_empty() || (positionals.len() == 1 && (positionals[0] == "help" || positionals[0] == "--help" || positionals[0] == "-h")) {
            params.insert("action".to_string(), json!("help"));
        } else {
            let action = positionals[0].to_ascii_lowercase();
            params.insert("action".to_string(), json!(action));
            if action == "new" || action == "create" || action == "write" {
                // `--stdin` / `-` / piped input / `--from <file>`: the content never passes through shell arguments.
                // A flag placed before the path would swallow it (`file new --stdin Foo.kt`).
                let mut use_stdin = false;
                // `--overwrite` written before the path would swallow it as its value.
                if let Some(Value::String(v)) = params.get("overwrite").cloned() {
                    if !v.is_empty() && positionals.len() == 1 {
                        positionals.push(v);
                    }
                    params.insert("overwrite".to_string(), json!(true));
                }
                if let Some(v) = params.get("stdin").cloned() {
                    use_stdin = true;
                    if let Value::String(path) = v {
                        positionals.insert(1, path);
                    }
                    params.insert("stdin".to_string(), json!(true));
                }
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
                let from = params.get("from").or_else(|| params.get("file")).and_then(|v| v.as_str()).map(String::from);
                let content = if let Some(path) = from {
                    Some(read_text_input(&path, false))
                } else if use_stdin || positionals.get(2).map(String::as_str) == Some("-") {
                    Some(read_text_input("-", false))
                } else if positionals.len() > 2 {
                    Some(positionals[2..].join(" "))
                } else if positionals.len() > 1 && !io::stdin().is_terminal() {
                    Some(read_text_input("-", false))
                } else {
                    None
                };
                params.remove("file");
                if let Some(c) = content {
                    params.insert("content".to_string(), json!(c));
                }
            } else if action == "copy" || action == "cp" {
                if let Some(Value::String(v)) = params.get("overwrite").cloned() {
                    positionals.push(v);
                    params.insert("overwrite".to_string(), json!(true));
                }
                if positionals.len() > 2 {
                    params.insert("sources".to_string(), json!(positionals[1..positionals.len() - 1].to_vec()));
                    params.insert("dest".to_string(), json!(positionals[positionals.len() - 1]));
                }
            } else if action == "rename" || action == "mv" || action == "move" {
                if positionals.len() > 1 {
                    params.insert("old_path".to_string(), json!(positionals[1]));
                }
                if positionals.len() > 2 {
                    params.insert("new_path".to_string(), json!(positionals[2]));
                }
            } else if action == "delete" || action == "remove" || action == "rm" {
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
            } else if action == "mkdir" || action == "create_folder" || action == "new_folder" {
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
            } else if action == "list" || action == "ls" || action == "list_dir" || action == "dir" {
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
            } else {
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
            }
        }
    } else if verb == "git" || verb == "purge" {
        // mysid git purge [--gitignore ...] [--yes]  /  mysid purge ...: a bare flag would swallow the next word.
        for k in ["yes", "gitignore", "gitattributes", "gitmodules", "extras", "nested", "all"] {
            if let Some(Value::String(v)) = params.get(k).cloned() {
                if !v.is_empty() {
                    positionals.push(v);
                }
                params.insert(k.to_string(), json!(true));
            }
        }
        if verb == "git" {
            if let Some(first) = positionals.first() {
                params.insert("action".to_string(), json!(first));
            }
        }
    } else if verb == "publish" {
        // mysid publish <repo-url> [--yes] [--name origin]; a bare --yes before the url would swallow it.
        if let Some(Value::String(v)) = params.get("yes").cloned() {
            if !v.is_empty() {
                positionals.insert(0, v);
            }
            params.insert("yes".to_string(), json!(true));
        }
        if let Some(first) = positionals.first() {
            params.insert("url".to_string(), json!(first));
        }
    } else if verb == "copy" || verb == "cp" {
        // mysid copy <src>... <dest> [--overwrite]; a bare flag would swallow the next word.
        if let Some(Value::String(v)) = params.get("overwrite").cloned() {
            positionals.push(v);
            params.insert("overwrite".to_string(), json!(true));
        }
        if positionals.len() > 1 {
            params.insert("sources".to_string(), json!(positionals[..positionals.len() - 1].to_vec()));
            params.insert("dest".to_string(), json!(positionals[positionals.len() - 1]));
        }
    } else if verb == "test" {
        if let Some(first) = positionals.first() {
            params.insert("query".to_string(), json!(first));
        }
    } else if verb == "dev" {
        // mysid dev [start|log|status|stop] [--port N] [--lines N] [--wait S]
        if let Some(first) = positionals.first() {
            params.insert("query".to_string(), json!(first));
        }
    } else if verb == "debug" {
        // Boolean flags must not swallow the keyword that follows them (`debug --all usb`).
        for k in ["all", "new", "raw"] {
            if let Some(Value::String(s)) = params.get(k).cloned() {
                positionals.insert(0, s);
                params.insert(k.to_string(), json!(true));
            }
        }
        // Single-dash spellings: `-raw` and `-20` (seconds).
        positionals.retain(|p| {
            if p == "-raw" {
                params.insert("raw".to_string(), json!(true));
                false
            } else if let Some(n) = p.strip_prefix('-').and_then(|n| n.parse::<u64>().ok()) {
                params.insert("seconds".to_string(), json!(n));
                false
            } else {
                true
            }
        });
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "new" {
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "skill" {
        if let Some(Value::String(s)) = params.get("global").cloned() {
            positionals.insert(0, s);
            params.insert("global".to_string(), json!(true));
        }
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "checkpoint" || verb == "diff" || verb == "restore" {
        // `restore 3 --yes`: the flag must not swallow the target that follows it.
        if let Some(Value::String(s)) = params.get("yes").cloned() {
            positionals.push(s);
            params.insert("yes".to_string(), json!(true));
        }
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "commit" {
        // mysid commit "message"   |   mysid commit --message "m" [--branch main] [--paths a.kt b.kt]
        let flag_message = matches!(params.get("message"), Some(Value::String(s)) if !s.is_empty());
        if flag_message {
            // Bare words after --message / --paths are more paths: `--paths a.kt b.kt`.
            if !positionals.is_empty() {
                let mut paths: Vec<String> = match params.get("paths") {
                    Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
                    Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(String::from)).collect(),
                    _ => Vec::new(),
                };
                paths.extend(positionals.iter().cloned());
                params.insert("paths".to_string(), json!(paths));
            }
        } else if !positionals.is_empty() {
            params.insert("message".to_string(), json!(positionals.join(" ")));
        }
    } else if verb == "history" {
        // `history`, `history 10`, `history --past 10`, `history -past 10`, `history -10`
        let mut it = positionals.iter().peekable();
        while let Some(p) = it.next() {
            let n = if p == "-past" { it.next().and_then(|v| v.parse::<u64>().ok()) } else { p.trim_start_matches('-').parse::<u64>().ok() };
            if let Some(n) = n {
                params.insert("past".to_string(), json!(n));
            }
        }
    } else if verb == "graph" {
        // `--flow <symbol>` would otherwise swallow the symbol as the flag's value.
        if let Some(Value::String(sym)) = params.get("flow").cloned() {
            positionals.insert(0, sym);
            params.insert("flow".to_string(), json!(true));
        }
        if params.contains_key("h") || (positionals.len() == 1 && (positionals[0] == "help" || positionals[0] == "-h")) {
            params.insert("help".to_string(), json!(true));
        } else if let Some(symbol) = positionals.first() {
            params.insert("symbol".to_string(), json!(symbol));
        }
    } else if verb == "read" {
        if positionals.len() > 1 {
            let second_is_number = positionals[1].parse::<i64>().is_ok() || positionals[1].eq_ignore_ascii_case("end");
            if !second_is_number {
                params.insert("files".to_string(), json!(positionals));
            } else {
                params.insert("path".to_string(), json!(positionals[0]));
                if let Ok(offset) = positionals[1].parse::<i64>() {
                    params.insert("offset".to_string(), json!(offset));
                }
                if positionals.len() > 2 {
                    if let Ok(limit) = positionals[2].parse::<i64>() {
                        params.insert("limit".to_string(), json!(limit));
                    }
                }
            }
        } else if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
        }
    } else {
        if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
            if positionals.len() > 1 {
                if let Ok(offset) = positionals[1].parse::<i64>() {
                    params.insert("offset".to_string(), json!(offset));
                }
            }
            if positionals.len() > 2 {
                if let Ok(limit) = positionals[2].parse::<i64>() {
                    params.insert("limit".to_string(), json!(limit));
                }
            }
        }
    }

    (verb, Value::Object(params))
}
