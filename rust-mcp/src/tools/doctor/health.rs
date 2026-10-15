//! `mysid health <folder>`: where the space goes. Total size, the largest folders and the largest files.

use serde_json::{json, Value};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::fs;
use std::path::{Path, PathBuf};

use super::compare::{human, thousands, GENERATED};

/// Entries visited before the walk stops (a drive root can hold millions).
const MAX_ENTRIES: usize = 3_000_000;

struct DirNode {
    rel: String,
    parent: Option<usize>,
    depth: usize,
    size: u64,
    files: usize,
}

pub(crate) struct Health {
    pub total_bytes: u64,
    pub files: usize,
    pub folders: usize,
    /// (size, folder path relative to the root), largest first.
    pub top_folders: Vec<(u64, String)>,
    /// (size, file path relative to the root), largest first.
    pub top_files: Vec<(u64, String)>,
    pub unreadable: usize,
    pub stopped_at_limit: bool,
}

fn rel_join(rel: &str, name: &str) -> String {
    if rel.is_empty() { name.to_string() } else { format!("{}/{}", rel, name) }
}

/// Walks `root` (symbolic links are not followed) and measures it.
pub(crate) fn measure(root: &Path, top: usize, depth: usize) -> Health {
    let mut dirs: Vec<DirNode> = vec![DirNode { rel: String::new(), parent: None, depth: 0, size: 0, files: 0 }];
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    let mut heap: BinaryHeap<Reverse<(u64, String)>> = BinaryHeap::new();
    let (mut files, mut unreadable, mut seen) = (0usize, 0usize, 0usize);
    let mut stopped = false;

    while let Some((path, idx)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&path) else {
            unreadable += 1;
            continue;
        };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                stopped = true;
                break;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_dir() {
                let rel = rel_join(&dirs[idx].rel, &name);
                let d = dirs[idx].depth + 1;
                dirs.push(DirNode { rel, parent: Some(idx), depth: d, size: 0, files: 0 });
                stack.push((e.path(), dirs.len() - 1));
            } else if let Ok(m) = e.metadata() {
                let len = m.len();
                files += 1;
                dirs[idx].size += len;
                dirs[idx].files += 1;
                if top > 0 {
                    let rel = rel_join(&dirs[idx].rel, &name);
                    if heap.len() < top {
                        heap.push(Reverse((len, rel)));
                    } else if heap.peek().map_or(false, |Reverse((s, _))| len > *s) {
                        heap.pop();
                        heap.push(Reverse((len, rel)));
                    }
                }
            }
        }
        if stopped {
            break;
        }
    }

    // Children were created after their parents, so one reverse pass adds every folder into its parent.
    let mut total = vec![0u64; dirs.len()];
    for i in 0..dirs.len() {
        total[i] = dirs[i].size;
    }
    for i in (1..dirs.len()).rev() {
        if let Some(p) = dirs[i].parent {
            total[p] += total[i];
        }
    }
    let mut folders: Vec<(u64, String)> = (1..dirs.len()).filter(|&i| dirs[i].depth <= depth.max(1)).map(|i| (total[i], dirs[i].rel.clone())).collect();
    folders.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    folders.truncate(top);
    let mut top_files: Vec<(u64, String)> = heap.into_iter().map(|Reverse(x)| x).collect();
    top_files.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    Health { total_bytes: total[0], files, folders: dirs.len() - 1, top_folders: folders, top_files, unreadable, stopped_at_limit: stopped }
}

/// What a folder is, from its name, so the report says which space is rebuildable and which is not. A folder inside a
/// `src` tree is source code whatever its name (this project has a `build/` of its own).
fn kind_tag(rel: &str, root_in_src: bool) -> &'static str {
    let parts: Vec<String> = rel.split('/').map(|s| s.to_lowercase()).collect();
    let (last, ancestors) = match parts.split_last() {
        Some(x) => x,
        None => return "",
    };
    if root_in_src || ancestors.iter().any(|a| a == "src" || a == "source" || a == "sources") {
        return "";
    }
    match last.as_str() {
        ".git" => "   [git history: keep]",
        ".mysid" => "   [mysid cache]",
        ".idea" | ".vs" => "   [IDE settings]",
        "node_modules" | "pods" => "   [dependencies]",
        other if GENERATED.contains(&other) => "   [build output]",
        _ => "",
    }
}

fn num(params: &Value, key: &str, default: usize) -> usize {
    params.get(key).and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).map_or(default, |n| n as usize)
}

pub fn run(params: &Value, default_root: Option<&str>) -> String {
    let fail = |m: String| json!({ "success": false, "error": m }).to_string();
    let base = params.get("workspace_root").and_then(|v| v.as_str()).or(default_root).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let target = match params.get("path").or_else(|| params.get("a")).and_then(|v| v.as_str()).filter(|s| !s.trim().is_empty()) {
        Some(p) => {
            let pb = PathBuf::from(p.trim());
            if pb.is_absolute() { pb } else { base.join(pb) }
        }
        None => base,
    };
    if !target.is_dir() {
        return fail(format!("not a folder: {}", target.display()));
    }
    let top = num(params, "top", 5).clamp(1, 50);
    let depth = num(params, "depth", 1).clamp(1, 6);
    let root_in_src = target.components().any(|c| matches!(c.as_os_str().to_string_lossy().to_lowercase().as_str(), "src" | "source" | "sources"));
    let h = measure(&target, top, depth);
    let pct = |s: u64| if h.total_bytes == 0 { 0 } else { (s as f64 * 100.0 / h.total_bytes as f64).round() as u64 };

    let mut out: Vec<String> = Vec::new();
    out.push(format!("path: {}", target.display().to_string().replace('\\', "/")));
    out.push(format!("total: {} in {} files and {} folders", human(h.total_bytes), thousands(h.files), thousands(h.folders)));
    out.push(String::new());
    out.push(format!("top {} largest folders{}:", h.top_folders.len(), if depth > 1 { format!(" (up to {} levels deep)", depth) } else { String::new() }));
    if h.top_folders.is_empty() {
        out.push("  (no sub-folders)".to_string());
    }
    for (i, (size, rel)) in h.top_folders.iter().enumerate() {
        out.push(format!("  {}. {:>10}  {:>3}%  {}/{}", i + 1, human(*size), pct(*size), rel, kind_tag(rel, root_in_src)));
    }
    out.push(String::new());
    out.push(format!("top {} largest files:", h.top_files.len()));
    if h.top_files.is_empty() {
        out.push("  (no files)".to_string());
    }
    for (i, (size, rel)) in h.top_files.iter().enumerate() {
        out.push(format!("  {}. {:>10}  {:>3}%  {}", i + 1, human(*size), pct(*size), rel));
    }
    if h.unreadable > 0 {
        out.push(String::new());
        out.push(format!("{} folder(s) could not be read (permissions) and are not counted.", h.unreadable));
    }
    if h.stopped_at_limit {
        out.push(String::new());
        out.push(format!("stopped after {} entries: the numbers cover only part of this folder. Point it at a sub-folder.", thousands(MAX_ENTRIES)));
    }
    json!({ "success": true, "output": out.join("\n") }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(root: &Path, rel: &str, bytes: usize) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, vec![b'x'; bytes]).unwrap();
    }

    fn fixture(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mysid_health_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_largest_folders_and_files_come_first_with_nested_sizes_added_up() {
        let root = fixture("sizes");
        put(&root, "build/out/a.bin", 5000);
        put(&root, "build/b.bin", 3000);
        put(&root, "src/main.rs", 1000);
        put(&root, "src/deep/big.rs", 2000);
        put(&root, "notes.txt", 100);
        let h = measure(&root, 5, 1);
        assert_eq!((h.total_bytes, h.files, h.folders), (11_100, 5, 4));
        assert_eq!(h.top_folders, vec![(8000, "build".to_string()), (3000, "src".to_string())]);
        assert_eq!(h.top_files.iter().map(|f| f.0).collect::<Vec<_>>(), vec![5000, 3000, 2000, 1000, 100]);
        assert_eq!(h.top_files[0].1, "build/out/a.bin");
        // Deeper: nested folders compete too.
        let deep = measure(&root, 3, 2);
        assert_eq!(deep.top_folders[0], (8000, "build".to_string()));
        assert!(deep.top_folders.iter().any(|f| f.1 == "build/out" && f.0 == 5000));
        // The top limit applies to both lists.
        let one = measure(&root, 1, 1);
        assert_eq!((one.top_folders.len(), one.top_files.len()), (1, 1));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_report_names_the_path_marks_generated_folders_and_rejects_a_bad_path() {
        let root = fixture("report");
        put(&root, "node_modules/x/index.js", 4000);
        put(&root, "src/a.kt", 100);
        let p = json!({ "path": root.to_string_lossy() });
        let r: Value = serde_json::from_str(&run(&p, None)).unwrap();
        let o = r["output"].as_str().unwrap();
        assert!(o.starts_with("path: "), "{}", o);
        assert!(o.contains("top 2 largest folders:") && o.contains("node_modules/   [dependencies]"), "{}", o);
        // Source folders that happen to be called build, and git history, are labelled for what they are.
        assert_eq!(kind_tag("build", false), "   [build output]");
        assert_eq!(kind_tag("rust-mcp/src/tools/build", false), "");
        assert_eq!(kind_tag("build", true), "", "a root inside a src tree: its build/ is source");
        assert_eq!(kind_tag(".git", false), "   [git history: keep]");
        assert!(o.contains("top 2 largest files:") && o.contains("node_modules/x/index.js"), "{}", o);
        assert!(o.contains("98%") || o.contains(" 97%"), "{}", o);
        let bad: Value = serde_json::from_str(&run(&json!({ "path": root.join("nope").to_string_lossy() }), None)).unwrap();
        assert!(bad["error"].as_str().unwrap().contains("not a folder"));
        let _ = fs::remove_dir_all(&root);
    }
}
