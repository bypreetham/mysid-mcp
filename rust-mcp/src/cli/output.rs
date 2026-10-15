//! What the command line prints: shortened paths, formatted output, exit codes.

use serde_json::Value;
use std::path::Path;

use crate::tools;

/// Process exit status for CLI use: 2 for an unknown command, 1 when the tool reported a failure
/// (`ERROR: ...` text, or JSON with `"success": false`), 0 otherwise. Scripts and `&&` chains
/// rely on this; the text output is unchanged.
pub(crate) fn cli_exit_code(output: &str, unknown_command: bool) -> i32 {
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
pub(crate) fn shorten_paths(output: String, params: &Value, default_root: Option<&str>) -> String {
    if !output.contains('/') || output.starts_with("ERROR") {
        return output;
    }
    match tools::workspace::resolve_existing_root(params, "workspace_root", default_root) {
        Ok(root) => tools::scanner::paths::Shortener::new(Path::new(&root)).apply(&output),
        Err(_) => output,
    }
}

pub(crate) fn format_output(raw: &str) -> String {
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
