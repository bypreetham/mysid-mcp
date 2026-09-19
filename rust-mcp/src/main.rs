mod adapters;
mod protocol;
mod tools;

use protocol::{JsonRpcError, JsonRpcErrorResponse, JsonRpcRequest, JsonRpcResponse};
use serde_json::{json, Value};
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tools::workspace::{parse_file_uri, WorkspaceManager};

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
        "set_workspace" => {
            let path_val = params
                .get("path")
                .or_else(|| params.get("workspace"))
                .or_else(|| params.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            let alias = params.get("alias").and_then(|v| v.as_str());

            if path_val.is_empty() {
                json!({
                    "success": false,
                    "error": "set_workspace requires a `path` or `name` parameter."
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

                        json!({
                            "success": true,
                            "workspace": resolved_path,
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

        "get_workspace" => {
            let mgr = workspace_mgr.lock().unwrap();
            let current = mgr.get_current();
            let known = mgr.get_known_workspaces();
            json!({
                "success": true,
                "workspace": current,
                "known_workspaces": known
            })
            .to_string()
        }

        // ── File / project tools ──────────────────────────────────────────────
        "search" => tools::search::execute_search(params, default_root),
        "read" => tools::read::execute_read(params, default_root),
        "graph" => tools::graph::execute_graph(params, default_root),
        "patch" => tools::patch::execute_patch(params, default_root),
        "patch_batch" => tools::patch::execute_patch_batch(params, default_root),
        "replace" => tools::replace::execute_replace(params, default_root),
        "create_file" | "write" => tools::file::execute_create_file(params, default_root),
        "list_dir" | "ls" => tools::file::execute_list_dir(params, default_root),
        "mkdir" => tools::file::execute_mkdir(params, default_root),
        "rename" | "mv" => tools::file::execute_rename(params, default_root),
        "delete" | "rm" => tools::file::execute_delete(params, default_root),
        "file" => tools::file::execute_file(params, default_root),
        "exec" | "run" => tools::exec::execute_exec(params, default_root),
        "lint" | "check" => tools::lint::execute_lint(params, default_root),
        "symbols" => tools::symbols::execute_symbols(params, default_root),
        "build" => tools::build::execute_build(params, default_root),
        "release" | "aab" => tools::build::execute_release(params, default_root),
        "install" => tools::build::execute_install(params, default_root),
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
        "inspect_symbol" => tools::read::execute_read(params, default_root),
        _ => format!("ERROR: Unknown tool method '{}'", method),
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

fn parse_cli_args(args: &[String]) -> (String, Value) {
    let verb = args[0].clone();
    let mut params = serde_json::Map::new();
    let mut positionals = Vec::new();
    let mut i = 1;

    // Check for --json <file> or --json '<raw_json>'
    while i < args.len() {
        let a = &args[i];
        if a == "--json" && i + 1 < args.len() {
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

    if verb == "set_workspace" {
        params.insert("path".to_string(), json!(positionals.join(" ")));
    } else if verb == "list_dir" || verb == "ls" {
        if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
        }
    } else if verb == "search" {
        if !positionals.is_empty() {
            params.insert("query".to_string(), json!(positionals.join(" ")));
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
        }
    } else if verb == "mkdir" {
        if !positionals.is_empty() {
            params.insert("path".to_string(), json!(positionals[0]));
        }
    } else if verb == "rename" || verb == "mv" {
        if positionals.len() > 0 {
            params.insert("old_path".to_string(), json!(positionals[0]));
        }
        if positionals.len() > 1 {
            params.insert("new_path".to_string(), json!(positionals[1]));
        }
    } else if verb == "create_file" || verb == "write" {
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
                if positionals.len() > 1 {
                    params.insert("path".to_string(), json!(positionals[1]));
                }
                if positionals.len() > 2 {
                    params.insert("content".to_string(), json!(positionals[2..].join(" ")));
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
    } else if verb == "graph" {
        if params.contains_key("help") || params.contains_key("h") || (positionals.len() == 1 && (positionals[0] == "help" || positionals[0] == "--help" || positionals[0] == "-h")) {
            params.insert("mode".to_string(), json!("help"));
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
            lines.push("# F = file, D = directory".to_string());
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
            return format!("ERROR: {}", err);
        }
        serde_json::to_string_pretty(&val).unwrap_or_else(|_| raw.to_string())
    } else {
        raw.to_string()
    }
}

fn get_root_help() -> &'static str {
    "mysid: High-performance standalone codebase intelligence and MCP tool server\n\n\
Usage:\n  \
  mysid <command> [arguments...]\n  \
  mysid <command> --help             (show detailed documentation and examples for any command)\n\n\
Core Commands:\n  \
  mysid read <symbol | file#Lrange>   AST symbol definition + call sites, or file slices\n  \
  mysid search <query>                Fast regex/text grep with --types, --max, and context\n  \
  mysid mkdir <path>                  Create directory recursively (auto-creates parents)\n  \
  mysid patch <file> <old> <new>      Surgical search-and-replace in a single file\n  \
  mysid patch_batch --json <file>     Atomic multi-file batch patching\n  \
  mysid replace <old> <new>           Global search-and-replace across files (--types, --dry-run)\n  \
  mysid file <action>                 File utilities: new, rename, delete, list ('file --help')\n  \
  mysid graph [--mode overview|flow]  Architectural call-flow & impact analysis ('graph --help')\n\n\
Build & Delivery:\n  \
  mysid build [clean]                 Fast assembleDebug (or clean assembleDebug) with token-filtered errors\n  \
  mysid release                       Compile release AAB bundle, verify bundle & R8 mapping\n  \
  mysid install                       Deploy built APK directly to connected Android device\n\n\
Device & Environment:\n  \
  mysid android <action>              ADB utilities: devices, wifi, logcat, input ('android --help')\n  \
  mysid devices                       Quick shortcut: list connected ADB devices\n  \
  mysid set_workspace <path>          Switch active repository/project directory\n  \
  mysid get_workspace                 Show currently active repository path\n  \
  mysid exec <command>                Execute shell command in project directory\n"
}

fn get_tool_help(tool: &str) -> Option<&'static str> {
    match tool.to_ascii_lowercase().as_str() {
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
            "mysid read: AST symbol inspection & file slice retrieval\n\n\
Description:\n  \
  High-performance code reader. Automatically differentiates between AST symbol lookups,\n  \
  single line-range slices, batch slices, and Markdown outlines.\n\n\
Usage:\n  \
  mysid read <symbol>                 Inspect symbol definition and top reference/call sites\n  \
  mysid read <file#Lstart-end>        Read a slice of a file (e.g. File.kt#L10-50 or File.kt:10-50)\n  \
  mysid read <slice1> <slice2> ...    Batch read multiple slices in ONE turn (Zero Peeking Rule)\n  \
  mysid read <file.md>                Print high-level outline of Markdown headings\n\n\
Examples:\n  \
  mysid read StreamResolver\n  \
  mysid read \"MusicService.kt#L100-150\"\n  \
  mysid read \"AudioEngine.cpp#L40-80\" \"Player.kt#L200-240\"\n  \
  mysid read README.md\n"
        ),
        "search" => Some(
            "mysid search: Fast multithreaded codebase grep\n\n\
Description:\n  \
  High-speed regex/string search across the workspace respecting .gitignore.\n  \
  Returns token-lean 1-line hits (file:line: snippet).\n\n\
Usage:\n  \
  mysid search <query> [options]\n\n\
Options:\n  \
  --types <ext1,ext2>   Filter by file extensions (e.g. --types kt,cpp)\n  \
  --max <n>             Maximum results to return (default: 15, max: 25)\n  \
  --case                Case-sensitive search (default: false)\n  \
  --regex               Treat query as regular expression (default: false)\n  \
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
  Fails cleanly if the target string is not unique or not found.\n\n\
Usage:\n  \
  mysid patch <file> <old_string> <new_string>\n\n\
Examples:\n  \
  mysid patch app/build.gradle.kts \"versionCode = 44\" \"versionCode = 45\"\n"
        ),
        "patch_batch" => Some(
            "mysid patch_batch: Multi-file atomic batch patching\n\n\
Description:\n  \
  Applies multiple file patches atomically from a JSON specification file or string.\n\n\
Usage:\n  \
  mysid patch_batch --json <file.json>\n"
        ),
        "file" => Some(
            "mysid file: Filesystem management utilities\n\n\
Description:\n  \
  Unified file lifecycle operations. Auto-creates parent directories on write/move.\n\n\
Usage:\n  \
  mysid file new <path> <content>         Create or overwrite file (auto-creates parent dirs)\n  \
  mysid file mkdir <path>                 Recursively create directory\n  \
  mysid file rename <old_path> <new_path> Rename or move file/directory\n  \
  mysid file delete <path> [--recursive]  Delete file or directory\n  \
  mysid file list [path] [-L<depth>]      List files (defaults to -L1, respects .gitignore)\n\n\
Examples:\n  \
  mysid file new config/settings.json \"{\\\"theme\\\":\\\"dark\\\"}\"\n  \
  mysid file rename old_name.kt new_name.kt\n  \
  mysid file delete temp_cache --recursive\n  \
  mysid file list app -L2\n"
        ),
        "graph" => Some(
            "mysid graph: Architectural code graph & call-flow intelligence\n\n\
Description:\n  \
  Analyzes project symbols, call relationships, and package dependencies.\n\n\
Usage:\n  \
  mysid graph [--mode overview] [--filter <pkg>]  Generate package/module dependency overview\n  \
  mysid graph --mode flow --query <symbol>        Trace downstream call graph\n  \
  mysid graph --mode impact --query <symbol>      Trace upstream callers / blast radius\n  \
  mysid graph --mode symbol --query <symbol>      Inspect node metadata and connections\n\n\
Examples:\n  \
  mysid graph --mode overview\n  \
  mysid graph --mode flow --query StreamResolver\n  \
  mysid graph --mode impact --query PlaybackEngineController\n"
        ),
        "build" => Some(
            "mysid build: Native Android Gradle compilation\n\n\
Description:\n  \
  Compiles the Android project via Gradle assembleDebug. Automatically intercepts\n  \
  and filters compilation noise, returning only actionable compiler errors (~70 tokens).\n\n\
Usage:\n  \
  mysid build         Fast incremental debug build\n  \
  mysid build clean   Purge stale native .so files & intermediate caches, then rebuild\n\n\
Examples:\n  \
  mysid build\n  \
  mysid build clean\n"
        ),
        "release" | "aab" => Some(
            "mysid release: Production Android App Bundle (.aab) generation\n\n\
Description:\n  \
  Executes bundleRelease, verifies app-release.aab creation, validates R8 mapping.txt,\n  \
  and outputs bundle file size telemetry.\n\n\
Usage:\n  \
  mysid release\n\n\
Examples:\n  \
  mysid release\n"
        ),
        "install" => Some(
            "mysid install: Deploy APK to connected Android device\n\n\
Description:\n  \
  Finds the latest built debug APK and deploys it directly to the target device via ADB.\n\n\
Usage:\n  \
  mysid install\n\n\
Examples:\n  \
  mysid install\n"
        ),
        "android" | "adb" => Some(
            "mysid android: Android device & ADB utilities\n\n\
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
  mysid android tap --x <x> --y <y>           Simulate touch screen tap\n  \
  mysid android type --text <text>            Simulate text keyboard input\n  \
  mysid android keyevent --keycode <code|key> Send keycode (e.g. KEYCODE_BACK)\n"
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
        "set_workspace" => Some(
            "mysid set_workspace: Set active project workspace\n\n\
Description:\n  \
  Updates the persisted active workspace path and automatically returns the top-level (L1)\n  \
  directory entries to provide immediate project context without extra roundtrips.\n\n\
Usage:\n  \
  mysid set_workspace <path>\n\n\
Examples:\n  \
  mysid set_workspace E:\\Hypersonus\\v44\n"
        ),
        "get_workspace" => Some(
            "mysid get_workspace: Get currently active workspace path\n\n\
Usage:\n  \
  mysid get_workspace\n"
        ),
        "exec" | "run" => Some(
            "mysid exec: Execute command in workspace sandbox\n\n\
Usage:\n  \
  mysid exec <command>\n\n\
Examples:\n  \
  mysid exec \"git status\"\n  \
  mysid exec \"adb devices\"\n"
        ),
        _ => None,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // ── CLI Mode (standalone terminal execution without Python) ───────────────
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

        let workspace_mgr = Arc::new(Mutex::new(WorkspaceManager::new()));
        let (verb, mut params) = parse_cli_args(&args);
        let active_ws = workspace_mgr.lock().unwrap().get_current();

        // Inject workspace if not explicitly provided
        if let Some(ref ws) = active_ws {
            if let Some(obj) = params.as_object_mut() {
                obj.entry("workspace_root").or_insert_with(|| json!(ws));
                obj.entry("project_path").or_insert_with(|| json!(ws));
            }
        }

        let output = dispatch_method(&verb, &params, active_ws.as_deref(), &workspace_mgr);
        println!("{}", format_output(&output));
        return;
    }

fn get_mcp_tools_list() -> Value {
    json!([
        {
            "name": "read",
            "description": "AST symbol lookup or batch file slice inspection. Bare filenames are automatically resolved via LRU cache without full workspace paths. Bundle all file slices into a single call.",
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
            "name": "search",
            "description": "High-speed multi-threaded regex/string search respecting .gitignore.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search pattern or regex" },
                    "types": { "type": "string", "description": "Optional comma-separated file extensions (e.g. 'kt,cpp,rs')" },
                    "max_results": { "type": "integer", "description": "Maximum match entries to return (default: 50)" },
                    "context": { "type": "integer", "description": "Context lines around each match (default: 1)" }
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
                    "path": { "type": "string", "description": "Optional file path or subdirectory to restrict replacement" },
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
                    "file": { "type": "string", "description": "Path to target file" },
                    "old": { "type": "string", "description": "Exact original code chunk to replace" },
                    "new": { "type": "string", "description": "New replacement code chunk" }
                },
                "required": ["file", "old", "new"]
            }
        },
        {
            "name": "patch_batch",
            "description": "Atomic multi-file batch patching across multiple files.",
            "inputSchema": {
                "type": "object",
                "properties": {
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
                        "description": "List of patch operations to apply atomically"
                    }
                },
                "required": ["patches"]
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
                    "path": { "type": "string", "description": "Target file or directory path" },
                    "content": { "type": "string", "description": "Content for new file" },
                    "new_path": { "type": "string", "description": "Destination path for rename" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "graph",
            "description": "Architectural call-flow, dependency, and impact analysis.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "mode": { "type": "string", "enum": ["overview", "flow"], "description": "Graph analysis mode" },
                    "entry": { "type": "string", "description": "Optional entry point symbol or function" }
                }
            }
        },
        {
            "name": "build",
            "description": "Android Gradle compile runner with token-filtered errors and build optimization.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["build", "clean", "release", "install"], "description": "Build action to perform" }
                }
            }
        },
        {
            "name": "android",
            "description": "Android ADB device bridge and device control utilities.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["devices", "connect_wifi", "install", "launch_app", "logcat", "crashes", "clear_logcat", "tap", "type", "keyevent"],
                        "description": "Android device command"
                    },
                    "package_name": { "type": "string", "description": "Application package ID (optional)" },
                    "lines": { "type": "integer", "description": "Logcat line limit (default: 50)" },
                    "x": { "type": "integer", "description": "X coordinate for tap" },
                    "y": { "type": "integer", "description": "Y coordinate for tap" },
                    "text": { "type": "string", "description": "Text to type" },
                    "key": { "type": "string", "description": "Key code for keyevent" }
                },
                "required": ["action"]
            }
        },
        {
            "name": "set_workspace",
            "description": "Set active project workspace by path or registered alias (e.g. 'mysid-mcp', 'hypersonus'). Optional if already opened in project root.",
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
                    "command": { "type": "string", "description": "Shell command to execute" }
                },
                "required": ["command"]
            }
        }
    ])
}

    // ── Stdio JSON-RPC / MCP Mode (for IDE / MCP clients) ────────────────────
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

                let output_text = dispatch_method(tool_name, &arguments, default_root.as_deref(), &workspace_mgr);
                let is_error = output_text.starts_with("ERROR") || output_text.starts_with("Error");

                let response = JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: json!({
                        "content": [
                            {
                                "type": "text",
                                "text": output_text
                            }
                        ],
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

