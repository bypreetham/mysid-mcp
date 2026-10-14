//! Environment problems that make a Gradle / CMake build fail minutes in: caches written at another path (the project
//! folder was moved or copied), and an SDK folder in `local.properties` that no longer exists. Checked before a build
//! starts, so the failure costs a second instead of a 7-minute attempt.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

const SKIP: &[&str] = &["node_modules", ".git", ".mysid", ".gradle", "target", "external", "externals", "third_party", "vendor", ".idea", "intermediates", "outputs"];
const MAX_DEPTH: usize = 7;
const MAX_ENTRIES: usize = 30_000;

pub struct Stale {
    /// The folder to delete, relative to the project root, with `/` separators.
    pub dir: PathBuf,
    pub rel: String,
    /// Where the cache says it was created.
    pub was: String,
}

fn norm(p: &str) -> String {
    p.trim_start_matches(r"\\?\").replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Every `CMakeCache.txt` under the root (Gradle keeps them in `app/.cxx/...`) whose recorded folder is not under the root.
pub fn stale_caches(root: &Path) -> Vec<Stale> {
    let here = norm(&root.canonicalize().unwrap_or_else(|_| root.to_path_buf()).to_string_lossy());
    let mut found: Vec<Stale> = Vec::new();
    let mut seen = 0usize;
    let mut stack: Vec<(PathBuf, usize)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return found;
            }
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < MAX_DEPTH && !SKIP.contains(&name.as_str()) {
                    stack.push((path, depth + 1));
                }
            } else if name == "CMakeCache.txt" {
                if let Some(was) = cache_dir_of(&path) {
                    if !norm(&was).starts_with(&here) {
                        // Gradle: delete the whole `.cxx`; CMake: the build folder holding the cache.
                        let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                        let comps: Vec<String> = rel_path.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
                        let cut = comps.iter().position(|c| c == ".cxx").map_or(comps.len().saturating_sub(1), |i| i + 1);
                        let dir_rel: PathBuf = comps[..cut].iter().collect();
                        if !found.iter().any(|f| f.dir == root.join(&dir_rel)) {
                            found.push(Stale { dir: root.join(&dir_rel), rel: comps[..cut].join("/"), was });
                        }
                    }
                }
            }
        }
    }
    found.sort_by(|a, b| a.rel.cmp(&b.rel));
    found
}

fn cache_dir_of(cache: &Path) -> Option<String> {
    let text = fs::read_to_string(cache).ok()?;
    text.lines().find_map(|l| l.strip_prefix("CMAKE_CACHEFILE_DIR:INTERNAL=").map(|v| v.trim().to_string()))
}

/// `sdk.dir` / `ndk.dir` of `local.properties` pointing at a folder that does not exist.
pub fn missing_sdk_dirs(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("local.properties")) else { return Vec::new() };
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        for key in ["sdk.dir", "ndk.dir"] {
            if let Some(v) = line.strip_prefix(&format!("{}=", key)) {
                let path = v.replace("\\:", ":").replace("\\\\", "\\");
                if !path.is_empty() && !Path::new(&path).exists() {
                    out.push(format!("local.properties {} points at {}, which does not exist (Android Studio rewrites it on open, or edit it)", key, path));
                }
            }
        }
    }
    out
}

fn truthy(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| match params.get(*k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"),
        _ => false,
    })
}

/// None when the build may start. Some(answer) when it must not: stale caches (unless `--fix-stale` deletes them or
/// `--ignore-stale` goes on anyway) or a missing SDK folder.
pub fn guard(root: &Path, params: &Value) -> Option<String> {
    let stale = stale_caches(root);
    let sdk = missing_sdk_dirs(root);
    if stale.is_empty() && sdk.is_empty() {
        return None;
    }
    if truthy(params, &["ignore_stale", "ignore-stale"]) {
        return None;
    }
    if !stale.is_empty() && sdk.is_empty() && truthy(params, &["fix_stale", "fix-stale"]) {
        let mut done = Vec::new();
        for s in &stale {
            match fs::remove_dir_all(&s.dir) {
                Ok(()) => done.push(format!("deleted {} (made at {})", s.rel, s.was)),
                Err(e) => {
                    return Some(json!({ "success": false, "error": format!("NOT STARTED: cannot delete {}: {}. Close Android Studio / stop the Gradle daemon (gradlew --stop) and retry.", s.rel, e) }).to_string());
                }
            }
        }
        eprintln!("mysid: stale build caches removed: {}", done.join("; "));
        return None;
    }
    let mut lines = vec![format!("NOT STARTED: the project is at {} but the build state says otherwise; the build would fail minutes in.", root.display())];
    for s in &stale {
        lines.push(format!("  stale: {} was created at {}", s.rel, s.was));
    }
    for m in &sdk {
        lines.push(format!("  {}", m));
    }
    if !stale.is_empty() {
        lines.push(format!("Fix: mysid build <same args> --fix-stale   (deletes {}; they are rebuilt)   or --ignore-stale to try anyway.", stale.iter().map(|s| s.rel.as_str()).collect::<Vec<_>>().join(", ")));
    }
    Some(json!({ "success": false, "error": lines.join("\n") }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mysid_stale_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn caches_made_at_another_path_are_stale_and_can_be_fixed() {
        let root = fixture("moved");
        let cxx = root.join("app").join(".cxx").join("Debug").join("abc").join("arm64-v8a");
        fs::create_dir_all(&cxx).unwrap();
        fs::write(cxx.join("CMakeCache.txt"), "CMAKE_CACHEFILE_DIR:INTERNAL=D:/old place/proj/app/.cxx/Debug/abc/arm64-v8a\n").unwrap();
        // A cache made here is fine.
        let ok = root.join("lib").join(".cxx").join("Debug");
        fs::create_dir_all(&ok).unwrap();
        fs::write(ok.join("CMakeCache.txt"), format!("CMAKE_CACHEFILE_DIR:INTERNAL={}\n", ok.to_string_lossy().replace('\\', "/"))).unwrap();

        let stale = stale_caches(&root);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].rel, "app/.cxx");
        assert!(stale[0].was.starts_with("D:/old place"));

        let blocked = guard(&root, &json!({})).unwrap();
        assert!(blocked.contains("NOT STARTED") && blocked.contains("app/.cxx") && blocked.contains("--fix-stale"), "{}", blocked);
        assert!(guard(&root, &json!({ "ignore_stale": true })).is_none());
        assert!(guard(&root, &json!({ "fix_stale": true })).is_none());
        assert!(!root.join("app").join(".cxx").exists() && root.join("lib").join(".cxx").exists(), "only the stale one is removed");
        assert!(guard(&root, &json!({})).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_sdk_folder_blocks_the_build() {
        let root = fixture("sdk");
        fs::write(root.join("local.properties"), "sdk.dir=C\\:\\\\Nope\\\\Sdk\n").unwrap();
        let blocked = guard(&root, &json!({ "fix_stale": true })).unwrap();
        assert!(blocked.contains("sdk.dir") && blocked.contains("does not exist"), "{}", blocked);
        let _ = fs::remove_dir_all(&root);
    }
}
