//! Small helpers shared by write, append and edit.

use serde_json::{json, Value};
use std::path::{Component, Path, PathBuf};

pub(super) fn out(success: bool, text: &str) -> String {
    json!({ "success": success, "output": text }).to_string()
}

pub(super) fn content_of(params: &Value) -> Option<String> {
    ["text", "content", "contents"].iter().find_map(|k| params.get(*k).and_then(|v| v.as_str())).map(String::from)
}

pub(super) fn flag(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| params.get(*k).and_then(|v| v.as_bool()).unwrap_or(false))
}

pub(super) fn path_of(params: &Value) -> Option<String> {
    ["path", "file", "filepath", "file_path"]
        .iter()
        .find_map(|k| params.get(*k).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()))
        .map(|s| s.trim().to_string())
}

pub(super) fn is_crlf(content: &str) -> bool {
    content.contains("\r\n") && !content.replace("\r\n", "").contains('\n')
}

pub(super) fn to_crlf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\n', "\r\n")
}

pub(super) fn count_lines(text: &str) -> usize {
    text.lines().count()
}

/// A path for a file that may not exist yet: absolute as given, otherwise inside the workspace (no `..` out of it).
pub(super) fn target_path(root: &Path, name: &str) -> Result<PathBuf, String> {
    let p = Path::new(name);
    if p.is_absolute() {
        return Ok(p.to_path_buf());
    }
    if p.components().any(|c| matches!(c, Component::ParentDir)) {
        return Err(format!("'{}' leaves the workspace (..). Use a path inside it, or an absolute path.", name));
    }
    Ok(root.join(p))
}
