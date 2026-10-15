//! Bare file names in, short names out.
//!
//! Commands accept `Foo.kt` instead of `app/src/main/java/.../Foo.kt` (`find_by_name`), and the text
//! they print uses the shortest name that still identifies a file (`Shortener`), so a model that copies
//! what it sees types short names too.

use std::collections::HashMap;
use std::path::Path;

use crate::index::scan::list_files;

/// Files whose name (or trailing path) equals `name`: `Foo.kt` matches any `.../Foo.kt`,
/// `ui/Foo.kt` only `.../ui/Foo.kt`. Falls back to a case-insensitive comparison when nothing matches.
pub fn find_by_name(files: &[String], name: &str) -> Vec<String> {
    let name = name.replace('\\', "/");
    let name = name.trim_start_matches("./").trim_matches('/');
    if name.is_empty() {
        return Vec::new();
    }
    let hit = |f: &str, n: &str| f == n || (f.len() > n.len() && f.ends_with(n) && f.as_bytes()[f.len() - n.len() - 1] == b'/');
    let exact: Vec<String> = files.iter().filter(|f| hit(f, name)).cloned().collect();
    if !exact.is_empty() {
        return exact;
    }
    let lower = name.to_lowercase();
    files.iter().filter(|f| hit(&f.to_lowercase(), &lower)).cloned().collect()
}

/// Maps a workspace-relative path to the shortest trailing part that is unique in the workspace.
pub struct Shortener {
    short: HashMap<String, String>,
}

impl Shortener {
    pub fn new(root: &Path) -> Self {
        Self::from_files(&list_files(root))
    }

    pub fn from_files(files: &[String]) -> Self {
        let mut by_name: HashMap<&str, Vec<&String>> = HashMap::new();
        for f in files {
            by_name.entry(f.rsplit('/').next().unwrap_or(f)).or_default().push(f);
        }
        let mut short = HashMap::new();
        for (_, group) in by_name {
            for f in &group {
                let parts: Vec<&str> = f.split('/').collect();
                // Fewest trailing components that no other file with the same name ends with.
                let mut take = 1;
                while take < parts.len() {
                    let tail = parts[parts.len() - take..].join("/");
                    let clashes = group.iter().filter(|g| g.as_str() == tail || g.ends_with(&format!("/{}", tail))).count();
                    if clashes <= 1 {
                        break;
                    }
                    take += 1;
                }
                let tail = parts[parts.len() - take..].join("/");
                if tail != **f {
                    short.insert((*f).clone(), tail);
                }
            }
        }
        Shortener { short }
    }

    /// Replaces every known workspace-relative path in `text` with its short form.
    pub fn apply(&self, text: &str) -> String {
        if self.short.is_empty() || !text.contains('/') {
            return text.to_string();
        }
        let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | '$' | '+' | '@');
        let mut out = String::with_capacity(text.len());
        let mut token = String::new();
        let flush = |token: &mut String, out: &mut String| {
            if token.is_empty() {
                return;
            }
            let trimmed = token.trim_end_matches(|c| c == '.');
            let tail = &token[trimmed.len()..];
            match self.short.get(trimmed) {
                Some(s) => {
                    out.push_str(s);
                    out.push_str(tail);
                }
                None => out.push_str(token),
            }
            token.clear();
        };
        // The text may be JSON, where a newline is a backslash plus a letter, not the start of the next path.
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                flush(&mut token, &mut out);
                out.push(c);
                if let Some(esc) = chars.next() {
                    out.push(esc);
                }
            } else if is_path_char(c) {
                token.push(c);
            } else {
                flush(&mut token, &mut out);
                out.push(c);
            }
        }
        flush(&mut token, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files() -> Vec<String> {
        ["app/src/ui/Foo.kt", "app/src/data/Foo.kt", "app/src/Bar.kt", "app/src/main/cpp/bridge/UsbBridge.cpp", "docs/abc.txt"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    #[test]
    fn bare_and_partial_names_find_files() {
        let f = files();
        assert_eq!(find_by_name(&f, "Bar.kt"), vec!["app/src/Bar.kt"]);
        assert_eq!(find_by_name(&f, "Foo.kt").len(), 2);
        assert_eq!(find_by_name(&f, "ui/Foo.kt"), vec!["app/src/ui/Foo.kt"]);
        assert_eq!(find_by_name(&f, "usbbridge.cpp"), vec!["app/src/main/cpp/bridge/UsbBridge.cpp"]);
        assert_eq!(find_by_name(&f, "app/src/Bar.kt"), vec!["app/src/Bar.kt"]);
        assert!(find_by_name(&f, "ar.kt").is_empty());
    }

    #[test]
    fn output_uses_the_shortest_unique_name() {
        let s = Shortener::from_files(&files());
        let out = s.apply("app/src/Bar.kt:12 calls app/src/ui/Foo.kt:3, app/src/data/Foo.kt, docs/abc.txt. See app/src/Bar.kt.");
        assert_eq!(out, "Bar.kt:12 calls ui/Foo.kt:3, data/Foo.kt, abc.txt. See Bar.kt.");
        // inside JSON strings a newline is the two characters backslash + n
        assert_eq!(
            s.apply(r#"{"output": "x\napp/src/Bar.kt:1\ndocs/abc.txt"}"#),
            r#"{"output": "x\nBar.kt:1\nabc.txt"}"#
        );
        // every short form must resolve back to exactly its file
        let f = files();
        for full in &f {
            let short = s.apply(full);
            assert_eq!(find_by_name(&f, &short), vec![full.clone()], "{} -> {}", full, short);
        }
    }
}
