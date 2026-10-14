use ignore::{WalkBuilder, WalkState};
use std::sync::Mutex;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::adapters::detect_adapter;
use crate::parsers::find_parser;

/// A source file on disk that some parser can index.
pub struct ScannedFile {
    /// Workspace-relative path with `/` separators.
    pub rel: String,
    pub path: PathBuf,
    pub language: String,
    pub mtime: i64,
    pub size: i64,
}

/// Cheap checks first (extension -> parser), so only files that can be indexed cost a `stat`:
/// walking a tree full of images and audio must not stat every one of them.
fn describe(root: &Path, path: &Path, metadata: impl FnOnce() -> Option<Metadata>) -> Option<ScannedFile> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let parser = find_parser(&ext)?;
    if parser.language_name() == "Markdown" {
        return None;
    }
    let meta = metadata()?;
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
}

/// Lists indexable source files (gitignore-aware, minus the stack adapter's skipped dirs), sorted by path.
/// The walk runs on several threads: it is the cost of every refresh, and it scales with the tree.
pub fn scan(root: &Path) -> Vec<ScannedFile> {
    let adapter = detect_adapter(root);
    let base = root.to_path_buf();
    let found: Mutex<Vec<ScannedFile>> = Mutex::new(Vec::new());

    WalkBuilder::new(root)
        .filter_entry(move |e| {
            let rel = e.path().strip_prefix(&base).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            e.file_name().to_str().map_or(true, |n| !adapter.is_skipped_entry(&rel, n))
        })
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                // The walker already knows each entry's type from the directory listing; no stat needed.
                if let Ok(e) = entry {
                    if e.file_type().map_or(false, |t| t.is_file()) {
                        if let Some(file) = describe(root, e.path(), || e.metadata().ok()) {
                            found.lock().unwrap().push(file);
                        }
                    }
                }
                WalkState::Continue
            })
        });

    // Threads finish in arbitrary order; sort so the result is deterministic.
    let mut out = found.into_inner().unwrap();
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

/// Every file under `root` (any type) as workspace-relative `/` paths, sorted: gitignore-aware and minus the
/// stack adapter's skipped dirs. Used to turn a bare file name into a path and a path back into a short name.
pub fn list_files(root: &Path) -> Vec<String> {
    let adapter = detect_adapter(root);
    let base = root.to_path_buf();
    let found: Mutex<Vec<String>> = Mutex::new(Vec::new());
    WalkBuilder::new(root)
        .filter_entry(move |e| {
            let rel = e.path().strip_prefix(&base).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_default();
            e.file_name().to_str().map_or(true, |n| !adapter.is_skipped_entry(&rel, n))
        })
        .build_parallel()
        .run(|| {
            Box::new(|entry| {
                if let Ok(e) = entry {
                    if e.file_type().map_or(false, |t| t.is_file()) {
                        if let Ok(rel) = e.path().strip_prefix(root) {
                            found.lock().unwrap().push(rel.to_string_lossy().replace('\\', "/"));
                        }
                    }
                }
                WalkState::Continue
            })
        });
    let mut out = found.into_inner().unwrap();
    out.sort();
    out.dedup();
    out
}

/// One file, for incremental updates. None when it is missing, unindexable, or inside a directory
/// the stack adapter skips. (Unlike `scan`, `.gitignore` rules are not consulted here.)
pub fn scan_one(root: &Path, path: &Path) -> Option<ScannedFile> {
    let rel = path.strip_prefix(root).ok()?;
    let adapter = detect_adapter(root);

    let comps: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let mut acc = String::new();
    for name in comps.iter().take(comps.len().saturating_sub(1)) {
        if !acc.is_empty() {
            acc.push('/');
        }
        acc.push_str(name);
        if adapter.is_skipped_entry(&acc, name) {
            return None;
        }
    }
    describe(root, path, || std::fs::metadata(path).ok().filter(|m| m.is_file()))
}
