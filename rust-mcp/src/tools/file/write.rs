//! `mysid write <path>`: a new file from stdin / --file; never replaces an existing file unless told to.

use serde_json::Value;
use std::fs;
use std::path::Path;

use super::util::*;
use crate::tools::workspace::resolve_root;

pub fn execute_write(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let Some(name) = path_of(params) else {
        return out(false, "NOT WRITTEN - a file path is required. Example: mysid write src/New.kt --file content.txt");
    };
    let Some(content) = content_of(params) else {
        return out(false, "NOT WRITTEN - no content received. Pipe it in (mysid write src/New.kt <<'EOF' ... EOF), or use --file content.txt.");
    };
    let overwrite = flag(params, &["overwrite", "force"]);
    let root = Path::new(&workspace_root);
    let full = match target_path(root, &name) {
        Ok(p) => p,
        Err(e) => return out(false, &format!("NOT WRITTEN - {}", e)),
    };
    let shown = full.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| full.to_string_lossy().replace('\\', "/"));

    let existing = fs::read_to_string(&full).ok();
    if full.is_dir() {
        return out(false, &format!("NOT WRITTEN - {} is a folder.", shown));
    }
    if full.exists() && !overwrite {
        let size = fs::metadata(&full).map(|m| m.len()).unwrap_or(0);
        return out(
            false,
            &format!("NOT WRITTEN - {} already exists ({} bytes). To replace it use --overwrite; to change part of it use mysid patch or mysid apply.", shown, size),
        );
    }
    // Replacing a CRLF file keeps it CRLF.
    let data = match &existing {
        Some(old) if is_crlf(old) => to_crlf(&content),
        _ => content.clone(),
    };
    if let Some(dir) = full.parent() {
        if let Err(e) = fs::create_dir_all(dir) {
            return out(false, &format!("NOT WRITTEN - cannot create {}: {}", dir.display(), e));
        }
    }
    if let Err(e) = fs::write(&full, data.as_bytes()) {
        return out(false, &format!("NOT WRITTEN - cannot write {}: {}", shown, e));
    }
    let verb = if existing.is_some() { "replaced" } else { "created" };
    out(true, &format!("{} {} ({} line(s), {} bytes)", verb, shown, count_lines(&data), data.len()))
}
