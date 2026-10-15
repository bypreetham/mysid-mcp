//! The steps of `mysid release` that mysid itself performs: a preflight before anything is
//! changed, and verification of the bundle after Gradle finishes. Each returns short lines the
//! worker reports as stages, so a release never looks like a silent script waiting for an end state.

use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::tools::build::gradle;
use crate::tools::build::exec::run_with_timeout;

const JARSIGNER_TIMEOUT: Duration = Duration::from_secs(60);

fn env_value(name: &str, env: &[(String, String)]) -> Option<String> {
    env.iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.clone())
        .or_else(|| std::env::var(name).ok())
        .filter(|v| !v.trim().is_empty())
}

fn java_home(env: &[(String, String)]) -> Option<PathBuf> {
    env_value("JAVA_HOME", env).map(PathBuf::from).filter(|p| p.exists())
}

/// Environment variables the project's Gradle files read (`System.getenv("NAME")`), sorted.
/// Release signing is usually configured this way, so they tell what a release needs.
fn env_names_read_by_gradle(root: &Path) -> Vec<String> {
    let re = Regex::new(r#"getenv\(\s*"([A-Za-z0-9_]+)"\s*\)"#).unwrap();
    let mut dirs = vec![root.to_path_buf()];
    if let Ok(entries) = fs::read_dir(root) {
        dirs.extend(entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.is_dir()));
    }
    let mut names = BTreeSet::new();
    for dir in dirs {
        for file in ["build.gradle.kts", "build.gradle"] {
            if let Ok(text) = fs::read_to_string(dir.join(file)) {
                names.extend(re.captures_iter(&text).map(|c| c[1].to_string()));
            }
        }
    }
    names.into_iter().collect()
}

/// What can be checked before touching anything: the JDK and the signing environment. These are
/// reported, not enforced: a project may fall back to another keystore when the variables are unset.
pub fn preflight(root: &Path, env: &[(String, String)]) -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(match java_home(env) {
        Some(home) => format!("JDK: JAVA_HOME ok ({})", home.display()),
        None => "JDK: JAVA_HOME not set, using java from PATH".to_string(),
    });

    let names = env_names_read_by_gradle(root);
    if !names.is_empty() {
        let missing: Vec<&String> = names.iter().filter(|n| env_value(n, env).is_none()).collect();
        let mut line = format!("signing env: {}/{} set", names.len() - missing.len(), names.len());
        if !missing.is_empty() {
            let list: Vec<&str> = missing.iter().map(|s| s.as_str()).collect();
            line.push_str(&format!(" (not set: {}; fine if build.gradle falls back to another keystore)", list.join(", ")));
        }
        lines.push(line);

        // A keystore path that is set but points nowhere fails late, after the whole compile.
        for name in &names {
            let upper = name.to_ascii_uppercase();
            if upper.contains("STORE_FILE") || upper.contains("KEYSTORE") {
                if let Some(path) = env_value(name, env) {
                    if !Path::new(&path).exists() {
                        lines.push(format!("keystore file NOT FOUND: {} = {}", name, path));
                    }
                }
            }
        }
    }
    lines
}

pub struct Verification {
    /// One line per check, reported as stage steps.
    pub notes: Vec<String>,
    /// Short verdict for the final report.
    pub signature: String,
}

#[derive(Debug, PartialEq, Eq)]
enum Signed {
    Verified,
    Unsigned,
    Unknown(String),
}

/// Reads `jarsigner -verify` output.
fn parse_jarsigner(output: &str) -> Signed {
    let lower = output.to_ascii_lowercase();
    if lower.contains("jar verified") {
        Signed::Verified
    } else if lower.contains("jar is unsigned") || lower.contains("no manifest") {
        Signed::Unsigned
    } else {
        Signed::Unknown(output.lines().find(|l| !l.trim().is_empty()).unwrap_or("no output").trim().to_string())
    }
}

/// An AAB (like an APK) is a zip archive: it starts with `PK\x03\x04`.
fn is_zip(path: &Path) -> bool {
    let mut magic = [0u8; 4];
    fs::File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && magic == *b"PK\x03\x04"
}

fn jarsigner_path(env: &[(String, String)]) -> String {
    let exe = if cfg!(windows) { "jarsigner.exe" } else { "jarsigner" };
    java_home(env)
        .map(|h| h.join("bin").join(exe))
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "jarsigner".to_string())
}

/// After a successful `bundleRelease`: is there a fresh, well-formed, signed bundle?
pub fn verify_release(root: &Path, env: &[(String, String)], since_ms: u64) -> Verification {
    let Some((path, mtime, len)) = gradle::newest_artifact_path(root, since_ms, "bundle", "aab") else {
        return Verification {
            notes: vec!["no bundle found under */build/outputs/bundle/".to_string()],
            signature: "not checked (no bundle)".to_string(),
        };
    };
    let mut notes = Vec::new();
    let rel = path.strip_prefix(root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| path.to_string_lossy().to_string());
    let fresh = if mtime + 5_000 < since_ms { " - NOT rebuilt by this run" } else { "" };

    if !is_zip(&path) {
        notes.push(format!("{} is not a valid zip archive{}", rel, fresh));
        return Verification { notes, signature: "not checked (bundle is not a valid zip)".to_string() };
    }
    notes.push(format!("bundle {} ({:.1} MB, valid zip){}", rel, len as f64 / 1_048_576.0, fresh));

    let mut cmd = Command::new(jarsigner_path(env));
    cmd.arg("-verify").arg(&path);
    let verdict = match run_with_timeout(cmd, JARSIGNER_TIMEOUT) {
        Ok(run) => match parse_jarsigner(&format!("{}\n{}", run.stdout, run.stderr)) {
            Signed::Verified => "verified".to_string(),
            Signed::Unsigned => "UNSIGNED - Google Play will reject this bundle".to_string(),
            Signed::Unknown(first) => format!("could not be checked (jarsigner: {})", first),
        },
        Err(_) => "not checked (jarsigner not found; install a JDK or set JAVA_HOME)".to_string(),
    };
    notes.push(format!("signature: {}", verdict));
    Verification { notes, signature: verdict }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("mysid-rc-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("app")).unwrap();
        d
    }

    #[test]
    fn finds_the_env_vars_gradle_reads_and_reports_which_are_missing() {
        let d = dir("env");
        fs::write(
            d.join("app").join("build.gradle.kts"),
            "val a = System.getenv(\"MYSID_TEST_ALIAS\")\nval b = System.getenv( \"MYSID_TEST_STORE_FILE\" )\nval c = System.getenv(\"MYSID_TEST_ALIAS\")\n",
        )
        .unwrap();
        assert_eq!(env_names_read_by_gradle(&d), vec!["MYSID_TEST_ALIAS", "MYSID_TEST_STORE_FILE"]);

        let lines = preflight(&d, &[("MYSID_TEST_ALIAS".to_string(), "key".to_string())]);
        assert!(lines.iter().any(|l| l.starts_with("signing env: 1/2 set") && l.contains("MYSID_TEST_STORE_FILE")), "{:?}", lines);

        let lines = preflight(
            &d,
            &[
                ("MYSID_TEST_ALIAS".to_string(), "key".to_string()),
                ("MYSID_TEST_STORE_FILE".to_string(), d.join("missing.jks").to_string_lossy().to_string()),
            ],
        );
        assert!(lines.iter().any(|l| l.starts_with("signing env: 2/2 set")), "{:?}", lines);
        assert!(lines.iter().any(|l| l.contains("keystore file NOT FOUND")), "{:?}", lines);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_project_that_reads_no_env_vars_gets_only_the_jdk_line() {
        let d = dir("noenv");
        fs::write(d.join("app").join("build.gradle.kts"), "android {}\n").unwrap();
        let lines = preflight(&d, &[]);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("JDK:"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn jarsigner_output_is_classified() {
        assert_eq!(parse_jarsigner("\njar verified.\n\nWarning:\nThis jar contains..."), Signed::Verified);
        assert_eq!(parse_jarsigner("jar is unsigned."), Signed::Unsigned);
        assert!(matches!(parse_jarsigner("jarsigner: java.util.zip.ZipException: bad"), Signed::Unknown(m) if m.contains("ZipException")));
    }

    #[test]
    fn zip_archives_are_recognized_by_their_magic_bytes() {
        let d = dir("zip");
        let good = d.join("good.aab");
        fs::write(&good, b"PK\x03\x04rest-of-archive").unwrap();
        let bad = d.join("bad.aab");
        fs::write(&bad, b"not a zip").unwrap();
        assert!(is_zip(&good) && !is_zip(&bad) && !is_zip(&d.join("absent.aab")));
        let _ = fs::remove_dir_all(&d);
    }
}
