//! The MCP server: JSON-RPC over stdin / stdout.

pub mod protocol;
pub mod tool_list;

use serde_json::json;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::dispatch::*;
use crate::help::*;
use crate::mcp::tool_list::*;
use crate::mcp::protocol::{JsonRpcError, JsonRpcErrorResponse, JsonRpcRequest, JsonRpcResponse};
use crate::tools;
use crate::tools::workspace::{parse_file_uri, WorkspaceManager};

/// Runs the server until stdin closes.
pub fn run() {
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
