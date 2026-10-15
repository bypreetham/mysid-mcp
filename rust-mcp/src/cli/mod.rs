//! The command line: argument parsing, output filters, "did you mean" hints, and `run`, the entry for a command.

pub mod args;
pub mod filter;
pub mod output;
pub mod suggest;

use serde_json::json;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::cli::args::*;
use crate::cli::filter::*;
use crate::cli::output::*;
use crate::dispatch::*;
use crate::help::*;
use crate::tools;
use crate::tools::workspace::WorkspaceManager;

/// Handles a command line. `true` when the arguments were handled (the process is done); `false` when there are none and
/// the MCP server should start.
pub fn run(args: &[String]) -> bool {

    // Internal: the detached process `mysid build` starts to run a build (see tools/build/worker.rs).
    if args.len() == 2 && args[0] == "__build_worker" {
        tools::build::worker::run_worker(&args[1]);
        return true;
    }

    // Internal: the detached logcat recorder `mysid debug` starts (see tools/build/debug.rs).
    if args.len() == 2 && args[0] == "__debug_recorder" {
        tools::build::debug::run_recorder(&args[1]);
        return true;
    }

    // ── CLI Mode (standalone terminal execution without Python) ───────────────
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-V" | "version") {
        println!("mysid {}", env!("CARGO_PKG_VERSION"));
        return true;
    }
    // `mysid help --claude` / `--codex` / `--antigravity`: help written in that client's own tool vocabulary.
    if let Some(client) = args.iter().find_map(|a| a.strip_prefix("--").filter(|c| matches!(*c, "claude" | "codex" | "antigravity" | "gemini"))) {
        if args[0] == "help" || args[0].starts_with("--") {
            if let Some(text) = client_help(client) {
                println!("{}", text);
                return true;
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
                    return true;
                }
            }

            println!("{}", get_root_help());
            return true;
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
    false
}
