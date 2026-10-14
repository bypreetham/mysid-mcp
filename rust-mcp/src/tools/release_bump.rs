//! `versionCode` bump for `mysid release`.
//!
//! Every release build needs a higher versionCode than the last upload, so `mysid release` raises
//! it by one before building. Only literal numbers are touched (`versionCode = 37` in Kotlin DSL,
//! `versionCode 37` in Groovy, `android:versionCode="37"` in a manifest); a computed value is left
//! alone. Each file's original number is recorded so a failed or cancelled build can restore it.

use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};

pub struct Bump {
    pub from: u64,
    pub to: u64,
    /// Each edited file with the number it held before the bump.
    pub files: Vec<(PathBuf, u64)>,
}

fn gradle_re(number: Option<u64>) -> Regex {
    let digits = number.map_or(r"\d+".to_string(), |n| n.to_string());
    Regex::new(&format!(r"(\bversionCode\s*=?\s*)({})\b", digits)).unwrap()
}

fn manifest_re(number: Option<u64>) -> Regex {
    let digits = number.map_or(r"\d+".to_string(), |n| n.to_string());
    Regex::new(&format!(r#"(android:versionCode\s*=\s*")({})(")"#, digits)).unwrap()
}

fn is_manifest(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("xml")
}

fn regex_for(path: &Path, number: Option<u64>) -> Regex {
    if is_manifest(path) { manifest_re(number) } else { gradle_re(number) }
}

/// The first literal versionCode in a file, if it has one.
fn current_value(path: &Path) -> Option<u64> {
    let text = fs::read_to_string(path).ok()?;
    regex_for(path, None).captures(&text)?.get(2)?.as_str().parse().ok()
}

fn candidate_files(root: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root) {
        let mut subs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        subs.sort();
        dirs.extend(subs);
    }
    let mut files = Vec::new();
    for d in dirs {
        for rel in ["build.gradle.kts", "build.gradle", "src/main/AndroidManifest.xml", "AndroidManifest.xml"] {
            let p = d.join(rel);
            if p.is_file() {
                files.push(p);
            }
        }
    }
    files
}

/// Raises every literal versionCode under `root` (and its direct subdirectories) to
/// `highest current value + 1`. Ok(None) when there is no literal to bump.
pub fn bump_version_code(root: &Path) -> Result<Option<Bump>, String> {
    let found: Vec<(PathBuf, u64)> = candidate_files(root)
        .into_iter()
        .filter_map(|p| current_value(&p).map(|v| (p, v)))
        .collect();
    let Some(from) = found.iter().map(|(_, v)| *v).max() else {
        return Ok(None);
    };
    let to = from + 1;

    let mut done: Vec<(PathBuf, u64)> = Vec::new();
    for (path, original) in &found {
        if let Err(e) = rewrite(path, *original, to) {
            restore(&done);
            return Err(format!("{}: {}", path.display(), e));
        }
        done.push((path.clone(), *original));
    }
    Ok(Some(Bump { from, to, files: done }))
}

/// Puts each file back to the number it held before `bump_version_code`.
pub fn restore(files: &[(PathBuf, u64)]) {
    for (path, original) in files {
        if let Some(current) = current_value(path) {
            let _ = rewrite(path, current, *original);
        }
    }
}

/// Replaces the first versionCode equal to `old` with `new`, leaving everything else byte-identical.
fn rewrite(path: &Path, old: u64, new: u64) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    let re = regex_for(path, Some(old));
    let replaced = re.replacen(&text, 1, |c: &regex::Captures| {
        format!("{}{}{}", &c[1], new, c.get(3).map_or("", |m| m.as_str()))
    });
    if replaced == text {
        return Err(format!("versionCode {} not found", old));
    }
    fs::write(path, replaced.as_bytes()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mysid-bump-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("app")).unwrap();
        dir
    }

    #[test]
    fn bumps_kotlin_dsl_keeping_everything_else_and_restores() {
        let dir = project("kts");
        let file = dir.join("app").join("build.gradle.kts");
        let original = "android {\r\n    defaultConfig {\r\n        versionCode = 37\r\n        versionName = \"1.0\"\r\n    }\r\n}\r\n";
        fs::write(&file, original).unwrap();

        let bump = bump_version_code(&dir).unwrap().unwrap();
        assert_eq!((bump.from, bump.to), (37, 38));
        let after = fs::read_to_string(&file).unwrap();
        assert_eq!(after, original.replace("versionCode = 37", "versionCode = 38"));

        restore(&bump.files);
        assert_eq!(fs::read_to_string(&file).unwrap(), original, "restore is byte-exact");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bumps_groovy_and_manifest_to_one_consistent_number() {
        let dir = project("groovy");
        fs::write(dir.join("app").join("build.gradle"), "defaultConfig {\n    versionCode 40\n}\n").unwrap();
        fs::create_dir_all(dir.join("app").join("src").join("main")).unwrap();
        let manifest = dir.join("app").join("src").join("main").join("AndroidManifest.xml");
        fs::write(&manifest, "<manifest android:versionCode=\"39\" android:versionName=\"x\">\n</manifest>\n").unwrap();

        let bump = bump_version_code(&dir).unwrap().unwrap();
        assert_eq!((bump.from, bump.to), (40, 41));
        assert!(fs::read_to_string(dir.join("app").join("build.gradle")).unwrap().contains("versionCode 41"));
        assert!(fs::read_to_string(&manifest).unwrap().contains("android:versionCode=\"41\""));

        restore(&bump.files);
        assert!(fs::read_to_string(&manifest).unwrap().contains("android:versionCode=\"39\""), "each file gets its own old value back");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn computed_version_code_is_left_alone() {
        let dir = project("computed");
        fs::write(dir.join("app").join("build.gradle.kts"), "versionCode = baseCode + 1\nversionName = \"1\"\n").unwrap();
        assert!(bump_version_code(&dir).unwrap().is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
