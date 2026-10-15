#![recursion_limit = "512"]
mod adapters;
mod index;
pub mod parsers;
mod protocol;
mod textindex;
mod tools;

use protocol::{JsonRpcError, JsonRpcErrorResponse, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tools::workspace::{parse_file_uri, WorkspaceManager};

/// Every command name `mysid` accepts (aliases included): the source for "did you mean" hints.
const KNOWN_COMMANDS: &[&str] = &[
    "read", "peek", "search", "graph", "patch", "patch_batch", "replace", "edit", "create_file", "write", "append",
    "list_dir", "ls", "dir", "listdir", "mkdir", "rename", "mv", "delete", "rm", "file", "exec", "run",
    "lint", "check", "symbols", "overview", "signatures", "build", "release", "aab", "install",
    "apply", "list", "commit", "history", "checkpoint", "diff", "restore", "skill", "new", "debug", "dev", "deps", "test", "status", "branch", "copy", "cp", "publish", "bootstrap", "doctor", "tree", "init", "vcs", "git", "purge", "msvc", "cl", "emsdk", "wasm", "android", "devices", "wifi", "connect_wifi", "inspect_symbol", "set", "set_workspace", "setworkspace",
    "get_workspace", "getworkspace", "help",
];

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            cur.push((prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Up to three known commands the user probably meant: prefixes of what they typed, or close spellings.
fn suggest_commands(unknown: &str) -> Vec<&'static str> {
    let u = unknown.trim().trim_start_matches('-').to_ascii_lowercase();
    if u.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(usize, &'static str)> = KNOWN_COMMANDS
        .iter()
        .filter_map(|c| {
            let d = edit_distance(&u, c);
            let prefix = u.len() >= 3 && (c.starts_with(u.as_str()) || u.starts_with(c));
            if prefix { Some((0, *c)) } else if d <= 2 { Some((d, *c)) } else { None }
        })
        .collect();
    scored.sort();
    // Only the closest matches: `bild` means `build`, not also the next-nearest spelling.
    let best = scored.first().map_or(0, |s| s.0);
    scored.into_iter().filter(|s| s.0 == best).map(|(_, c)| c).take(3).collect()
}

/// Process exit status for CLI use: 2 for an unknown command, 1 when the tool reported a failure
/// (`ERROR: ...` text, or JSON with `"success": false`), 0 otherwise. Scripts and `&&` chains
/// rely on this; the text output is unchanged.
fn cli_exit_code(output: &str, unknown_command: bool) -> i32 {
    if unknown_command {
        return 2;
    }
    let t = output.trim_start();
    if t.starts_with("ERROR") {
        return 1;
    }
    match serde_json::from_str::<Value>(t) {
        Ok(v) if v.get("success").and_then(|s| s.as_bool()) == Some(false) => 1,
        _ => 0,
    }
}

/// Prints files by their shortest unique name (`Foo.kt`, or `ui/Foo.kt` when two share a name), which
/// every command accepts back. Keeps output small, readable, and teaches the model to use short names.
fn shorten_paths(output: String, params: &Value, default_root: Option<&str>) -> String {
    if !output.contains('/') || output.starts_with("ERROR") {
        return output;
    }
    match tools::workspace::resolve_existing_root(params, "workspace_root", default_root) {
        Ok(root) => tools::paths::Shortener::new(Path::new(&root)).apply(&output),
        Err(_) => output,
    }
}

fn dispatch_method(method: &str, params: &Value, default_root: Option<&str>, workspace_mgr: &Arc<Mutex<WorkspaceManager>>) -> String {
    if params.get("help").and_then(|v| v.as_bool()).unwrap_or(false) {
        if let Some(help_text) = get_tool_help(method) {
            return help_text.to_string();
        }
    }
    if method == "help" {
        if let Some(tool) = params.get("tool").and_then(|v| v.as_str()) {
            if let Some(help_text) = get_tool_help(tool) {
                return help_text.to_string();
            }
        }
        return get_root_help().to_string();
    }

    match method {
        // ── Session management ────────────────────────────────────────────────
        "set_workspace" | "setworkspace" | "set" => {
            let mut path_val = params
                .get("path")
                .or_else(|| params.get("workspace"))
                .or_else(|| params.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            // `mysid set --all .` : the value-less flag took the folder; give it back.
            if let Some(Value::String(s)) = params.get("all") {
                if path_val.is_empty() && !s.is_empty() && s != "true" && s != "false" {
                    path_val = s.trim().to_string();
                }
            }

            let alias = params.get("alias").and_then(|v| v.as_str());

            if path_val.is_empty() {
                json!({
                    "success": false,
                    "error": "set needs a folder: mysid set <path>  (or a name from the registry)"
                })
                .to_string()
            } else {
                let mut mgr = workspace_mgr.lock().unwrap();
                match mgr.switch_workspace(&path_val) {
                    Ok(resolved_path) => {
                        if let Some(a) = alias {
                            mgr.register_and_activate(&resolved_path, Some(a));
                        }
                        let known = mgr.get_known_workspaces();
                        drop(mgr);

                        // Per-project scratch dir for logs (build.log); best effort.
                        let _ = tools::build::log::ensure_dir(Path::new(&resolved_path));

                        // Automatically list root directory entries (L1 depth) to save LLM roundtrips
                        let list_params = json!({
                            "workspace_root": &resolved_path,
                            "max_depth": 1
                        });
                        let list_raw = tools::file::execute_list_dir(&list_params, Some(&resolved_path));
                        let entries = serde_json::from_str::<Value>(&list_raw)
                            .ok()
                            .and_then(|v| v.get("entries").cloned())
                            .unwrap_or_else(|| json!([]));

                        let show_all = matches!(params.get("all"), Some(Value::Bool(true))) || matches!(params.get("all"), Some(Value::String(s)) if !s.is_empty() && s != "false");
                        let summary = tools::scanner::orient::workspace_map(Path::new(&resolved_path), show_all);
                        json!({
                            "success": true,
                            "workspace": resolved_path,
                            "output": format!(
                                "Workspace set to '{}' (for commands run from this folder and below; other folders and agents are not affected)
{}",
                                resolved_path, summary
                            ),
                            "message": format!("Workspace set to '{}'", resolved_path),
                            "known_workspaces": known,
                            "entries": entries
                        })
                        .to_string()
                    }
                    Err(e) => {
                        let known = mgr.get_known_workspaces();
                        json!({
                            "success": false,
                            "error": e,
                            "known_workspaces": known
                        })
                        .to_string()
                    }
                }
            }
        }

        "get_workspace" | "getworkspace" => {
            let mgr = workspace_mgr.lock().unwrap();
            let current = mgr.get_current().or_else(|| default_root.map(|s| s.to_string()));
            let known = mgr.get_known_workspaces();
            let resolved_from = mgr.resolved_from;
            drop(mgr);

            let entries = if let Some(ref root) = current {
                let list_params = json!({
                    "workspace_root": root,
                    "max_depth": 1
                });
                let list_raw = tools::file::execute_list_dir(&list_params, Some(root));
                serde_json::from_str::<Value>(&list_raw)
                    .ok()
                    .and_then(|v| v.get("entries").cloned())
                    .unwrap_or_else(|| json!([]))
            } else {
                json!([])
            };

            let summary = match current.as_deref() {
                Some(root) => tools::scanner::orient::workspace_map(Path::new(root), false),
                None => String::new(),
            };
            let output = format!(
                "workspace: {}
chosen from: {}
{}",
                current.as_deref().unwrap_or("(none)"),
                resolved_from.unwrap_or("MCP session or set_workspace"),
                summary
            );
            json!({
                "success": true,
                "workspace": current,
                "output": output,
                "resolved_from": resolved_from,
                "known_workspaces": known,
                "entries": entries
            })
            .to_string()
        }

        // ── File / project tools ──────────────────────────────────────────────
        "help" => get_root_help().to_string(),
        "search" => shorten_paths(tools::scanner::search::execute_search(params, default_root), params, default_root),
        "read" | "peek" => shorten_paths(tools::scanner::read::execute_read(params, default_root), params, default_root),
        "graph" => shorten_paths(tools::graph::execute_graph(params, default_root), params, default_root),
        "patch" => tools::patch::execute_patch(params, default_root),
        "patch_batch" => tools::patch::execute_patch_batch(params, default_root),
        "replace" => tools::replace::execute_replace(params, default_root),
        "create_file" => tools::file::execute_create_file(params, default_root),
        "write" => tools::file::execute_write(params, default_root),
        "append" => tools::file::execute_append(params, default_root),
        "edit" => tools::file::execute_edit(params, default_root),
        "list" | "list_dir" | "ls" | "dir" | "listdir" => tools::file::execute_list_dir(params, default_root),
        "mkdir" => tools::file::execute_mkdir(params, default_root),
        "rename" | "mv" => tools::file::execute_rename(params, default_root),
        "delete" | "rm" => tools::file::execute_delete(params, default_root),
        "file" => tools::file::execute_file(params, default_root),
        "exec" | "run" => tools::build::exec::execute_exec(params, default_root),
        "lint" => tools::build::lint::execute_lint(params, default_root),
        "check" => tools::check::execute_check(params, default_root),
        "symbols" => tools::scanner::symbols::execute_symbols(params, default_root),
        "overview" | "signatures" => shorten_paths(tools::scanner::overview::execute_overview(params, default_root), params, default_root),
        "build" => tools::build::execute_build(params, default_root),
        "release" | "aab" => tools::release::execute_release(params, default_root),
        "install" => tools::release::execute_install(params, default_root),
        "debug" => tools::build::debug::execute_debug(params, default_root),
        "dev" => tools::dev::execute_dev(params, default_root),
        "test" => tools::test_run::execute_test(params, default_root),
        // Version control: one registry (tools/vcs) maps bootstrap, commit, status, branch, history, publish,
        // checkpoint, diff and restore to their handlers, and adds the next-step hint for the state they leave.
        name if tools::vcs::find(name).is_some() => {
            let out = tools::vcs::dispatch(name, params, default_root).unwrap_or_default();
            if name == "diff" { shorten_paths(out, params, default_root) } else { out }
        }
        "vcs" => json!({ "success": true, "output": tools::vcs::overview() }).to_string(),
        "copy" | "cp" => tools::file::execute_copy(params, default_root),
        "deps" => tools::deps::execute_deps(params, default_root),
        "doctor" => tools::doctor::execute_doctor(params, default_root),
        "tree" => tools::scanner::orient::execute_tree(params, default_root),
        "skill" => tools::skill::execute_skill(params, default_root),
        "new" => tools::scaffold::execute_new(params, default_root),
        "apply" => tools::apply::execute_apply(params, default_root),

        "android" => tools::android::execute_android(params, default_root),
        "devices" => {
            let mut p = params.clone();
            if let Some(obj) = p.as_object_mut() {
                obj.insert("action".to_string(), json!("devices"));
            }
            tools::android::execute_android(&p, default_root)
        }
        "wifi" | "connect_wifi" => {
            let mut p = params.clone();
            if let Some(obj) = p.as_object_mut() {
                obj.insert("action".to_string(), json!("connect_wifi"));
            }
            tools::android::execute_android(&p, default_root)
        }
        "inspect_symbol" => shorten_paths(tools::scanner::read::execute_read(params, default_root), params, default_root),
        _ => {
            let hint = suggest_commands(method);
            let did_you_mean = if hint.is_empty() { String::new() } else { format!(" Did you mean: {}?", hint.join(", ")) };
            format!("ERROR: No such command: '{}'.{} Run 'mysid help' for all commands.", method, did_you_mean)
        }
    }
}

fn coerce_value(val: &str) -> Value {
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
fn parse_exec_args(rest: &[String]) -> Value {
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
fn read_text_input(path: &str, one_line: bool) -> String {
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

fn parse_cli_args(args: &[String]) -> (String, Value) {
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

/// `--head N`, `--tail N`, `--grep <regex>`, `--grep-v <regex>` on any command: trims the printed output, so nothing
/// needs piping through head, tail or grep. Smart case: a lower-case pattern ignores case. Not for MCP calls.
struct OutputFilter {
    head: Option<usize>,
    tail: Option<usize>,
    grep: Option<regex::Regex>,
    grep_v: Option<regex::Regex>,
    described: Vec<String>,
}

/// What `--errors` keeps: error and fatal lines, MSVC linker/compiler codes, undefined references, failures, exceptions,
/// and mysid's own `[exit N]` / `[timed out` lines.
const ERRORS_PATTERN: &str = r"\berror\b|\bfatal\b|\bLNK\d+|undefined reference|cannot (open|find)|\bfailed\b|exception|warning C\d+|^\[(exit|timed out)";

fn smart_regex(pattern: &str) -> regex::Regex {
    let ignore_case = !pattern.chars().any(|c| c.is_uppercase());
    regex::RegexBuilder::new(pattern)
        .case_insensitive(ignore_case)
        .build()
        .unwrap_or_else(|_| regex::RegexBuilder::new(&regex::escape(pattern)).case_insensitive(ignore_case).build().unwrap())
}

impl OutputFilter {
    fn take(params: &mut Value) -> Result<OutputFilter, String> {
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

    fn apply(&self, text: &str) -> String {
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

fn format_output(raw: &str) -> String {
    if let Ok(val) = serde_json::from_str::<Value>(raw) {
        if let Some(out_str) = val.get("output").or_else(|| val.get("stdout")).and_then(|v| v.as_str()) {
            if !out_str.trim().is_empty() {
                return out_str.to_string();
            }
        }
        if let Some(entries) = val.get("entries").and_then(|v| v.as_array()) {
            let mut lines: Vec<String> = Vec::with_capacity(entries.len() + 3);
            if let Some(msg) = val.get("message").and_then(|v| v.as_str()) {
                lines.push(msg.to_string());
                lines.push("".to_string());
            }
            let folders = entries.iter().filter(|e| e.get("is_dir").or_else(|| e.get("is_directory")).and_then(|v| v.as_bool()).unwrap_or(false)).count();
            lines.push(format!("# {} entries: {} folders (D), {} files (F)", entries.len(), folders, entries.len() - folders));
            for e in entries {
                let is_dir = e.get("is_dir").or_else(|| e.get("is_directory")).and_then(|v| v.as_bool()).unwrap_or(false);
                let tag = if is_dir { "D" } else { "F" };
                let p = e.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                lines.push(format!("{} {}", tag, p));
            }
            return lines.join("\n");
        }
        if let Some(symbols) = val.get("symbols").and_then(|v| v.as_array()) {
            let lines: Vec<String> = symbols.iter().map(|s| {
                let stype = s.get("symbol_type").and_then(|v| v.as_str()).unwrap_or("SYM");
                let sname = s.get("symbol_name").and_then(|v| v.as_str()).unwrap_or("?");
                let line = s.get("start_line").and_then(|v| v.as_i64()).unwrap_or(0);
                format!("  [{:8}] {} line {}", stype.to_uppercase(), sname, line)
            }).collect();
            return lines.join("\n");
        }
        if let Some(diags) = val.get("diagnostics").and_then(|v| v.as_array()) {
            if diags.is_empty() {
                return val.get("summary").and_then(|v| v.as_str()).unwrap_or("No errors.").to_string();
            }
            let lines: Vec<String> = diags.iter().map(|d| {
                let file = d.get("file").and_then(|v| v.as_str()).unwrap_or("?");
                let line = d.get("line").and_then(|v| v.as_i64()).unwrap_or(0);
                let msg = d.get("message").and_then(|v| v.as_str()).unwrap_or("");
                format!("  {}:{}: {}", file, line, msg)
            }).collect();
            return lines.join("\n");
        }
        if let Some(msg) = val.get("message").and_then(|v| v.as_str()) {
            return msg.to_string();
        }
        if let Some(bytes) = val.get("bytes_written").and_then(|v| v.as_u64()) {
            if let Some(p) = val.get("path").and_then(|v| v.as_str()) {
                return format!("File written: {} ({} bytes)", p, bytes);
            }
        }
        if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
            // "NOT PATCHED - ..." is already a clear first line.
            return if err.starts_with("NOT ") { err.to_string() } else { format!("ERROR: {}", err) };
        }
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| raw.to_string())
    } else {
        raw.to_string()
    }
}


/// `mysid help --claude`: the command list followed by a table from Claude Code's own tools to mysid commands.
fn client_help(client: &str) -> Option<String> {
    let section = match client {
        "claude" => r#"
For Claude Code: your tools, in mysid
=====================================
Run mysid through the Bash tool, from inside the project folder. Use file NAMES (Foo.kt), not paths.

  Your tool                         mysid
  --------------------------------  ---------------------------------------------------------------
  Read(file_path, offset, limit)    mysid read Foo.kt#L40-90        (several slices in ONE call)
                                    mysid read Foo.kt               a short file whole; a long source file: its outline + the command for a slice
                                    mysid read SymbolName           definition and call sites
                                    mysid overview Foo.kt           outline only: types and signatures with line numbers
  Grep(pattern, glob, -C)           mysid search "pattern" --types kt --context 1
  Glob(pattern)                     mysid list src/ui      every entry of a folder
                                    mysid graph            where the code lives (one call, with counts);  mysid graph src/ui = a folder's map
  Edit(old_string, new_string)      mysid patch Foo.kt "old" "new"            [--replace_all] [--dry-run]
    ...text with quotes, $, `, \     mysid patch Foo.kt --old-file a.txt --new-file b.txt
  MultiEdit(edits[])                mysid apply --file edits.txt      (SEARCH/REPLACE blocks, many files, all-or-nothing)
                                    mysid apply <<'EOF' ... EOF       (the quotes around EOF are required)
  Write(file_path, content)         mysid write src/New.kt --file content.txt
                                    mysid write src/New.kt <<'EOF' ... EOF    (refuses to replace a file: add --overwrite)
  (append to a file)                mysid append Notes.md --file more.txt   [--create]
  Bash(command)                     mysid exec "cmd"                   (30 s; --timeout N)
  Bash: build / test                mysid build   (errors only; full log in .mysid/build.log)
                                    mysid test [name]   (one line when green, failing tests when red)
  Bash: git status / cp             mysid status   (branch, changed files)    mysid copy <src>... <dest>
  Bash: dev server                  mysid dev   (background, returns the URL)   mysid dev log | stop

Edit semantics match yours: the text must match exactly once or nothing changes (NOT PATCHED / NOT APPLIED names why).
CRLF files stay CRLF. Trim any output with --head N, --tail N, --grep <regex>, --grep-v <regex> instead of piping.

Safe loop:  mysid checkpoint "label"  ->  edits  ->  mysid build  ->  mysid diff  ->  mysid commit "msg"
            not happy: mysid restore (preview), mysid restore --yes
More: mysid skill base   (the full workflow guide)
"#,
        "codex" | "antigravity" | "gemini" => {
            return Some(format!(
                "{}\n\n(`mysid help --{}`: the guide for this client is not written yet. Its native edit format is being added to `mysid apply`; until then the Claude Code section above shows the commands: `mysid help --claude`.)",
                get_root_help(),
                client
            ))
        }
        _ => return None,
    };
    Some(format!("{}\n{}", get_root_help(), section))
}

/// What `mysid help` shows: the seven commands that cover most work. `mysid help all` has every command.
fn get_root_help() -> &'static str {
    r#"mysid: code intelligence for LLM agents. Eleven commands cover most work:

   1. mysid set .                      pick the project; prints its source root, language, entry and the folders to look at
   2. mysid list <folder>              what is in a folder: sub-folders ranked by code, code files with line counts
   3. mysid overview <file.ext>        classes, functions and signatures with line numbers (no code)
   4. mysid read <file.ext>#L10-60     the code snippet (a SymbolName instead of a file works too)
   5. mysid search "text" [--types kt,ts] [--regex] [--context 1] [--group] [--count] [--max N] [--offset N]
                                       where is it? Shows 15 hits; if cut, the last line gives the --offset N for the next page (--max N shows more). --count = matches per file; --all also searches md/txt
   6. mysid graph <symbol> [--depth N]   callers and callees; --depth N = N hops (max 5, default 1); a file name: what it calls and who calls it
   7. change code, any of three:
        mysid patch <file> --stdin [--show]   exact old text. ALWAYS give code via --stdin (old text, a line with only =======, new text) or --old-file a --new-file b: quotes, $ and backslashes then never matter. --show = also print the removed (-) and added (+) lines, so you need not re-read the file
        mysid edit <file>#L10-20 "new text"   you know the line numbers (from read) [--insert-after | --insert-before | --delete | --expect "text"]
        mysid replace "old" "new"             the same change in many places        [--dry-run first]
   8. mysid patch_batch <file> --stdin [--show]   several replacements in ONE file, all or nothing: repeat <<<<<<< SEARCH / old text / ======= / new text / >>>>>>> REPLACE on stdin
   9. mysid file new dir/sub-dir/file.ext --stdin   create a new file, content on stdin (folders are made). If the file exists it stops with "already exists": add --overwrite to replace it
  10. mysid build                      build; errors only (the full log is .mysid/build.log)
  11. mysid get_workspace              CONTINUING earlier work? Run this first: which project this folder uses, and why (then set . if it is the wrong one)

Flow: (continuing earlier work: get_workspace) set . -> list <folder> -> overview <file> -> read <file>#L10-60 -> search "text" -> graph <symbol> -> patch | edit | replace | patch_batch | file new -> build.
One command in detail: mysid help <command>.   The other commands: mysid help remaining.
"#
}

fn get_full_help() -> &'static str {
    r#"mysid: code intelligence for LLM agents (command line and MCP server)

THE USUAL FLOW
   1. mysid set <path>                 optional: mysid finds the project from the folder you run it in
   2. mysid list <folder>              what is in a folder (every entry).  mysid graph = where the code lives, in one call
   3. mysid overview <file>            the outline: classes, functions, signatures with line numbers (no code)
   4. mysid read <file | symbol>       the code: Foo.kt#L121-220 (a slice), a SymbolName; a bare long file gives its outline; several in ONE call
   5. mysid search "terms"             where is it?        [--types kt,ts] [--context 1] [--regex] [--group] [--all = also md/txt]
   6. mysid graph <symbol | File.kt>   callers and callees (JNI bridges included); a file: what it calls and who calls it  [--depth 2] [--flow]
   7. mysid apply                      edit: many changes at once from SEARCH/REPLACE blocks on stdin or --file
   8. mysid patch <file> "old" "new"   edit: one exact replacement  [--old-file a --new-file b | --stdin for big text]
   9. mysid file new <path>            new file; content from stdin (--stdin) or --from <file>
                                       (mysid write <path> does the same but never overwrites; mysid append <path> adds)
  10. mysid build                      verify: errors only. mysid check = fast TypeScript type-check
  11. mysid help --claude              this flow in Claude Code's own tool names  (--codex, --antigravity: coming)
  12. mysid skill android              the Android workflow guide
  13. mysid skill react                React project: components, hooks, tsc, lint, dev server, Netlify release
  14. mysid skill base                 stack unknown: the general workflow
  15. mysid file <action>              more file commands: new, copy, rename, delete, mkdir, list  ('file --help')

SAFE EDITING
  mysid checkpoint "label"  ->  edits  ->  mysid build  ->  mysid diff  ->  mysid commit "message"
  not happy: mysid restore (preview) / mysid restore --yes.   mysid history = the last commits.

MORE
  mysid replace <old> <new>            search-and-replace across files  [--types] [--dry-run]
  mysid patch_batch --json <file>      atomic multi-file patches
  mysid test [name]                    run the tests: one line when green, failing tests with messages when not
  mysid bootstrap                      git init only (nothing staged or committed); --gitignore writes a .gitignore for the stack
  mysid vcs                            the version-control workflow: bootstrap -> .gitignore -> commit -> status -> publish
  mysid git purge  (or: mysid purge)   remove git from the project (preview; --yes does it; irreversible, ask the owner)
  mysid status                         git: branch, ahead/behind, changed files   (mysid branch lists branches)
  mysid publish <repo-url>             first upload to GitHub: shows the plan; add --yes to push (ask the owner first)
  mysid copy <src>... <dest>           copy files or folders (no overwrite unless --overwrite)
  Any command takes --head N, --tail N, --grep <regex>, --grep-v <regex> to trim its output
  mysid dev                            start the dev server in the background, get its URL; dev log | status | stop
  mysid deps                           npm packages the code imports that package.json is missing
  mysid exec <command>                 run a shell command  [--timeout N] [--out file] [--shell cmd|powershell|bash] [--errors] [-- ends mysid flags]
  mysid msvc <command>                 run with the Visual Studio C++ environment (vcvars64) set up;  mysid emsdk <command>: the Emscripten SDK
  mysid new react <name> [--dir D]     create a React (Vite + TypeScript) project

mysid <command> --help gives the details of any command.
"#
}

/// The commands `mysid help` lists; `mysid help remaining` is everything else.
const SHORT_HELP_COMMANDS: &[&str] = &["set", "list", "overview", "read", "search", "graph", "patch", "edit", "replace", "patch_batch", "file", "build", "get_workspace", "getworkspace", "help"];

/// The Android / device commands: not listed in `mysid help remaining`; `mysid android help` is their one page.
const ANDROID_COMMANDS: &[&str] = &["android", "adb", "devices", "wifi", "connect_wifi", "debug", "release", "aab"];

/// The file operations: not listed in `mysid help remaining`; `mysid file help` is their one page.
const FILE_COMMANDS: &[&str] = &["write", "append", "mkdir", "copy", "cp", "rename", "mv", "delete", "rm", "create_file", "list_dir", "ls", "dir", "listdir"];

/// The reading utilities, with their full syntax: shown at the top of `mysid help remaining`.
const READING_UTILITIES: &str = "UTILITIES: look at the code (the short help has the everyday forms; these are all the options)
  mysid read <file>                      a file up to 150 lines whole; a longer source file gives its outline and the command for a slice
  mysid read <file>#L10-60               a slice, up to 200 lines (600 with --full); File.kt:10-50 also works
  mysid read <slice1> <slice2> ...       several slices in ONE call
  mysid read <Symbol>                    a symbol's definition and its top callers
  mysid tree [folder] [--depth 1-4] [--all]
                                         the folder map: folders with their code-file counts, depth 2 by default; --all adds dot-folders
  mysid overview <file> [<file2> ...]    classes, functions and signatures with line numbers (no code)
  mysid overview <folder> [--depth 1] [--match word] [--format json]
                                         outlines of every file in a folder; --depth 1 = folder names and counts only; --match keeps files
                                         whose path, class or function contains the word
  mysid search \"text\" [--types kt,ts] [--regex] [--case] [--context 1] [--group] [--count] [--max N] [--offset N] [--all] [--vendor]
                                         --count = matching lines per file; --offset N = next page; --max N up to 500; --group = file name once;
                                         --all also searches md/txt; --vendor also searches external/ and third_party/
  mysid symbols <file>                   a flat list of the symbols in one file: [KIND] name line N (cheaper than overview)
Files are named by name only (Foo.kt); add a folder (ui/Foo.kt) only if two files share a name.
";

/// Commands shown in the utilities block instead of the one-line list.
const UTILITY_COMMANDS: &[&str] = &["tree", "symbols"];

/// One line per command that is not in the short help: its name and the first line of its own help page.
fn get_remaining_help() -> &'static str {
    static TEXT: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| {
        let mut lines: Vec<String> = Vec::new();
        let mut seen: Vec<&'static str> = Vec::new();
        let mut without_page: Vec<&str> = Vec::new();
        for name in KNOWN_COMMANDS {
            if SHORT_HELP_COMMANDS.contains(name) || ANDROID_COMMANDS.contains(name) || FILE_COMMANDS.contains(name) || UTILITY_COMMANDS.contains(name) {
                continue;
            }
            match get_tool_help(name) {
                Some(page) => {
                    let first = page.lines().next().unwrap_or("").trim();
                    if seen.contains(&first) {
                        continue;
                    }
                    seen.push(first);
                    let what = first.split_once(": ").map_or(first, |(_, w)| w);
                    lines.push(format!("  {:<14}{}", name, what));
                }
                None => without_page.push(name),
            }
        }
        let mut out = format!("mysid: the commands that are not in `mysid help`\n\n{}\nOTHER COMMANDS ({} with a help page)\n\n{}\n", READING_UTILITIES, lines.len(), lines.join("\n"));
        if !without_page.is_empty() {
            out.push_str(&format!("\nAlso accepted (aliases and small helpers): {}\n", without_page.join(", ")));
        }
        out.push_str("\nFile operations (new, write, append, mkdir, mv, cp, rm, ls): mysid file help\n");
        out.push_str("Android / device commands (devices, wifi, install, debug, release, goto, scenario, keyevent ...): mysid android help\n");
        out.push_str("\nOne command in detail: mysid help <command>.   The commands in the short list: mysid help.\n");
        out
    })
}

fn get_tool_help(tool: &str) -> Option<&'static str> {
    match tool.to_ascii_lowercase().trim_start_matches('-') {
        "remaining" | "rest" | "more" => Some(get_remaining_help()),
        "symbols" => Some("mysid symbols: the symbols of one file, one per line\n\nUsage:\n  mysid symbols <file>      [KIND] name line N, for every class, function and method (file name only: Foo.kt)\n\nCheaper than mysid overview when only the names and line numbers matter; overview adds signatures and parameters.\n"),
        "all" | "full" | "commands" => Some(get_full_help()),
        "replace" => Some(
            "mysid replace: Global search-and-replace refactoring\n\n\
Description:\n  \
  Replaces occurrences of a string across the entire workspace, or within a specific file.\n  \
  Automatically skips binary files, respects .gitignore, and preserves line endings.\n\n\
Usage:\n  \
  mysid replace <old_term> <new_term> [options]         Replace across the entire workspace\n  \
  mysid replace <file> <old_term> <new_term> [options]  Target a specific file\n\n\
Options:\n  \
  --types <ext1,ext2>   Filter by file extensions (e.g. --types kt,cpp,rs)\n  \
  --path <subfolder>    Restrict replacements to a specific subdirectory\n  \
  --dry-run, --preview  Preview changes without modifying files\n\n\
Examples:\n  \
  mysid replace \"StreamResolver\" \"AudioStreamResolver\"\n  \
  mysid replace \"old_api\" \"new_api\" --types kt,java\n  \
  mysid replace \"AudioEngine\" \"NativeAudioEngine\" --path app/src/main/cpp\n  \
  mysid replace \"versionCode = 44\" \"versionCode = 45\" --dry-run\n"
        ),
        "read" => Some(
            "mysid read: the code of a file, or a symbol's definition\n\n\
Description:\n  \
  The code of a file. A range (File.kt#L10-60), --full or an offset always returns code. A bare read of a LONG\n  \
  source file (over 150 lines) returns its outline instead, with the exact command for a slice, so it never floods.\n  \
  The full outline is `mysid overview <file>`; a folder's entries are `mysid list <folder>`.\n\n\
Files are named by file name only (Foo.kt, not app/src/.../Foo.kt): mysid finds them anywhere in the\n  \
  workspace and prints short names too. Add a folder (ui/Foo.kt) only if two files share a name.\n\n\
Usage:\n  \
  mysid read <file>                   A file up to 150 lines whole. A longer source file: its outline (line numbers) and the command for a slice. Other text (logs): the first 120 lines and the command for the next\n  \
  mysid read <file#Lstart-end>        A slice (e.g. File.kt#L10-50 or File.kt:10-50); up to 200 lines, 600 with --full\n  \
  mysid read <slice1> <slice2> ...    Several slices in ONE call (Zero Peeking Rule)\n  \
  mysid read <symbol>                 A symbol's definition and its top callers / call sites\n  \
  mysid read <folder>                 Not a file: the reply points to `mysid list <folder>` and `mysid graph <folder>`\n\n\
Examples:\n  \
  mysid read StreamResolver\n  \
  mysid read MusicService.kt\n  \
  mysid read \"MusicService.kt#L100-150\"\n  \
  mysid read \"AudioEngine.cpp#L40-80\" \"Player.kt#L200-240\"\n"
        ),
        "search" => Some(
            "mysid search: Fast multithreaded codebase grep\n\n\
Description:\n  \
  High-speed regex/string search across the workspace respecting .gitignore.\n  \
  Returns token-lean 1-line hits (Foo.kt:12: snippet); pass those file names to read/patch.\n\n\
Usage:\n  \
  mysid search <query> [options]\n\n\
Options:\n  \
  --types <ext1,ext2>   Filter by file extensions (e.g. --types kt,cpp)\n  \
  --max <n>             Maximum results to return (default: 15, max: 500)\n  \
  --case                Case-sensitive search (default: false)\n  \
  --regex               Treat query as regular expression (default: false)
  --count               Per-file summary: how many matching lines in each file (most first), no hit text
  --offset <n>          Skip the first n matches (the next page of a long result)
  --group               Group hits by file (file name once, then '  line: text'): fewer tokens
  --all                 Also search md, txt and rst files (left out by default so prose does not drown code hits)
  --vendor              Also search external/, third_party/, vendor/ (copied libraries: left out by default)\n  \
  --context <n>         Show n context lines around matches (max: 2)\n\n\
Examples:\n  \
  mysid search \"pref_bluetooth\"\n  \
  mysid search \"resolveAudioStream\" --types kt\n  \
  mysid search \"Java_com_hypersonus\" --types cpp --max 20\n"
        ),
        "mkdir" => Some(
            "mysid mkdir: Recursive directory creation\n\n\
Description:\n  \
  Creates a new directory and all necessary parent directories in the workspace.\n  \
  Never fails with missing parent path errors.\n\n\
Usage:\n  \
  mysid mkdir <path>\n\n\
Examples:\n  \
  mysid mkdir app/src/main/assets/presets\n  \
  mysid mkdir tools/cache\n"
        ),
        "patch" => Some(
            "mysid patch: Surgical search-and-replace modification\n\n\
Description:\n  \
  Safely replaces exact target string in a file with new content.\n  \
  Matches exact text, then text differing only in CRLF/LF line endings, then text ignoring surrounding\n  whitespace; everything outside the match stays byte-identical. Refuses (changing nothing) if the target is\n  not found or matches more than once: the error gives the match count and line numbers. Add context to make\n  it unique, or pass --replace_all.\n\n\
Usage:\n  \
  mysid patch <file> <old_string> <new_string> [--dry-run] [--replace_all] [--show]\n  \
  mysid patch <file> --old-file a.txt --new-file b.txt   text from files (no shell quoting)\n  \
  mysid patch <file> --stdin           stdin: the old text, a line with only =======, the new text (no markers);\n                                       or SEARCH/REPLACE blocks (see mysid apply)\n\n\
Examples:\n  \
  mysid patch build.gradle.kts \"versionCode = 44\" \"versionCode = 45\"   (file name only; ui/Foo.kt if two share a name)\n"
        ),
        "patch_batch" => Some(
            "mysid patch_batch: several replacements in ONE file, in one call\n\n\
Usage:\n  \
  mysid patch_batch <file> --stdin              snippets on stdin, applied in order, ALL or NOTHING [--show] [--dry-run] [--replace_all]\n  \
  mysid patch_batch <file> --file snippets.txt  the same from a file\n  \
  mysid patch_batch --json batch.json           several files from a JSON list (applied in order; stops at the first failure,\n                                                earlier patches stay: use apply for all-or-nothing across files)\n\n\
Each snippet is the old text and the new text between markers (the text is sent on stdin, so quotes, $ and backslashes never matter):\n\n\
<<<<<<< SEARCH\n\
old text 1\n\
=======\n\
new text 1\n\
>>>>>>> REPLACE\n\
<<<<<<< SEARCH\n\
old text 2\n\
=======\n\
new text 2\n\
>>>>>>> REPLACE\n\n\
A snippet whose old text is missing, or matches more than once, stops everything with NOT APPLIED and names the snippet; no file is touched.\n\
--show lists the removed (-) and added (+) lines of every snippet after writing, so the file need not be read again.\n\
Several files in one call: mysid apply (put the file name on the line before each <<<<<<< SEARCH).\n"
        ),
        "file" => Some(
            "mysid file: file operations, by what they do\n\n\
CREATE\n  \
  mysid file new <path> --stdin          new file, content on stdin (folders are made). If the file exists it stops with\n                                         \"already exists\": add --overwrite to replace it  [also: --from <file>, or the text as an argument]\n  \
  mysid write <path> --stdin             the same, and never replaces an existing file\n  \
  mysid append <path> --stdin            add text to the end of a file, on its own line  [--create makes the file when missing]\n  \
  mysid file mkdir <path>                a folder, with its parents\n\n\
MOVE / COPY\n  \
  mysid file mv <old> <new>              rename or move a file or folder         (also: rename, move)\n  \
  mysid file cp <src>... <dest>          copy files or folders, many at once; --overwrite replaces   (also: copy)\n\n\
REMOVE\n  \
  mysid file rm <path> [--recursive]     delete a file, or a folder with --recursive   (also: delete, remove)\n\n\
LOOK\n  \
  mysid file ls [path] [-L<depth>]       what is in a folder, same as mysid list   (also: list, dir)\n\n\
Examples:\n  \
  mysid file new config/settings.json --stdin\n  \
  mysid file mv old_name.kt new_name.kt\n  \
  mysid file cp ../other/src src/vendor --overwrite\n  \
  mysid file rm temp_cache --recursive\n\n\
Changing the inside of a file is not here: mysid patch, edit, replace, patch_batch (see mysid help).\n"
        ),
        "graph" => Some(
            "mysid graph: callers and callees of a symbol

Usage:
    mysid graph <symbol>                  Callers + symbol + callees (1 hop)
    mysid graph <symbol> --depth N        Callers and callees up to N hops (max 5)
    mysid graph <symbol> --flow           Same data drawn as UPSTREAM / TARGET / DOWNSTREAM trees
    mysid graph <symbol> --flow --depth 2 Trees, 2 hops
    mysid graph File.kt                   A file: what it defines, which files it calls into (JNI included), which call it
    mysid graph [dir]                     Map of the workspace or of a directory

<symbol> is a function, class, or Class.method (also file:Name when a name is ambiguous).

Examples:
    mysid graph useDarkMode
    mysid graph StreamResolver --flow --depth 2
    mysid graph src/Pages
"
        ),
        "build" => Some(
            "mysid build: Build the project (the stack is detected)\n\n\
Description:\n  \
  Detects the stack (Gradle/Android, Cargo, npm, Python, or a project run.bat) and builds it in a\n  \
  background worker. Prints each stage as it starts, then a short result: errors only when it fails,\n  \
  warnings grouped by message when it succeeds. The full output is always in .mysid/build.log.\n\n\
Usage:\n  \
  mysid build                Build; at a terminal waits for the result; piped (LLM) waits up to 20s, then reports progress and keeps running\n  \
  mysid build status         Progress, or the final report (waits up to 20s for a change)\n  \
  mysid build stop           Cancel the running build\n  \
  mysid build --fix-stale    Gradle: a moved project leaves caches (app/.cxx) naming the old folder, or local.properties names a missing SDK; the build stops with the cause. --fix-stale deletes the stale caches and builds, --ignore-stale builds anyway\n  \
  mysid build clean          Clean, then rebuild (Gradle)\n  \
  mysid build --wait 600     Wait up to 600s before returning a progress line (also for status)\n  \
  mysid build --release      Release profile (Cargo)\n\n\
Examples:\n  \
  mysid build\n  \
  mysid build status --wait 120\n"
        ),
        "release" | "aab" => Some(
            "mysid release: production release (Android bundle, or web deploy)\n\n\
Web project (package.json): runs the build script, then `netlify deploy --prod`. Needs netlify-cli, a login and\n\
`netlify link` once. Progress is a few timed lines; success prints RELEASE OK and the URL, failure only the errors\n\
and the failed step. Everything is in .mysid/release.log (follow live: Get-Content .mysid\\release.log -Wait).\n\n\
Android project:\n  \
  Raises versionCode by 1 (build.gradle.kts / build.gradle / manifest), then runs bundleRelease in the\n  \
  background with progress, and verifies app-release.aab and the R8 mapping.txt. If the build fails or is\n  \
  cancelled the versionCode is restored; if it succeeds the new number stays. A release already running\n  \
  is attached to, not bumped again. Each step is reported as a stage: preflight (JDK, signing env), version bump, Gradle startup, the build stages, then verifying (bundle zip, signature, R8 mapping). Uses the signing environment variables your build.gradle reads.\n\n\
Usage:\n  \
  mysid release                  Bump, build, and wait up to 20s for the result\n  \
  mysid build status             Progress, or the final report (waits up to 20s)\n  \
  mysid build stop               Cancel and restore versionCode\n  \
  mysid release --wait 600       Wait longer before returning a progress line\n\n\
Example output:\n  \
  RELEASE OK (Android, 4m10s) - versionCode 37 -> 38, 660 tasks (31 executed), 2 warning(s)\n  \
  AAB: app/build/outputs/bundle/release/app-release.aab (28.4 MB)\n  \
  R8 mapping: present\n"
        ),
        "install" => Some(
            "mysid install: put the built result where it is used\n\n\
Description:\n  \
  Android project: finds the latest built debug APK and deploys it to the device via ADB.\n  \
  Rust (Cargo) project: builds the release binary and installs it to %LOCALAPPDATA%\\mysid\\bin (~/.local/bin elsewhere).\n  \
  A running copy is moved aside to <name>.exe.old, so installing works while the MCP server is up.\n  \
  Node project: npm / yarn / pnpm install.\n\n\
Usage:\n  \
  mysid install\n\n\
Examples:\n  \
  mysid install\n"
        ),
        "debug" => Some(tools::build::debug::HELP),
        "dev" => Some(tools::dev::HELP),
        "test" => Some(tools::test_run::HELP),
        "status" => Some("mysid status: what changed, in git terms\n\nUsage:\n  mysid status     branch, upstream (ahead/behind), changed files (staged / unstaged / untracked), last commit\n  mysid branch     local branches with their last commit\n\nRead only. Secret-looking files are marked: mysid commit leaves them out.\n"),
        "branch" => Some("mysid branch: local branches with their last commit (read only). mysid status shows the changed files.\n"),
        "publish" => Some("mysid publish: the first upload of the project to GitHub (or any git remote)\n\nUsage:\n  mysid publish <repo-url>          PREVIEW: what would happen; nothing is changed or uploaded\n  mysid publish <repo-url> --yes    do it: create the repository and the first commit if there are none, add the remote\n                                    (origin), and git push -u origin <branch>\n  mysid publish owner/repo          shorthand for https://github.com/owner/repo.git\n  mysid publish <url> --name up     use a remote name other than origin\n\nCreate the repository on GitHub first (empty, no README). Sign-in is git's own (credential manager, or an SSH key); mysid\nnever sees a password or token. It never forces: a remote that already has other commits is left alone and the answer\nsays so. It refuses when secret-looking files (.env, keystores, .pem ...) are in the repository's history, and uncommitted\nchanges are not part of the push (mysid commit first). This sends the code to the internet: an agent must ask the\nowner before using --yes.\n"),
        "copy" | "cp" => Some("mysid copy: copy files or folders into the workspace (same as mysid file copy)\n\nUsage:\n  mysid copy <source> <destination>\n  mysid copy <source1> <source2> ... <destination-folder>\n  mysid copy ../other/src src/vendor --overwrite\n\nSources may be anywhere (a sibling project, a bare file name); the destination must be inside the workspace. Folders are\ncopied whole except .git, node_modules and .mysid. If any source is missing or any target exists, nothing is copied\n(--overwrite replaces).\n"),
        "deps" => Some(tools::deps::HELP),
        "doctor" => Some(tools::doctor::HELP),
        "tree" => Some(tools::scanner::orient::TREE_HELP),
        "new" => Some(
            "mysid new: create a new project\n\n\
Usage:\n  \
  mysid new react <name> [--dir <folder>] [--template react-ts|react|react-swc-ts|react-swc]\n\n\
React: runs the official Vite generator (npm create vite@latest <name> -- --template react-ts), then npm install.\n  \
  Needs Node.js and network access; it downloads and runs create-vite from npm.\n  \
  <name>: lower case letters, digits, - _ .  The project goes to <dir>/<name> (dir defaults to the current folder).\n  \
  It only creates in a missing or empty folder and never overwrites anything.\n\n\
Android: not available yet.\n"
        ),
        "skill" => Some(
            "mysid skill: workflow guides that teach an LLM how to use mysid\n\n\
Usage:\n  \
  mysid skill                     list the skills (base, android, react)\n  \
  mysid skill <name>              print one: steps plus small command examples\n  \
  mysid skill install [name]      write them as Claude Code skills into .claude/skills/ (base is always included)\n  \
  mysid skill install --global    write them to ~/.claude/skills/ instead\n"
        ),
        "apply" => Some(
            "mysid apply: apply edits from text that never goes through shell quoting

Usage:
    mysid apply <<'EOF'            (bash; in PowerShell use a single-quoted here-string @'...'@ piped in)
    <file name>
    <<<<<<< SEARCH
    <exact text to find>
    =======
    <replacement text>
    >>>>>>> REPLACE
    EOF
    mysid apply --file edit.txt    read the blocks from a file (write it with your file tool; no quoting at all)

Options: --dry-run (preview, write nothing), --replace_all (a block may match several places), --to <file> (blocks without a file name)

One call may hold many blocks for many files. Matching is as in `mysid patch` (exact, then line-ending tolerant, then
whitespace tolerant; refused on 0 or 2+ matches). Blocks for a file apply in order. All blocks apply or none is written;
a failure prints NOT APPLIED and names the block. An empty SEARCH on a missing file creates it. Bare file names work.
Quote the heredoc delimiter ('EOF'), or the shell will expand $ and backticks inside the blocks.
"
        ),
        "checkpoint" => Some(
            "mysid checkpoint: save the working tree so it can be reviewed and rolled back\n\n\
Usage:\n  \
  mysid checkpoint [\"label\"]   snapshot every file under the workspace (new files included; ignored files and\n                           secret-looking files left out). Branch, index and history are untouched.\n  \
  mysid checkpoint list      the saved checkpoints, newest first\n\n\
With no git repository it runs git init, git add and an initial commit (message = the label) first. Checkpoints live under refs/mysid/checkpoints and never appear in git log.\n"
        ),
        "check" => Some(
            "mysid check: fast type-check, errors only

Usage:
    mysid check [--workspace <path>]

Runs the TypeScript compiler (tsc --noEmit) in the workspace and prints only the errors, as File.ts:line: message.
Use it after each edit in a React/TypeScript project; use mysid build for a full production build.
"
        ),
        "list" => Some(
            "mysid list: what is in a folder

Usage:
    mysid list [folder]               sub-folders ranked by code (with their code-file counts), code files with line counts,
                                      other files collapsed; dot-folders and build output named once at the end
    mysid list [folder] --depth 1     every entry, plain (D folder / F file), also with -L<depth> or --full for the whole tree

It never shows file content: use mysid overview <file> for the outline, mysid read <file>#L10-60 for code, mysid graph <symbol>
for callers and callees. Respects .gitignore.
"
        ),
        "write" => Some(
            "mysid write: create a new file without shell quoting

Usage:
    mysid write <path> --stdin | --from <source>   content from stdin (heredoc / pipe) or a file

Refuses to replace an existing file (NOT WRITTEN, exit 1): use mysid apply or patch to change one, mysid file new to overwrite.
Parent folders are created.
"
        ),
        "edit" => Some(
            "mysid edit: change a file by line numbers

Usage:
  mysid edit <file>#L10-20 \"new text\"          replace lines 10 to 20 with the text   (file#L10 = one line)
  mysid edit <file>#L10-20 --file new.txt       the text from a file; or pipe it in (--stdin)
  mysid edit <file>#L10 --insert-after \"text\"   add lines after line 10   (--insert-before: above it)
  mysid edit <file>#L10-12 --delete             remove lines 10 to 12
  mysid edit <file>#L10-20 --file new.txt --expect \"fun load\"   refuse unless line 10 contains that text

Numbers are the ones mysid read shows (Foo.kt#L121-220). Use --expect when the file may have changed since you read it: the
edit then stops with NOT EDITED instead of changing the wrong lines. The answer lists the removed (-) and added (+) lines and
the new line numbers. All or nothing; CRLF files stay CRLF. Choose by what you know: the exact old text -> mysid patch; the line
numbers -> mysid edit; the same change in many places -> mysid replace.
"
        ),
        "append" => Some(
            "mysid append: add text to the end of a file without shell quoting

Usage:
    mysid append <path> --stdin | --from <source>

The file must exist. A line break is inserted first if the file does not end with one.
"
        ),
        "diff" => Some(
            "mysid diff: what changed since a checkpoint\n\n\
Usage:\n  \
  mysid diff            vs the latest checkpoint (or HEAD if there is none)\n  \
  mysid diff 2          vs checkpoint #2        mysid diff HEAD   /   mysid diff <any git revision>\n\n\
Prints a file list (A added, M modified, D deleted, with +/- line counts) and the first lines of each change.\n"
        ),
        "restore" => Some(
            "mysid restore: put the files back to a checkpoint\n\n\
Usage:\n  \
  mysid restore [target]          PREVIEW only: lists what would change (target: checkpoint number, last, HEAD, git revision)\n  \
  mysid restore [target] --yes    apply it\n\n\
Modified files get their old content back, deleted files are recreated, files added since are removed.\n\
A safety checkpoint is saved first (\"auto: before restore\"), so a restore can itself be undone. Ignored and\n\
secret-looking files are never touched.\n"
        ),
        "git" | "purge" => Some(tools::vcs::PURGE_HELP),
        "msvc" | "cl" => Some("mysid msvc: run a command with the Visual Studio C++ environment (vcvars64) set up\n\nUsage:\n  mysid msvc cl /nologo main.cpp          the compiler is on PATH\n  mysid msvc nmake /f Makefile.win        also link, lib, msbuild, ...\n  mysid msvc --errors \"nmake\"             only the error lines (error, fatal, LNK1xxx, [exit N])\n\nFinds Visual Studio with vswhere (the Desktop C++ workload), then runs `call vcvars64.bat && <command>` in cmd.exe.\nThe same flags as exec: --timeout N, --out file, --workspace_root.\n"),
        "emsdk" | "wasm" => Some("mysid emsdk: run a command with the Emscripten SDK environment set up\n\nUsage:\n  mysid emsdk emcc main.cpp -O2 -o out.js        emcc / em++ / emmake / emcmake on PATH\n  mysid wasm emmake make                         (wasm is the same command)\n  mysid emsdk --errors \"emmake make\"             only the error lines\n\nFinds the SDK from EMSDK, then %USERPROFILE%\\emsdk, C:\\emsdk, D:\\emsdk, and runs `call emsdk_env.bat && <command>`\n(`. emsdk_env.sh` elsewhere). The same flags as exec: --timeout N, --out file, --workspace_root.\n"),
        "bootstrap" | "init" => Some(
            "mysid bootstrap: initialize a local git repository, and nothing else\n\n\
Usage:\n  \
  mysid bootstrap                   git init (branch main) unless a repository is here; nothing is staged or committed\n  \
  mysid bootstrap --branch trunk    name the initial branch\n  \
  mysid bootstrap --gitignore       also write a .gitignore for this stack (never overwrites one)\n\n\
Already a repository: it says so and changes nothing. Inside a parent repository: it says which, and creates nothing.\n\
Never in a drive root or the home folder. Build output and secret-looking files are hidden locally through\n\
.git/info/exclude. .gitignore only affects untracked files: a file that is already committed stays tracked until\n\
`git rm --cached <file>`. Then: write or review .gitignore, and mysid commit --branch main --message \"Initial commit\".\n"
        ),
        "commit" => Some(
            "mysid commit: record the changes with git, locally\n\n\
Usage:\n  \
  mysid commit --message \"Add authentication\"        commit on the current branch\n  \
  mysid commit \"Add authentication\"                  the same, message as a plain argument\n  \
  mysid commit --branch main --message \"Initial commit\"   make sure the work lands on branch main (see below)\n  \
  mysid commit --message \"Fix validation\" --paths a.kt b.kt   commit only these paths\n\n\
What it does:\n  \
  - stages every change under the workspace (or only --paths) and commits it locally; the answer lists the files\n    \
    that went in, so nothing unrelated slips by unseen\n  \
  - --branch: no commits yet -> the initial commit is made on that branch; already on it -> nothing; it exists ->\n    \
    switch to it (git refuses when your changes would be overwritten); it does not exist -> create it from here\n  \
  - no repository yet: runs git init first (see mysid bootstrap for init alone)\n  \
  - leaves out files that look like secrets (*.jks, *.keystore, *.pem, .env, keystore.properties,\n    \
    local.properties, ...) and says which\n  \
  - notes tracked files that a .gitignore rule matches: they stay tracked until git rm --cached\n  \
  - never pushes (mysid publish does, with --yes), and adds no attribution lines to the message\n  \
  - prints: <hash> <message> - N files, +added -removed [branch]\n"
        ),
        "history" => Some(
            "mysid history: the last commits, compactly\n\n\
Usage:\n  \
  mysid history              last 5 commits\n  \
  mysid history --past 10    last 10 (also: history 10, history -10); at most 100\n\n\
Each line: <hash>  <age>  <subject>  (N files, +added -removed)\n"
        ),
        "android" | "adb" => Some(
            "mysid android: the Android / device toolkit\n\n\
Start here:\n  \
  1. mysid devices                              list the connected devices\n  \
  2. mysid wifi [--ip <ip>] [--port <port>]     connect a device over Wi-Fi\n  \
  3. mysid install                              put the built app on the device (Android: debug APK; Rust: the built exe)\n  \
  4. mysid debug \"usb, router\" [--seconds 40]   record only the device-log (logcat) lines you care about, then read them back; mysid help debug\n  \
  5. mysid android goto \"USB Drive\" \"Durandhar\"   TV: D-pad to each label, then OK  [--no-ok] [--nth 2]\n  \
  6. mysid android scenario steps.json | mysid android focus   scripted run with checks (pass/fail, cold start, jank, crashes) | the focused element\n  \
  7. mysid android keyevent --keycode <back|home|dpad_right|KEYCODE_..> [--times 20 --fast]   send keys\n  \
  8. mysid release                              Android: raise versionCode and build a local release AAB (web project: Netlify deploy); mysid help release\n\n\
Also in the toolkit (details below): launch_app, logcat, crashes, clear_logcat, tap, swipe, longpress, type, scroll, screenshot, ui_dump, batch.\n\n\
Description:\n  \
  Device controls, logs, Wi-Fi connectivity, and UI input simulation.\n\n\
Usage:\n  \
  mysid android devices                       List connected ADB devices\n  \
  mysid android connect_wifi [--ip <ip>]      Pair/connect device over Wi-Fi\n  \
  mysid android install                       Deploy built debug APK to device\n  \
  mysid android launch_app                    Launch main application activity\n  \
  mysid android logcat [--lines <n>]          Dump recent logcat entries\n  \
  mysid android crashes                       Filter logcat for fatal exceptions & crashes\n  \
  mysid android clear_logcat                  Flush ADB logcat buffer\n  \
  mysid android tap --x <x> --y <y>           Tap, then return a screenshot  (also: tap 540 1200)\n  \
  mysid android tap --text \"Apply\"            Tap the element with that text (--desc \"Open menu\", --id <resource-id>, --index <n>)\n  \
  mysid android swipe --x1 --y1 --x2 --y2     Swipe [--duration ms]\n  \
  mysid android longpress --x <x> --y <y>     Press and hold [--duration ms]\n  \
  mysid android type --text <text>            Type text (newline = Enter), then screenshot\n  \
  mysid android keyevent --keycode <code|key> Send a key (back, home, enter, KEYCODE_..., 66) [--times n], then screenshot\n  \
  mysid android keyevent dpad_right --times 20 --fast   Many presses in ONE adb call (also --keys \"dpad_right dpad_right enter\", --gap ms)\n  \
  mysid android scroll down | up | left | right   Scroll [to \"text\"] (scrolls until the text is on screen)\n  \
  mysid android screenshot                    Just the screen\n  \
  mysid android scenario steps.json           Scripted run with checks: pass/fail per step, cold start, janky frames, crashes (below)\n  \
  mysid android focus [--list]                Text only: the focused element (label, class, bounds, path); --list adds every focusable one\n  \
  mysid android goto \"USB Drive\" \"Durandhar\"  TV / D-pad: press the D-pad until each label has focus, then OK [--no-ok] [--max 30] [--nth 2]\n  \
  mysid android ui_dump                       Visible elements: [i] text | desc | id | clickable | center(x,y)\n  \
  mysid android batch --json steps.json       Many steps, one call: [{\"action\":\"tap\",\"x\":540,\"y\":1200,\"shot\":true},{\"action\":\"wait\",\"ms\":500}]\n\n\
Every action waits --wait ms (default 600), captures the screen and prints:\n  \
  tap(540,1200) ok | device dee50561 | screen 1080x2412 | image 1000x2243 | scale 1.080\n\
CLI saves the PNG to .mysid/shots/<ms>.png (last 20 kept); over MCP the image comes back as image content.\n\
Coordinates are DEVICE pixels. The image is smaller: device = image x scale. --space image takes coordinates read off the\n\
image instead and converts them. --grid overlays a labelled 100 px grid and a red crosshair where the action landed.\n\
Every result also says `screen changed` or `screen UNCHANGED` (so you know the action did something; --no-diff skips the\n\
extra capture), the foreground app, and the workspace. `--app <package>` refuses to act when another app is in front.\n\
`--expect \"text\"` waits for that text to appear (--timeout ms, default 5000) instead of a fixed wait. In a batch, a step stops\n\
the run when its expectation fails, when it errors, or with \"require_change\": true when the screen did not change.\n\
Options: --device <serial> (when several), --scale <0.1-1>, --wait <ms>, --grid, --space image. Every adb call times out after 10 s.\n\
TV / no touch: a key press waits until the screen stops changing (two equal captures; --settle-timeout ms, default 2000)\n\
instead of a fixed 600 ms, unless --wait is given. `keyevent dpad_right --no-shot` answers with text only: the result and the\n\
focus line (`focus: \"Play all\" [Button] center (x,y) bounds ... path: A > B`); `--focus` adds that line to a screenshot answer.\n\
`goto` reads the accessibility tree before every press, picks the key towards the target, and stops with a reason when focus\n\
does not move, jumps the wrong way, loops, or the target never appears (then with a screenshot). Also usable as a batch step.\n\
scenario file: {\"app\":\"com.x.tv\",\"steps\":[\"launch --cold\",\"goto 'USB Drive'\",\"expect text 'Play all'\",\"expect focus 'Play all'\"]}\n\
  steps: launch [--cold] [pkg] | key <k> [--times n] | goto <labels> | tap, swipe, type, scroll, wait ... | expect text|focus|gone|app <v> [--timeout ms]\n\
  Report: PASS/FAIL per step (a screenshot of the first failure only), cold start (am start -W), janky frames (gfxinfo), and every\n\
  crash / ANR / exception of the app under the step that was running; a crash fails its step. --keep-going continues after a failure;\n\
  \"max_cold_start_ms\" / \"max_janky_percent\" in the file make those numbers pass/fail. Exit status is non-zero on FAIL.\n\
Actions are logged to .mysid/touch.log.\n"
        ),
        "devices" => Some(
            "mysid devices: List connected ADB devices\n\n\
Usage:\n  \
  mysid devices\n"
        ),
        "wifi" | "connect_wifi" => Some(
            "mysid wifi: Connect to Android device over Wi-Fi\n\n\
Usage:\n  \
  mysid wifi [--ip <ip_address>] [--port <port>]\n"
        ),
        "set_workspace" | "set" => Some(
            "mysid set: choose the project for commands run from this folder\n\n\
Description:\n  \
  Optional: mysid already finds the project from the folder you run it in. `set` pins a project (a path or a\n  \
  registered name) for commands run from THIS folder and the ones below it, by THIS session (the agent or terminal\n  \
  that ran it: another agent sharing the folder keeps its own choice, and yours survives the shell being reset).\n  \
  Prints the project's top-level entries. (`set_workspace` is the same command.)\n\n\
Usage:\n  \
  mysid set <path | name>\n\n\
Examples:\n  \
  mysid set E:\\Hypersonus\\v44\n"
        ),
        "get_workspace" => Some(
            "mysid get_workspace: Get currently active workspace path\n\n\
Usage:\n  \
  mysid get_workspace\n"
        ),
        "exec" | "run" => Some(
            "mysid exec: Run a shell command in the workspace\n\n\
Usage:\n  \
  mysid exec <command...>              Everything after `exec` is the command; -A, -m, | pass through\n  \
  mysid exec \"<command>\"               A single quoted string works too (pipes, redirects)\n  \
  mysid exec --timeout 120 <command>   Seconds before the command is killed (default 30)\n  \
  mysid exec --out run.txt <command>   Also save everything the command printed to a file, even if it was killed\n  \
  mysid exec --tail 25 --errors -- <command>   mysid flags before the command; `--` ends them (--head/--tail/--grep/--grep-v/--errors filter the output)\n  \
  mysid exec \"<command>\" --timeout 120 --tail 30   after ONE quoted command string the flags are mysid's too\n  \
  mysid exec --env msvc|emsdk ...      set up Visual Studio C++ / Emscripten first (the same as mysid msvc ... / mysid emsdk ...)\n\n\
Notes:\n  \
  Runs through cmd.exe on Windows (sh elsewhere): && | > work, ; does not, so a command with ; is run in PowerShell\n  \
  (the answer says so). Choose with --shell cmd|powershell|bash (bash = Git Bash). --out must be inside the workspace.\n  \
  Exit code 1 with no output usually means \"no match\"\n  \
  (grep/findstr), not a failure of mysid. Prefer `mysid search` and `mysid read` for code.\n\n\
Examples:\n  \
  mysid exec git status\n  \
  mysid exec \"adb devices\"\n"
        ),
        "overview" | "signatures" => Some(
            "mysid overview: Extract method signatures and parameter outlines across languages\n\n\
Description:\n  \
  High-speed signature extractor for Java, Kotlin, C/C++, Rust, Python, TypeScript/JS, Go, Swift, and C#.\n  \
  Returns clean outlines of classes, methods, and full parameter lists without wasting context.\n\n\
Usage:\n  \
  mysid overview <file>                Extract method signatures from a single file\n  \
  mysid overview <file1> <file2> ...   Extract signatures from multiple files in one turn\n  \
  mysid overview <directory>           Extract signatures from all source files in a folder\n  \
  mysid overview <directory> --depth 1 Only the folder names and their code-file counts (choose a folder first)\n  \
  mysid overview <directory> --match word   Only files whose path, class or function contains the word\n\n\
Options:\n  \
  --format json                        Return structured JSON instead of human-readable outline\n\n\
Examples:\n  \
  mysid overview MusicService.kt\n  \
  mysid overview FlacDecoder.cpp VolumeManager.kt\n  \
  mysid overview app/src/main/java/com/hypersonus/player/volume\n"
        ),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Internal: the detached process `mysid build` starts to run a build (see tools/build/worker.rs).
    if args.len() == 2 && args[0] == "__build_worker" {
        tools::build::worker::run_worker(&args[1]);
        return;
    }

    // Internal: the detached logcat recorder `mysid debug` starts (see tools/build/debug.rs).
    if args.len() == 2 && args[0] == "__debug_recorder" {
        tools::build::debug::run_recorder(&args[1]);
        return;
    }

    // ── CLI Mode (standalone terminal execution without Python) ───────────────
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V" | "version") {
        println!("mysid {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    // `mysid help --claude` / `--codex` / `--antigravity`: help written in that client's own tool vocabulary.
    if let Some(client) = args.iter().find_map(|a| a.strip_prefix("--").filter(|c| matches!(*c, "claude" | "codex" | "antigravity" | "gemini"))) {
        if args[0] == "help" || args[0].starts_with("--") {
            if let Some(text) = client_help(client) {
                println!("{}", text);
                return;
            }
        }
    }
    if !args.is_empty() {
        let wants_help = args.iter().any(|a| a == "-h" || a == "--help" || a == "help");
        if wants_help {
            let target_tool = if args[0] == "-h" || args[0] == "--help" || args[0] == "help" {
                args.get(1).map(|s| s.as_str())
            } else {
                Some(args[0].as_str())
            };

            if let Some(tool) = target_tool {
                if let Some(help_text) = get_tool_help(tool) {
                    println!("{}", help_text);
                    return;
                }
            }

            println!("{}", get_root_help());
            return;
        }

        let workspace_mgr = Arc::new(Mutex::new(WorkspaceManager::new_cli()));
        let (verb, mut params) = parse_cli_args(&args);
        let active_ws = workspace_mgr.lock().unwrap().get_current();
        let notice = workspace_mgr.lock().unwrap().notice.clone();

        // An explicit --workspace_root / --project_path wins and drives every tool's root key,
        // so one flag is enough whichever key a tool reads. Otherwise use the active workspace.
        let explicit_root = params.as_object().and_then(|obj| {
            ["workspace_root", "project_path", "workspace"]
                .iter()
                .find_map(|k| obj.get(*k).and_then(|v| v.as_str()).map(|s| s.to_string()))
        });
        // `--workspace_root web-ui` may be an alias from the registry.
        let explicit_root = explicit_root.map(|r| workspace_mgr.lock().unwrap().resolve_alias(&r).unwrap_or(r));
        // No flag: an absolute path in the arguments names its own project (`mysid read D:pps\web-ui\src\App.tsx`).
        let path_root = if explicit_root.is_none() && !matches!(verb.as_str(), "new" | "skill" | "set" | "set_workspace" | "setworkspace" | "get_workspace" | "getworkspace" | "exec" | "run") {
            tools::workspace::root_from_paths(&params).map(|p| p.to_string_lossy().to_string())
        } else {
            None
        };
        let chosen = explicit_root.clone().or(path_root.clone());
        let active_ws = chosen.clone().or(active_ws);
        if let Some(ref ws) = active_ws {
            if let Some(obj) = params.as_object_mut() {
                if chosen.is_some() {
                    obj.insert("workspace_root".to_string(), json!(ws));
                    obj.insert("project_path".to_string(), json!(ws));
                } else {
                    obj.entry("workspace_root").or_insert_with(|| json!(ws));
                    obj.entry("project_path").or_insert_with(|| json!(ws));
                }
            }
        }

        let filters = match OutputFilter::take(&mut params) {
            Ok(f) => f,
            Err(e) => {
                println!("ERROR: {}", e);
                std::process::exit(2);
            }
        };
        let output = dispatch_method(&verb, &params, active_ws.as_deref(), &workspace_mgr);
        let unknown_command = output.starts_with("ERROR: No such command");
        // Commands that never touch a workspace (skill guides) print no workspace notice.
        let installing = params.get("query").and_then(|q| q.as_str()).map_or(false, |q| q.trim_start().starts_with("install"));
        let uses_workspace = !(verb == "new" || (verb == "skill" && !installing));
        if let (Some(n), None, false, true) = (notice, &chosen, unknown_command, uses_workspace) {
            println!("{}", n);
        }
        println!("{}", filters.apply(&format_output(&output)));
        let _ = io::stdout().flush();
        std::process::exit(cli_exit_code(&output, unknown_command));
    }

fn get_mcp_tools_list() -> Value {
    json!([
        {
            "name": "read",
            "description": "The CODE of a file: a short file whole, a longer source file returns its outline with line numbers and the exact command for a slice (so a bare read never floods), a slice (Player.kt#L10-40), several slices in one call, or a symbol definition with callers. For the full outline use overview; not for folders (use list). Use just the file name (Player.kt), never the full path: mysid finds it anywhere in the workspace, and output prints short names too. Give a folder (ui/Player.kt) only when two files share a name. Bundle all file slices into a single call.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Symbol name (e.g. 'StreamResolver') or file slices (e.g. 'Player.kt#L10-40' or 'AudioEngine.cpp#L100-140')"
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of file slice specs to inspect together (e.g. ['FileA.kt#L1-30', 'FileB.cpp#L40-80'])"
                    }
                }
            }
        },
        {
            "name": "overview",
            "description": "Extract all method signatures, parameters, return types, and class structures across Java, Kotlin, C/C++, Rust, Python, TypeScript, Go, Swift, and C#.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File name such as 'MusicService.kt' (no directories needed), or a directory" },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional list of files to inspect simultaneously"
                    },
                    "format": { "type": "string", "description": "Output format: 'text' (default human-readable outline) or 'json'" },
                    "depth": { "type": "integer", "description": "For a directory: folder names and code-file counts only, this many levels deep (no outlines)" },
                    "match": { "type": "string", "description": "Keep only the files whose path, or a class / function in them, contains this word" }
                }
            }
        },
        {
            "name": "tree",
            "description": "The folder map of the workspace: folders with their number of code files, depth 2 by default. Build output and node_modules are left out. Use it to pick the folder for overview.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Folder to map (default: the workspace)" },
                    "depth": { "type": "integer", "description": "Levels (default 2, max 4)" },
                    "all": { "type": "boolean", "description": "Also dot-folders" }
                }
            }
        },
        {
            "name": "search",
            "description": "High-speed multi-threaded regex/string search respecting .gitignore. Results list short file names (Foo.kt:12); pass those names straight to read/patch. md, txt and rst files are left out unless all=true or types includes them; a no-match answer says so. grep-style \\| is alternation; a very long line is shown as a window around the match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern or regex" },
                    "types": { "type": "string", "description": "Optional comma-separated file extensions (e.g. 'kt,cpp,rs')" },
                    "max_results": { "type": "integer", "description": "Maximum match entries to return (default 15, up to 500)" },
                    "offset": { "type": "integer", "description": "Skip the first n matches (next page)" },
                    "count": { "type": "boolean", "description": "Per-file summary: matching lines per file, most first, no hit text" },
                    "context": { "type": "integer", "description": "Context lines around each match (default: 1)" },
                    "group": { "type": "boolean", "description": "File name once, then 'line: text' under it (fewer tokens)" },
                    "all": { "type": "boolean", "description": "Also search md, txt and rst files" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "replace",
            "description": "Fast global workspace search-and-replace refactoring or targeted single-file replacement.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old": { "type": "string", "description": "Exact text to search and replace" },
                    "new": { "type": "string", "description": "Replacement text" },
                    "path": { "type": "string", "description": "Optional file name (Foo.kt) or subdirectory to restrict replacement" },
                    "types": { "type": "string", "description": "Optional comma-separated file extensions (e.g. 'kt,cpp')" },
                    "dry_run": { "type": "boolean", "description": "If true, previews matches and diffs without modifying files on disk" }
                },
                "required": ["old", "new"]
            }
        },
        {
            "name": "patch",
            "description": "Surgical search-and-replace patching in a single target file.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "file": { "type": "string", "description": "File name only, e.g. Foo.kt (no directories needed; add a folder only if two files share the name)" },
                    "old": { "type": "string", "description": "Exact original code chunk to replace" },
                    "new": { "type": "string", "description": "New replacement code chunk" },
                    "show": { "type": "boolean", "description": "Also return the removed (-) and added (+) lines, so the file need not be read again" }
                },
                "required": ["file", "old", "new"]
            }
        },
        {
            "name": "patch_batch",
            "description": "Several replacements in one call. For ONE file: path + text, where text holds SEARCH/REPLACE snippets ('<<<<<<< SEARCH', the exact old text, '=======', the new text, '>>>>>>> REPLACE', repeated); all applied or none, show=true lists what changed. For several files: patches (applied in order; stops at the first failure, earlier ones stay).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "The one file the snippets in text are for" },
                    "text": { "type": "string", "description": "SEARCH/REPLACE snippets for that file" },
                    "show": { "type": "boolean", "description": "Return the removed (-) and added (+) lines" },
                    "patches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "old": { "type": "string" },
                                "new": { "type": "string" }
                            },
                            "required": ["file", "old", "new"]
                        },
                        "description": "List of patch operations across files, applied in order"
                    }
                }
            }
        },
        {
            "name": "mkdir",
            "description": "Create directory recursively, automatically creating any missing parent directories.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Directory path to create" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "file",
            "description": "File system utilities: new file, rename, delete, or list.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["new", "rename", "delete", "list"], "description": "Action to perform" },
                    "path": { "type": "string", "description": "File name (Foo.kt, resolved anywhere in the workspace) or a directory path" },
                    "content": { "type": "string", "description": "Content for new file" },
                    "new_path": { "type": "string", "description": "Destination path for rename" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "graph",
            "description": "Callers and callees of a symbol (1 hop by default), including Kotlin/Java <-> C/C++ JNI bridges (a native method links to its Java_... function, native code calling back by method name links to the Kotlin method), calls through typed receivers and chained getters, thread entry points (&Class::loop), constructors, and base-class methods to their overrides. A FILE NAME (UsbDirectOutput.kt) gives the file card: what it defines, which files it calls into, which call it. With no symbol, a compact map of the workspace; a directory drills into the map.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Function, class, Class.method, a file name (File.kt), or a directory" },
                    "depth": { "type": "integer", "description": "Hops of callers and callees to include (default 1, max 5)" },
                    "flow": { "type": "boolean", "description": "Draw UPSTREAM / TARGET / DOWNSTREAM trees instead of the compact card" }
                }
            }
        },
        {
            "name": "build",
            "description": "Build the project (Gradle, Cargo, npm, Python). Runs in a detached worker: waits up to 20s, then returns a progress line with the current stage; call again with action 'status' to check, 'stop' to cancel. Full output is live in .mysid/build.log. Failures show only the errors.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["build", "clean", "status", "stop", "install"], "description": "Build action to perform (default build)" },
                    "wait": { "type": "integer", "description": "Seconds to wait before returning a progress line (default 20)" }
                }
            }
        },
        {
            "name": "commit",
            "description": "Commit the workspace's changes locally with git (message required). Creates the repository first if none exists (never in a drive root or home folder), skips secret-looking files, never pushes, adds no attribution. Returns '<hash> <message> - N files, +added -removed [branch]'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "message": { "type": "string", "description": "Commit message" },
                    "branch": { "type": "string", "description": "Make the commit land on this branch: no commits yet -> the initial commit is made on it; exists -> switch to it when safe; missing -> create it from the current commit" },
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Commit only these paths (default: every change under the workspace)" }
                },
                "required": ["message"]
            }
        },
        {
            "name": "history",
            "description": "Last commits of the workspace repository, one compact line each: hash, age, subject, files and lines changed.",
            "inputSchema": { "type": "object", "properties": { "past": { "type": "integer", "description": "How many commits (default 5, max 100)" } } }
        },
        {
            "name": "new",
            "description": "Create a new project. query 'react my-app' runs the official Vite generator (react-ts) in <dir>/my-app and npm installs it; needs Node and network (downloads create-vite from npm). Only creates in a missing or empty folder. Android is not available yet.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "'<stack> <name>', e.g. 'react my-app'" },
                    "dir": { "type": "string", "description": "Folder to create the project in (default: the current folder; pass it explicitly when calling over MCP)" },
                    "template": { "type": "string", "description": "React template: react-ts (default), react, react-swc-ts, react-swc" }
                },
                "required": ["query"]
            }
        },
        {
            "name": "list",
            "description": "Every entry of a folder (folders D, files F, with counts). For where the code lives use graph; for a file's outline use overview; for code use read.",
            "inputSchema": { "type": "object", "properties": { "path": { "type": "string", "description": "Folder, relative to the workspace (default: the workspace root)" } } }
        },
        {
            "name": "write",
            "description": "Create a file with the given content (parent folders are created). Refuses to replace an existing file unless overwrite=true; replacing a CRLF file keeps CRLF. For changing part of an existing file use patch or apply.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File to create, relative to the workspace (or absolute)" },
                    "text": { "type": "string", "description": "The file content" },
                    "overwrite": { "type": "boolean", "description": "Allow replacing an existing file" }
                },
                "required": ["path", "text"]
            }
        },
        {
            "name": "edit",
            "description": "Edit a file by line numbers (the ones read shows): replace lines first-last with new text, insert after/before a line, or delete lines. Give path as Foo.kt#L10-20 (or path plus start/end). expect= refuses unless the first line named contains that text, so numbers that moved cannot damage the file. Keeps CRLF; answers with the removed and added lines.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File, optionally with the range: Foo.kt#L10-20 or Foo.kt#L10" },
                    "start": { "type": "integer", "description": "First line (when the path has no #L range)" },
                    "end": { "type": "integer", "description": "Last line (default: start)" },
                    "text": { "type": "string", "description": "The new text (not needed for delete)" },
                    "mode": { "type": "string", "enum": ["replace", "insert_after", "insert_before", "delete"], "description": "default replace" },
                    "expect": { "type": "string", "description": "Text the first named line must contain, else nothing changes" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "append",
            "description": "Add text to the end of an existing file (found by file name), on its own line, keeping the file's line endings. create=true makes the file if it is missing.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File name, e.g. Notes.md" },
                    "text": { "type": "string", "description": "Text to add" },
                    "create": { "type": "boolean", "description": "Create the file when it does not exist" }
                },
                "required": ["path", "text"]
            }
        },
        {
            "name": "apply",
            "description": "Apply several edits in one call from SEARCH/REPLACE blocks (file name on the line before '<<<<<<< SEARCH', then the exact text, '=======', the replacement, '>>>>>>> REPLACE'). Same safe matching as patch (refuses on 0 or 2+ matches, keeps CRLF), many blocks and files per call, all applied or none (a failure prints 'NOT APPLIED' and names the block). An empty SEARCH on a missing file creates it. Preferred for multi-line code, quotes, $ and backslashes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The blocks" },
                    "dry_run": { "type": "boolean", "description": "Preview, write nothing" },
                    "show": { "type": "boolean", "description": "After applying, list the removed (-) and added (+) lines of each block" },
                    "replace_all": { "type": "boolean", "description": "Let a block match several places" },
                    "to": { "type": "string", "description": "File for blocks that have no file name line" }
                },
                "required": ["text"]
            }
        },
        {
            "name": "checkpoint",
            "description": "Save a snapshot of the working tree (new files included; ignored and secret-looking files left out) under refs/mysid, without touching the branch, index or history. Use before a batch of edits. query 'list' shows saved checkpoints. Creates the git repository if there is none.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Label for the checkpoint, or 'list'" } } }
        },
        {
            "name": "diff",
            "description": "Compact review of what changed since a checkpoint (default: the latest, else HEAD): files with A/M/D and +/- counts plus the first lines of each change. Run it after edits and a build, before committing.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Checkpoint number, 'HEAD', or any git revision (default: latest checkpoint)" } } }
        },
        {
            "name": "restore",
            "description": "Roll files back to a checkpoint. Without yes=true it only previews the plan and changes nothing. With yes=true it first saves a safety checkpoint ('auto: before restore'), restores modified files, recreates deleted ones and removes files added since. Never touches ignored or secret-looking files.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Checkpoint number, 'last', 'HEAD' or a git revision (default: latest checkpoint)" },
                    "yes": { "type": "boolean", "description": "Apply the restore (otherwise preview only)" }
                }
            }
        },
        {
            "name": "skill",
            "description": "Workflow guides for using mysid well. query '' lists them; 'base', 'android' or 'react' returns the steps and small command examples; 'install' writes them as Claude Code skills into .claude/skills. Read 'base' first in a new workspace.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Skill name (base, android, react), 'install [name]', or empty to list" } } }
        },
        {
            "name": "release",
            "description": "Android release: bumps versionCode, builds the release bundle (bundleRelease) with stage-by-stage progress, then verifies the bundle signature and R8 mapping. Same wait/status/stop behaviour as build.",
            "inputSchema": { "type": "object", "properties": { "wait": { "type": "integer", "description": "Seconds to wait before returning a progress line (default 20)" } } }
        },
        {
            "name": "debug",
            "description": "Android device log recorder. query 'Buffer, router, usb' clears the device logcat, records only lines whose tag or text contains a keyword into .mysid/debug.log for a fixed window (seconds, default 20, 10-60) while the user reproduces the issue, then returns '20 sec log record done' with the lines (repeats collapsed). Call with no query to read the last recording again. Also 'status', 'stop', 'clear'.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Comma-separated keywords, or status/stop/clear. Omit to show the recording." },
                    "new": { "type": "boolean", "description": "Only lines recorded since the previous debug call" },
                    "raw": { "type": "boolean", "description": "Raw mode: clear logcat, capture the WHOLE device log (all buffers, system dump) for `seconds`, keep lines containing the query terms with 3 lines above and below, save to .mysid/debug.log. Use for system/framework errors not logged by the app's own tags." },
                    "seconds": { "type": "integer", "description": "Recording window in seconds, 10-60 (default 20). The call waits for the window, then reports 'N sec log record done'." },
                    "level": { "type": "string", "description": "Minimum level to show: V D I W E" },
                    "lines": { "type": "integer", "description": "Maximum lines to show (default 60)" }
                }
            }
        },
        {
            "name": "test",
            "description": "Run the project tests (Cargo, npm/pnpm/yarn test, Gradle, pytest; detected). One line when everything passes (TESTS OK, 104 passed); on failure only the failing tests with their messages and locations. Full output is in .mysid/test.log. Optional query filters by test name.",
            "inputSchema": { "type": "object", "properties": { "query": { "type": "string", "description": "Only tests matching this name" }, "timeout": { "type": "integer", "description": "Seconds before the run is killed (default 600)" } } }
        },
        {
            "name": "git",
            "description": "Git-level operations. action=purge removes git from the project as if it had never been used: .git/ (commits, branches, tags, stashes, index, config, remote URLs, mysid checkpoints); .gitignore / .gitattributes / .gitmodules / extras / nested repositories only with their own flag. Source and every other project file are never touched. WITHOUT yes it only previews (and warns about commits that exist on no remote). IRREVERSIBLE: NEVER set yes=true unless the user explicitly told you to purge.",
            "inputSchema": { "type": "object", "properties": {
                "action": { "type": "string", "enum": ["purge"] },
                "yes": { "type": "boolean", "description": "Actually remove. Only after the user said so." },
                "gitignore": { "type": "boolean", "description": "Also remove .gitignore" },
                "gitattributes": { "type": "boolean", "description": "Also remove .gitattributes" },
                "gitmodules": { "type": "boolean", "description": "Also remove .gitmodules" },
                "extras": { "type": "boolean", "description": "Also remove .git-blame-ignore-revs and .gitkeep files" },
                "nested": { "type": "boolean", "description": "Also remove repositories inside subfolders" },
                "all": { "type": "boolean", "description": "Every optional item above" }
            }, "required": ["action"] }
        },
        {
            "name": "bootstrap",
            "description": "Initialize a local git repository and nothing else: no staging, no commit, no upload. If a repository already exists it says so and changes nothing; inside a parent repository it reports that. Optional gitignore=true writes a .gitignore for the detected stack (never overwrites). Next: review .gitignore, then commit with branch and message.",
            "inputSchema": { "type": "object", "properties": { "branch": { "type": "string", "description": "Initial branch name (default main)" }, "gitignore": { "type": "boolean", "description": "Also write a .gitignore for this stack if none exists" } } }
        },
        {
            "name": "publish",
            "description": "First upload of the project to a git remote (GitHub). WITHOUT yes it only previews (nothing changes). With yes=true it creates the repository and first commit if needed, adds the remote and pushes the current branch (never forces; refuses when secret-looking files are in the history). Uploading code is outward-facing: NEVER set yes=true unless the user has explicitly told you to publish to that repository.",
            "inputSchema": { "type": "object", "properties": { "url": { "type": "string", "description": "https://github.com/owner/repo.git, git@github.com:owner/repo.git, or owner/repo" }, "yes": { "type": "boolean", "description": "Actually push. Only after the user said so." }, "name": { "type": "string", "description": "Remote name (default origin)" } }, "required": ["url"] }
        },
        {
            "name": "status",
            "description": "Git status in one call: branch, upstream (ahead/behind), changed files (staged, unstaged, untracked), last commit. Read only. Secret-looking files are marked: mysid commit leaves them out.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "dev",
            "description": "Run the project's dev server (npm/pnpm/yarn run dev or start) in the background. No query: start it and wait until it prints a local URL that accepts connections, then return the URL (open it in a browser to check the app renders). Also 'log' (server errors, else last lines; lines=N for a raw tail), 'status', 'stop'. Output is in .mysid/dev.log.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "start (default), log, status or stop" },
                    "port": { "type": "integer", "description": "Port to pass to the server (--port)" },
                    "lines": { "type": "integer", "description": "log: raw tail of this many lines" },
                    "wait": { "type": "integer", "description": "start: seconds to wait for the URL (default 30)" }
                }
            }
        },
        {
            "name": "doctor",
            "description": "Read-only project check. sub=compare with a (old folder) and b (new folder): totals (files, folders, size), how many files are identical, changed (lines, which side is newer), differ only in CRLF/LF, or exist in one folder only, a table per top-level folder, and a verdict on whether the old folder can be deleted. Generated folders (node_modules, build, .git ...) are left out unless everything=true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sub": { "type": "string", "enum": ["compare"], "description": "the check to run" },
                    "a": { "type": "string", "description": "compare: the old folder" },
                    "b": { "type": "string", "description": "compare: the new folder" },
                    "max": { "type": "integer", "description": "entries listed per group (default 25)" },
                    "brief": { "type": "boolean", "description": "totals, verdict and per-folder table only" },
                    "everything": { "type": "boolean", "description": "also compare generated folders" }
                },
                "required": ["sub", "a", "b"]
            }
        },
        {
            "name": "deps",
            "description": "npm packages the source imports that package.json does not list (with the files that need them and the install command), and listed dependencies nothing imports. Relative imports, Node built-ins and tsconfig path aliases are ignored.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "android",
            "description": "Android ADB device bridge and device control. tap/swipe/longpress/type/keyevent/screenshot/batch act on the device and ALWAYS return a screenshot (image) plus one line mapping image pixels to device pixels (x device = x image * scale). tap accepts text/desc/id to find an element itself; ui_dump lists visible elements.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["devices", "connect_wifi", "install", "launch_app", "logcat", "crashes", "clear_logcat", "tap", "swipe", "longpress", "type", "keyevent", "screenshot", "ui_dump", "focus", "goto", "scenario", "batch"],
                        "description": "Android device command"
                    },
                    "package_name": { "type": "string", "description": "Application package ID (optional)" },
                    "lines": { "type": "integer", "description": "Logcat line limit (default: 50)" },
                    "x": { "type": "integer", "description": "X for tap/longpress, in device pixels (or image pixels with space=image)" },
                    "y": { "type": "integer", "description": "Y for tap/longpress" },
                    "x1": { "type": "integer", "description": "swipe start X" },
                    "y1": { "type": "integer", "description": "swipe start Y" },
                    "x2": { "type": "integer", "description": "swipe end X" },
                    "y2": { "type": "integer", "description": "swipe end Y" },
                    "text": { "type": "string", "description": "type: the text to type. tap/longpress: tap the element with this text instead of coordinates" },
                    "desc": { "type": "string", "description": "tap/longpress: element content-description (Compose icons)" },
                    "id": { "type": "string", "description": "tap/longpress: element resource-id" },
                    "index": { "type": "integer", "description": "which match to use when text/desc/id matches several elements" },
                    "key": { "type": "string", "description": "Key code or name for keyevent (BACK, HOME, ENTER, KEYCODE_..., 66)" },
                    "duration": { "type": "integer", "description": "swipe (default 300) / longpress (default 800) duration in ms" },
                    "wait": { "type": "integer", "description": "ms to wait after the action before the screenshot (default 600)" },
                    "scale": { "type": "number", "description": "returned image size as a fraction of the screen, 0.1-1 (default: longest side 2000 px)" },
                    "space": { "type": "string", "enum": ["device", "image"], "description": "coordinate space of x/y: device pixels (default) or the pixels of the returned image" },
                    "grid": { "type": "boolean", "description": "overlay a labelled 100 px grid and a red crosshair at the action point" },
                    "app": { "type": "string", "description": "package that must be in the foreground; otherwise nothing is done (guards against taps landing in the wrong app)" },
                    "expect": { "type": "string", "description": "after the action wait until this text or description is on screen (up to timeout ms) instead of a fixed wait" },
                    "timeout": { "type": "integer", "description": "expect: max ms to wait (default 5000)" },
                    "times": { "type": "integer", "description": "keyevent: repeat count (up to 10; with fast up to 200)" },
                    "fast": { "type": "boolean", "description": "keyevent: send all presses in ONE adb call (each call costs ~1.7 s on a TV)" },
                    "keys": { "type": "string", "description": "keyevent: a sequence in one call, e.g. \"dpad_right dpad_right enter\" (times repeats it)" },
                    "gap": { "type": "integer", "description": "keyevent: ms between presses, run as one on-device loop" },
                    "nth": { "type": "integer", "description": "goto: pick the nth (1-based, screen order) of several elements that match the label" },
                    "to": { "type": "string", "description": "scroll: keep scrolling until this text appears" },
                    "targets": { "type": "array", "items": { "type": "string" }, "description": "goto: labels to reach with the D-pad, in order; each gets focus, then OK. Text only (no screenshot) unless it fails" },
                    "max": { "type": "integer", "description": "goto: most D-pad presses per target (default 30)" },
                    "keep_going": { "type": "boolean", "description": "scenario: run the remaining steps after a failure (default: stop at the first). steps / json give the scenario; \"app\" or a launch step names the package" },
                    "no_ok": { "type": "boolean", "description": "goto: only move focus, do not press OK" },
                    "list": { "type": "boolean", "description": "focus: also list every focusable element on screen" },
                    "no_shot": { "type": "boolean", "description": "keyevent and other actions: answer with text (result + focus line) instead of a screenshot" },
                    "focus": { "type": "boolean", "description": "add the focus line to the result of a screenshot action" },
                    "device": { "type": "string", "description": "adb serial when several devices are connected" },
                    "steps": { "type": "array", "items": { "type": "object" }, "description": "batch: list of {action, ..., shot} steps (or pass json=<file>)" },
                    "json": { "type": "string", "description": "batch: path of a steps JSON file" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "set_workspace",
            "description": "Optional. The workspace is the project around the folder the server was started in, so most agents never need this. Pins a workspace by path or alias for this server session only (it does not affect other agents or terminals).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Absolute path or registered alias name of project workspace" },
                    "alias": { "type": "string", "description": "Optional custom alias name to assign to this workspace in the registry" }
                },
                "required": ["path"]
            }
        },
        {
            "name": "get_workspace",
            "description": "Retrieve the currently active workspace root path and all registered project aliases in the HashMap.",
            "inputSchema": {
                "type": "object",
                "properties": {}
            }
        },
        {
            "name": "exec",
            "description": "Execute a command inside the active project directory sandbox.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "command": { "type": "string", "description": "Shell command to execute" },
                    "timeout": { "type": "integer", "description": "Seconds before the command is killed (default 30)" },
                    "out": { "type": "string", "description": "File to save the complete output to (kept even if the command is killed at the timeout)" }
                },
                "required": ["command"]
            }
        },
        {
            "name": "check",
            "description": "Fast type-check without building. In a TypeScript project (tsconfig.json) runs tsc (tsc -b for solution-style configs) and returns 'CHECK OK' or 'CHECK FAILED - N error(s)' with the errors only; other projects fall back to lint. Use it after edits, before a full build.",
            "inputSchema": { "type": "object", "properties": { "timeout": { "type": "integer", "description": "Seconds before tsc is killed (default 120)" } } }
        }
    ])
}

    // ── Stdio JSON-RPC / MCP Mode (for IDE / MCP clients) ────────────────────
    // `mysid` with no command is the MCP server. Do not wait on stdin silently: on a terminal show
    // the help, otherwise say on stderr (which MCP clients ignore) what is happening.
    if io::stdin().is_terminal() {
        println!("{}", get_root_help());
        println!("\n(`mysid` with no command starts the MCP server on stdin/stdout; launch it from an MCP client.)");
        return;
    }
    eprintln!("mysid: MCP server ready on stdio (JSON-RPC). For the command line, run `mysid help`.");
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut handle = stdout.lock();

    let workspace_mgr = Arc::new(Mutex::new(WorkspaceManager::new()));

    for line in stdin.lock().lines() {
        let line_text = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line_text.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line_text) {
            Ok(req) => req,
            Err(e) => {
                let err_resp = JsonRpcErrorResponse {
                    jsonrpc: "2.0".to_string(),
                    error: JsonRpcError {
                        code: -32700,
                        message: format!("Parse error: {}", e),
                    },
                    id: json!(null),
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&err_resp).unwrap());
                let _ = handle.flush();
                continue;
            }
        };

        // If request has no id (JSON-RPC notification), do not send any response
        let req_id = match request.id {
            Some(ref id) if !id.is_null() => id.clone(),
            _ => {
                // Notifications: e.g. "notifications/initialized"
                continue;
            }
        };

        let default_root: Option<String> = workspace_mgr.lock().unwrap().get_current();

        // ── Official MCP Standard Methods ────────────────────────────────────
        match request.method.as_str() {
            "initialize" => {
                // Auto-detect rootUri or workspaceFolders from client initialize handshake
                if let Some(root_uri) = request.params.get("rootUri").and_then(|v| v.as_str()) {
                    let cleaned = parse_file_uri(root_uri);
                    if Path::new(&cleaned).is_dir() {
                        workspace_mgr.lock().unwrap().register_and_activate(&cleaned, None);
                    }
                } else if let Some(folders) = request.params.get("workspaceFolders").and_then(|v| v.as_array()) {
                    if let Some(first_folder) = folders.first() {
                        let alias = first_folder.get("name").and_then(|n| n.as_str());
                        if let Some(uri) = first_folder.get("uri").and_then(|u| u.as_str()) {
                            let cleaned = parse_file_uri(uri);
                            if Path::new(&cleaned).is_dir() {
                                workspace_mgr.lock().unwrap().register_and_activate(&cleaned, alias);
                            }
                        }
                    }
                }

                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": {
                            "tools": {},
                            "roots": { "listChanged": true }
                        },
                        "serverInfo": {
                            "name": "mysid",
                            "version": "1.0.0"
                        }
                    }),
                    id: req_id,
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&response).unwrap());
                let _ = handle.flush();
            }
            "ping" => {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({}),
                    id: req_id,
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&response).unwrap());
                let _ = handle.flush();
            }
            "tools/list" => {
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({
                        "tools": get_mcp_tools_list()
                    }),
                    id: req_id,
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&response).unwrap());
                let _ = handle.flush();
            }
            "tools/call" => {
                let tool_name = request.params.get("name").and_then(|v| v.as_str()).unwrap_or("");
                let arguments = request.params.get("arguments").cloned().unwrap_or_else(|| json!({}));

                tools::android::shot::MCP_MODE.store(true, std::sync::atomic::Ordering::Relaxed);
                let output_text = dispatch_method(tool_name, &arguments, default_root.as_deref(), &workspace_mgr);
                let is_error = output_text.starts_with("ERROR") || output_text.starts_with("Error");
                let mut content = vec![json!({ "type": "text", "text": output_text })];
                // Touch commands answer with the screen: one image block per screenshot.
                for png in tools::android::shot::take_images() {
                    content.push(json!({ "type": "image", "data": tools::android::shot::base64(&png), "mimeType": "image/png" }));
                }

                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({
                        "content": content,
                        "isError": is_error
                    }),
                    id: req_id,
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&response).unwrap());
                let _ = handle.flush();
            }
            // ── Direct / Legacy Method Dispatch (Backwards Compatibility) ────
            _ => {
                let output_text = dispatch_method(&request.method, &request.params, default_root.as_deref(), &workspace_mgr);
                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({ "output": output_text }),
                    id: req_id,
                };
                let _ = writeln!(handle, "{}", serde_json::to_string(&response).unwrap());
                let _ = handle.flush();
            }
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn exec_args(a: &[&str]) -> Value {
        parse_exec_args(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }

    #[test]
    fn exec_flags_before_the_command_after_one_quoted_command_and_after_the_separator() {
        // Before the command.
        let p = exec_args(&["--tail", "25", "--errors", "cargo", "build"]);
        assert_eq!((p["tail"].clone(), p["errors"].clone(), p["command"].clone()), (json!("25"), json!(true), json!("cargo build")));
        // After ONE quoted command string, mysid's flags still work.
        let p = exec_args(&["npm run build", "--timeout", "90", "--tail", "30", "--errors"]);
        assert_eq!((p["command"].clone(), p["timeout_secs"].clone(), p["tail"].clone(), p["errors"].clone()), (json!("npm run build"), json!(90), json!("30"), json!(true)));
        // `--` ends mysid's flags: what follows is the command, even `--tail`.
        let p = exec_args(&["--timeout", "5", "--", "docker", "logs", "--tail", "50", "web"]);
        assert_eq!((p["timeout_secs"].clone(), p["command"].clone(), p.get("tail").cloned()), (json!(5), json!("docker logs --tail 50 web"), None));
        // Several unquoted words: a `--tail` among them belongs to the command (docker, tail, git ...).
        let p = exec_args(&["docker", "logs", "--tail", "50", "web"]);
        assert_eq!((p["command"].clone(), p.get("tail").cloned()), (json!("docker logs --tail 50 web"), None));
        // The toolchain environment.
        let (verb, p) = parse_cli_args(&["msvc".to_string(), "cl".to_string(), "/nologo".to_string(), "a.cpp".to_string()]);
        assert_eq!((verb.as_str(), p["env"].clone(), p["command"].clone()), ("exec", json!("msvc"), json!("cl /nologo a.cpp")));
        let (_, p) = parse_cli_args(&["wasm".to_string(), "emcc a.cpp".to_string()]);
        assert_eq!(p["env"], json!("emsdk"));
    }

    #[test]
    fn errors_keeps_only_the_lines_that_say_something_went_wrong() {
        let mut params = json!({ "errors": true });
        let f = OutputFilter::take(&mut params).unwrap();
        let out = f.apply("compiling a.c\nb.c(3): error C2065: 'x': undeclared identifier\nLINK : fatal error LNK1181: cannot open input file\nall fine\n[exit 2]");
        assert!(out.contains("error C2065") && out.contains("LNK1181") && out.contains("[exit 2]") && !out.contains("all fine") && !out.contains("compiling"), "{}", out);
        assert!(out.contains("--errors"), "says what it filtered: {}", out);
    }

    #[test]
    fn typos_get_suggestions_and_nonsense_gets_none() {
        assert_eq!(suggest_commands("bild"), vec!["build"]);
        assert_eq!(suggest_commands("serach"), vec!["search"]);
        assert!(suggest_commands("rel").contains(&"release"), "prefix of a command");
        assert!(suggest_commands("--grpah").contains(&"graph"), "leading dashes are ignored");
        assert!(suggest_commands("zzzzzz").is_empty());
        assert!(suggest_commands("").is_empty());
    }

    #[test]
    fn every_known_command_is_actually_dispatched() {
        let mgr = Arc::new(Mutex::new(WorkspaceManager::new()));
        for cmd in KNOWN_COMMANDS {
            let out = dispatch_method(cmd, &json!({ "help": true }), None, &mgr);
            assert!(!out.starts_with("ERROR: No such command"), "'{}' is listed but not handled", cmd);
        }
        assert!(dispatch_method("definitely-not-a-command", &json!({}), None, &mgr).starts_with("ERROR: No such command"));
    }

    #[test]
    fn exit_codes_reflect_success_failure_and_unknown_commands() {
        assert_eq!(cli_exit_code("ERROR: File not found: x", false), 1);
        assert_eq!(cli_exit_code("{\"success\": false, \"error\": \"boom\"}", false), 1);
        assert_eq!(cli_exit_code("{\"success\": true, \"output\": \"fine\"}", false), 0);
        assert_eq!(cli_exit_code("No matches found.", false), 0);
        assert_eq!(cli_exit_code("ERROR: No such command: 'x'.", true), 2);
    }
}
