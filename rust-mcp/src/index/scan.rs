use ignore::WalkBuilder;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::parsers::find_parser;

const SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "venv", ".venv", "__pycache__", "target", "build", "dist", ".gradle",
    ".idea", ".vscode",
];

/// A source file on disk that some parser can index.
pub struct ScannedFile {
    /// Workspace-relative path with `/` separators.
    pub rel: String,
    pub path: PathBuf,
    pub language: String,
    pub mtime: i64,
    pub size: i64,
}

/// Lists indexable source files (gitignore-aware), sorted by path.
pub fn scan(root: &Path) -> Vec<ScannedFile> {
    let mut out: Vec<ScannedFile> = WalkBuilder::new(root)
        .filter_entry(|e| e.file_name().to_str().map_or(true, |n| !SKIP_DIRS.contains(&n)))
        .build()
        .filter_map(|r| r.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let path = e.path();
            let ext = path.extension()?.to_str()?.to_lowercase();
            let parser = find_parser(&ext)?;
            if parser.language_name() == "Markdown" {
                return None;
            }
            let meta = e.metadata().ok()?;
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            Some(ScannedFile {
                rel: path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/"),
                path: path.to_path_buf(),
                language: parser.language_name().to_string(),
                mtime,
                size: meta.len() as i64,
            })
        })
        .collect();
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}
