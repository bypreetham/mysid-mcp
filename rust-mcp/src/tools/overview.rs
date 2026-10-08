use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::{resolve_file_in_workspace, resolve_root};
pub use crate::parsers::{find_parser, is_supported_extension, MethodSignature, TypeInfo};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileOverview {
    pub path: String,
    pub language: String,
    pub total_lines: usize,
    pub types: Vec<TypeInfo>,
    pub methods: Vec<MethodSignature>,
}

pub fn execute_overview(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&workspace_root);

    let format_json = params
        .get("format")
        .and_then(|v| v.as_str())
        .map(|f| f.eq_ignore_ascii_case("json"))
        .unwrap_or(false);

    // Collect all target files
    let mut targets: Vec<String> = Vec::new();

    if let Some(arr) = params.get("files").and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(s) = item.as_str() {
                if !s.trim().is_empty() {
                    targets.push(s.trim().to_string());
                }
            }
        }
    }

    if targets.is_empty() {
        if let Some(single) = params
            .get("path")
            .or_else(|| params.get("file"))
            .or_else(|| params.get("query"))
            .and_then(|v| v.as_str())
        {
            for token in single.split_whitespace() {
                if !token.is_empty() {
                    targets.push(token.to_string());
                }
            }
        }
    }

    if targets.is_empty() {
        return json!({
            "success": false,
            "error": "overview requires a `path`, `file`, or `files` parameter."
        })
        .to_string();
    }

    let mut overviews: Vec<FileOverview> = Vec::new();
    let mut errors: Vec<String> = Vec::new();

    for target in &targets {
        let clean_target = target.split('#').next().unwrap_or(target).trim();
        let resolved = match resolve_file_in_workspace(root, clean_target, &workspace_root) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("Could not resolve '{}': {}", clean_target, e));
                continue;
            }
        };

        if resolved.is_dir() {
            // Walk directory and analyze supported source files
            for entry in WalkBuilder::new(&resolved)
                .hidden(false)
                .git_ignore(true)
                .git_global(true)
                .git_exclude(true)
                .build()
                .filter_map(|r| r.ok())
            {
                let path = entry.path();
                if path.is_file() && is_supported_file(path) {
                    if let Ok(rel) = path.strip_prefix(root) {
                        let rel_str = rel.to_string_lossy().replace('\\', "/");
                        match analyze_file(path, &rel_str) {
                            Ok(overview) => overviews.push(overview),
                            Err(e) => errors.push(format!("Error reading '{}': {}", rel_str, e)),
                        }
                    }
                }
            }
        } else if resolved.is_file() {
            let rel_str = resolved
                .strip_prefix(root)
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| clean_target.to_string());

            match analyze_file(&resolved, &rel_str) {
                Ok(overview) => overviews.push(overview),
                Err(e) => errors.push(format!("Error reading '{}': {}", rel_str, e)),
            }
        }
    }

    if format_json {
        json!({
            "success": errors.is_empty(),
            "count": overviews.len(),
            "files": overviews,
            "errors": errors
        })
        .to_string()
    } else {
        render_human_readable(&overviews, &errors)
    }
}

fn is_supported_file(path: &Path) -> bool {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    is_supported_extension(ext)
}

fn analyze_file(path: &Path, rel_path: &str) -> Result<FileOverview, String> {
    let content = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();

    let parser = find_parser(&ext)
        .ok_or_else(|| format!("No parser found for extension '{}'", ext))?;

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let (types, methods) = parser.parse(&lines);

    Ok(FileOverview {
        path: rel_path.to_string(),
        language: parser.language_name().to_string(),
        total_lines,
        types,
        methods,
    })
}

// ── Human-Readable Outline Renderer ─────────────────────────────────────────

fn render_human_readable(overviews: &[FileOverview], errors: &[String]) -> String {
    let mut out = String::new();

    for overview in overviews {
        out.push_str(&format!(
            "=== {} ({}) [{} lines] ===\n",
            overview.path, overview.language, overview.total_lines
        ));

        if overview.language == "Markdown" {
            if overview.methods.is_empty() {
                out.push_str("  (no headings detected)\n\n");
            } else {
                for m in &overview.methods {
                    out.push_str(&format!("  [L{}] {}\n", m.start_line, m.signature));
                }
                out.push('\n');
            }
            continue;
        }

        if !overview.types.is_empty() {
            for t in &overview.types {
                out.push_str(&format!("  [L{}] {} {}\n", t.line, t.kind, t.name));
            }
        }

        if overview.methods.is_empty() {
            out.push_str("  (no method signatures detected)\n\n");
            continue;
        }

        // Group methods by parent or show top-level
        for m in &overview.methods {
            let parent_prefix = if let Some(ref p) = m.parent {
                format!("[{}] ", p)
            } else {
                String::new()
            };
            out.push_str(&format!("    [L{}] {}{}\n", m.start_line, parent_prefix, m.signature));
        }
        out.push('\n');
    }

    if !errors.is_empty() {
        out.push_str("Errors encountered:\n");
        for err in errors {
            out.push_str(&format!("  - {}\n", err));
        }
    }

    out.trim_end().to_string()
}
