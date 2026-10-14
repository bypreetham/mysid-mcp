use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

use super::workspace::{resolve_file_in_workspace, resolve_root};
use crate::index::{Index, Outline};
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
    let depth = params.get("depth").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map(|d| d as usize);
    let mut tree_blocks: Vec<String> = Vec::new();
    // Signatures come from the code index: a named file is refreshed on its own (no tree walk), a
    // directory refreshes the index once. Files the index does not hold are parsed directly.
    let mut lazy_index: Option<Index> = None;

    for target in &targets {
        let clean_target = target.split('#').next().unwrap_or(target).trim();
        let resolved = match resolve_file_in_workspace(root, clean_target, &workspace_root) {
            Ok(p) => p,
            Err(e) => {
                errors.push(format!("Could not resolve '{}': {}", clean_target, super::workspace::resolution_message(&e)));
                continue;
            }
        };

        if resolved.is_dir() && depth.is_some() {
            // `--depth n`: the folder map (names and code counts), no outlines.
            tree_blocks.push(super::orient::tree(root, &resolved, depth.unwrap_or(1), false));
            continue;
        }
        if resolved.is_dir() {
            let rel_dir = resolved.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            let indexed = Index::open(root).outlines(&rel_dir).unwrap_or_default();
            if !indexed.is_empty() {
                overviews.extend(indexed.into_iter().map(from_outline));
                continue;
            }
            // Nothing indexed there (e.g. only Markdown): walk directory and analyze supported source files
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

            let index = lazy_index.get_or_insert_with(|| Index::open_lazy(root));
            let hit = index
                .refresh_paths(root, &[resolved.clone()])
                .ok()
                .and_then(|_| index.outlines(&rel_str).ok())
                .and_then(|v| v.into_iter().find(|o| o.path == rel_str));
            if let Some(outline) = hit {
                overviews.push(from_outline(outline));
                continue;
            }
            match analyze_file(&resolved, &rel_str) {
                Ok(overview) => overviews.push(overview),
                Err(e) => errors.push(format!("Error reading '{}': {}", rel_str, e)),
            }
        }
    }

    if !tree_blocks.is_empty() && overviews.is_empty() {
        let mut out = tree_blocks.join("\n\n");
        for e in &errors {
            out.push_str(&format!("\n{}", e));
        }
        return out;
    }
    // `--match word`: only the files whose path, or a type / function in them, contains the word.
    if let Some(w) = params.get("match").and_then(|v| v.as_str()).map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()) {
        let before = overviews.len();
        overviews.retain(|o| {
            o.path.to_lowercase().contains(&w) || o.types.iter().any(|t| t.name.to_lowercase().contains(&w)) || o.methods.iter().any(|m| m.name.to_lowercase().contains(&w))
        });
        if overviews.is_empty() && errors.is_empty() {
            return format!("No file, class or function matching '{}' among the {} file(s) here. Try mysid search {}.", w, before, w);
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

fn from_outline(o: Outline) -> FileOverview {
    FileOverview { path: o.path, language: o.language, total_lines: o.lines, types: o.types, methods: o.methods }
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
