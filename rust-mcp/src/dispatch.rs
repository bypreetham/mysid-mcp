//! The one table from command name to handler, shared by the command line and the MCP server.

use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::cli::output::*;
use crate::cli::suggest::*;
use crate::help::*;
use crate::tools;
use crate::tools::workspace::WorkspaceManager;

/// Every command name `mysid` accepts (aliases included): the source for "did you mean" hints.
pub(crate) const KNOWN_COMMANDS: &[&str] = &[
    "read", "peek", "search", "graph", "patch", "patch_batch", "replace", "edit", "create_file", "write", "append",
    "list_dir", "ls", "dir", "listdir", "mkdir", "rename", "mv", "delete", "rm", "file", "exec", "run",
    "lint", "check", "symbols", "overview", "signatures", "build", "release", "aab", "install",
    "apply", "list", "commit", "history", "checkpoint", "diff", "restore", "skill", "new", "debug", "dev", "deps", "test", "status", "branch", "copy", "cp", "publish", "bootstrap", "doctor", "health", "clean", "tree", "init", "vcs", "git", "purge", "msvc", "cl", "emsdk", "wasm", "android", "devices", "wifi", "connect_wifi", "inspect_symbol", "set", "set_workspace", "setworkspace",
    "get_workspace", "getworkspace", "help",
];

pub(crate) fn dispatch_method(method: &str, params: &Value, default_root: Option<&str>, workspace_mgr: &Arc<Mutex<WorkspaceManager>>) -> String {
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
        "patch" => tools::file::execute_patch(params, default_root),
        "patch_batch" => tools::file::execute_patch_batch(params, default_root),
        "replace" => tools::file::execute_replace(params, default_root),
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
        "check" => tools::build::check::execute_check(params, default_root),
        "symbols" => tools::scanner::symbols::execute_symbols(params, default_root),
        "overview" | "signatures" => shorten_paths(tools::scanner::overview::execute_overview(params, default_root), params, default_root),
        "build" => tools::build::execute_build(params, default_root),
        "release" | "aab" => tools::build::execute_release(params, default_root),
        "install" => tools::build::execute_install(params, default_root),
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
        "clean" => {
            let mut p = params.clone();
            p["sub"] = json!("clean");
            tools::doctor::execute_doctor(&p, default_root)
        }
        "health" => {
            let mut p = params.clone();
            p["sub"] = json!("health");
            tools::doctor::execute_doctor(&p, default_root)
        }
        "tree" => tools::scanner::orient::execute_tree(params, default_root),
        "skill" => tools::skill::execute_skill(params, default_root),
        "new" => tools::scaffold::execute_new(params, default_root),
        "apply" => tools::file::execute_apply(params, default_root),

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
