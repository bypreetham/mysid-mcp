use ignore::WalkBuilder;
use serde_json::Value;
use std::fs;
use std::path::Path;

use super::workspace::resolve_root;

const BINARY_EXTENSIONS: &[&str] = &[
    "so", "dll", "exe", "dylib", "a", "lib", "o", "obj",
    "jar", "aab", "apk", "class", "pyc", "wasm",
    "png", "jpg", "jpeg", "webp", "gif", "ico", "bmp", "tiff", "svgz",
    "zip", "tar", "gz", "7z", "rar", "bin", "dat",
    "keystore", "jks",
];

pub fn execute_replace(params: &Value, default_root: Option<&str>) -> String {
    let workspace_root = match resolve_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };

    let old_string = match params
        .get("old_string")
        .or_else(|| params.get("from"))
        .or_else(|| params.get("search"))
        .or_else(|| params.get("find"))
        .and_then(|v| v.as_str())
    {
        Some(s) if !s.is_empty() => s,
        _ => return "ERROR: replace requires an `old_string` (search term). Example: mysid replace \"old\" \"new\"".to_string(),
    };

    let new_string = match params
        .get("new_string")
        .or_else(|| params.get("to"))
        .or_else(|| params.get("replace"))
        .or_else(|| params.get("with"))
        .and_then(|v| v.as_str())
    {
        Some(s) => s,
        _ => return "ERROR: replace requires a `new_string` (replacement term). Example: mysid replace \"old\" \"new\"".to_string(),
    };

    let dry_run = params
        .get("dry_run")
        .or_else(|| params.get("dry-run"))
        .or_else(|| params.get("preview"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Optional file type filter: e.g. ["kt", "cpp"] or comma-separated string "kt,cpp"
    let file_types: Vec<String> = params
        .get("file_types")
        .or_else(|| params.get("types"))
        .map(|v| {
            if let Some(arr) = v.as_array() {
                arr.iter()
                    .filter_map(|s| s.as_str())
                    .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
                    .collect()
            } else if let Some(s) = v.as_str() {
                s.split(',')
                    .map(|item| item.trim().trim_start_matches('.').to_ascii_lowercase())
                    .filter(|item| !item.is_empty())
                    .collect()
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();

    let root = Path::new(&workspace_root);

    // If explicit target file was provided, operate strictly on that file
    if let Some(target_file) = params.get("file").or_else(|| params.get("target_file")).and_then(|v| v.as_str()) {
        let full_path = if Path::new(target_file).is_absolute() {
            target_file.into()
        } else {
            root.join(target_file)
        };

        if !full_path.is_file() {
            return format!("ERROR: Target file not found: {}", full_path.display());
        }

        let content = match fs::read_to_string(&full_path) {
            Ok(c) => c,
            Err(e) => return format!("ERROR: Failed to read file {}: {}", target_file, e),
        };

        let matches = content.matches(old_string).count();
        if matches == 0 {
            return format!("No occurrences of \"{}\" found in {}.", old_string, target_file);
        }

        if dry_run {
            return format!("[DRY RUN] Would replace {} occurrence(s) of \"{}\" with \"{}\" in {}", matches, old_string, new_string, target_file);
        }

        let new_content = content.replace(old_string, new_string);
        return match fs::write(&full_path, new_content) {
            Ok(_) => format!("Replaced {} occurrence(s) in {}", matches, target_file),
            Err(e) => format!("ERROR: Failed to write file {}: {}", target_file, e),
        };
    }

    // Subdirectory restriction
    let search_dir = if let Some(sub) = params.get("path").and_then(|v| v.as_str()) {
        if Path::new(sub).is_absolute() {
            sub.into()
        } else {
            root.join(sub)
        }
    } else {
        root.to_path_buf()
    };

    if !search_dir.exists() {
        return format!("ERROR: Directory not found: {}", search_dir.display());
    }

    let mut builder = WalkBuilder::new(&search_dir);
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true);

    let mut results: Vec<(String, usize)> = Vec::new();
    let mut total_replacements = 0;

    for result in builder.build().filter_map(|e| e.ok()) {
        if !result.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }

        let path = result.path();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // Skip known binary file formats
        if BINARY_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }

        // Apply file type filter if specified
        if !file_types.is_empty() && !file_types.contains(&ext) {
            continue;
        }

        // Read file (skipping non-UTF8 / binary files)
        let Ok(content) = fs::read_to_string(path) else {
            continue;
        };

        let count = content.matches(old_string).count();
        if count == 0 {
            continue;
        }

        let rel_path = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

        if !dry_run {
            // Preserve original line endings (CRLF / LF)
            let new_content = content.replace(old_string, new_string);
            if let Err(e) = fs::write(path, new_content) {
                return format!("ERROR: Failed to write {}: {}", rel_path, e);
            }
        }

        total_replacements += count;
        results.push((rel_path, count));
    }

    if results.is_empty() {
        return format!("No occurrences of \"{}\" found across the workspace.", old_string);
    }

    let action_label = if dry_run { "[DRY RUN] Would replace" } else { "Replaced" };
    let noun = if dry_run { "occurrence" } else { "replacement" };
    let noun_plural = if dry_run { "occurrences" } else { "replacements" };

    let mut out = format!(
        "{} \"{}\" with \"{}\" across {} file(s) ({} {}):\n",
        action_label,
        old_string,
        new_string,
        results.len(),
        total_replacements,
        if total_replacements == 1 { noun } else { noun_plural }
    );

    for (file, count) in &results {
        out.push_str(&format!(
            "  • {} ({} {})\n",
            file,
            count,
            if *count == 1 { noun } else { noun_plural }
        ));
    }

    out
}
