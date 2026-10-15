//! `mysid append <path>`: add text to the end of a file, on its own line, keeping its line endings.

use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::util::*;
use super::write::execute_write;
use crate::tools::workspace::{resolve_file_in_workspace, resolve_root};

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
