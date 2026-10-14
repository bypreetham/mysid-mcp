//! `mysid deps`: which npm packages the code imports that package.json does not list (and which listed ones nothing
//! imports). Replaces grepping for imports and guessing what to `npm install`.

use ignore::WalkBuilder;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use super::workspace::resolve_existing_root;

pub const HELP: &str = "mysid deps: packages the code imports that package.json is missing\n\n\
Usage:\n  \
  mysid deps                 imports vs package.json: missing packages (with the files that need them), the install\n                             command, and listed dependencies that nothing imports\n\n\
Reads import / export-from / require / dynamic import in ts, tsx, js, jsx, mjs, cjs, vue, svelte files (node_modules, build\n\
output and .gitignore'd files skipped). Relative imports, Node built-ins and tsconfig path aliases (@/, ~/) are not packages.\n\
A package listed but never imported may still be used by a config file, a script or a CLI: check before removing it.\n";

const SOURCE_EXTS: &[&str] = &["ts", "tsx", "js", "jsx", "mjs", "cjs", "mts", "cts", "vue", "svelte"];

const NODE_BUILTINS: &[&str] = &[
    "assert", "async_hooks", "buffer", "child_process", "cluster", "console", "constants", "crypto", "dgram", "diagnostics_channel", "dns", "domain", "events",
    "fs", "http", "http2", "https", "inspector", "module", "net", "os", "path", "perf_hooks", "process", "punycode", "querystring", "readline", "repl",
    "stream", "string_decoder", "sys", "timers", "tls", "trace_events", "tty", "url", "util", "v8", "vm", "wasi", "worker_threads", "zlib",
];

/// `@scope/name/deep/path` -> `@scope/name`; `lodash/fp` -> `lodash`. None when the specifier is not a package.
fn package_of(spec: &str, aliases: &[String]) -> Option<String> {
    let s = spec.trim();
    if s.is_empty() || s.starts_with('.') || s.starts_with('/') || s.starts_with('#') || s.contains("://") || s.starts_with("data:") {
        return None;
    }
    if s.starts_with("node:") || s.starts_with("virtual:") || s.starts_with("astro:") || s.starts_with("bun:") || s.starts_with("npm:") {
        return None;
    }
    if aliases.iter().any(|a| s == a || s.starts_with(&format!("{}/", a))) {
        return None;
    }
    let mut parts = s.split('/');
    let first = parts.next()?;
    let name = if first.starts_with('@') {
        // `@/components/x` and `@components/x` are usually aliases; a real scope has a second segment.
        let second = parts.next()?;
        if first == "@" || second.is_empty() {
            return None;
        }
        format!("{}/{}", first, second)
    } else {
        first.to_string()
    };
    if NODE_BUILTINS.contains(&name.as_str()) || name.chars().any(|c| c.is_whitespace() || c == '{' || c == '$' || c == '*') {
        return None;
    }
    Some(name)
}

/// Path alias prefixes from tsconfig*.json / jsconfig.json (`"@/*"` -> `@`, `"~/*"` -> `~`, `"@app/*"` -> `@app`).
fn path_aliases(root: &Path) -> Vec<String> {
    let comment = Regex::new(r"(?m)^\s*//.*$|/\*[\s\S]*?\*/").unwrap();
    let key = Regex::new(r#""([^"*]+?)/?\*?"\s*:\s*\["#).unwrap();
    let mut out = BTreeSet::new();
    for name in ["tsconfig.json", "tsconfig.app.json", "tsconfig.base.json", "jsconfig.json"] {
        let Ok(text) = fs::read_to_string(root.join(name)) else { continue };
        let text = comment.replace_all(&text, "");
        if let Some(i) = text.find("\"paths\"") {
            for c in key.captures_iter(&text[i..]) {
                let k = c[1].trim_end_matches('/').trim_end_matches('*').to_string();
                if !k.is_empty() {
                    out.insert(k);
                }
            }
        }
    }
    out.into_iter().collect()
}

fn imports_in(text: &str, re: &[Regex]) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim_start();
        if t.starts_with("//") || t.starts_with('*') || t.starts_with("/*") {
            continue;
        }
        for r in re {
            for c in r.captures_iter(line) {
                out.push(c[1].to_string());
            }
        }
    }
    // `import { a,\n b } from "x"` spans lines: its `from "x"` is on a line of its own.
    out
}

fn declared(manifest: &Value) -> BTreeMap<String, &'static str> {
    let mut m = BTreeMap::new();
    for (field, label) in [("dependencies", "dependencies"), ("devDependencies", "devDependencies"), ("peerDependencies", "peerDependencies"), ("optionalDependencies", "optionalDependencies")] {
        if let Some(o) = manifest.get(field).and_then(|v| v.as_object()) {
            for k in o.keys() {
                m.entry(k.clone()).or_insert(label);
            }
        }
    }
    m
}

fn install_line(root: &Path, names: &[String]) -> String {
    let (pm, verb) = if root.join("pnpm-lock.yaml").exists() {
        ("pnpm", "add")
    } else if root.join("yarn.lock").exists() {
        ("yarn", "add")
    } else {
        ("npm", "install")
    };
    format!("{} {} {}", pm, verb, names.join(" "))
}

pub fn execute_deps(params: &Value, default_root: Option<&str>) -> String {
    let ws = match resolve_existing_root(params, "workspace_root", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let root = Path::new(&ws);
    let out = |ok: bool, t: &str| json!({ "success": ok, "output": t }).to_string();
    let manifest: Value = match fs::read_to_string(root.join("package.json")).ok().and_then(|t| serde_json::from_str(&t).ok()) {
        Some(m) => m,
        None => return out(false, "NO package.json here (or it is not valid JSON): `mysid deps` checks Node projects."),
    };
    let have = declared(&manifest);
    let aliases = path_aliases(root);

    let regexes: Vec<Regex> = [
        r#"\bfrom\s+["']([^"']+)["']"#,
        r#"\bimport\s+["']([^"']+)["']"#,
        r#"\brequire\(\s*["']([^"']+)["']\s*\)"#,
        r#"\bimport\(\s*["']([^"']+)["']\s*\)"#,
    ]
    .iter()
    .map(|r| Regex::new(r).unwrap())
    .collect();

    // package -> files (names) that import it
    let mut used: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut scanned = 0usize;
    let walker = WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| {
            let n = e.file_name().to_string_lossy();
            !matches!(n.as_ref(), "node_modules" | "dist" | "build" | ".next" | ".nuxt" | ".svelte-kit" | "coverage" | ".mysid" | ".git" | ".turbo" | ".vercel" | ".netlify" | "out")
        })
        .build();
    for entry in walker.flatten() {
        let p = entry.path();
        if !p.is_file() || !p.extension().and_then(|e| e.to_str()).map_or(false, |e| SOURCE_EXTS.contains(&e.to_ascii_lowercase().as_str())) {
            continue;
        }
        if p.metadata().map_or(true, |m| m.len() > 2_000_000) {
            continue;
        }
        let Ok(text) = fs::read_to_string(p) else { continue };
        scanned += 1;
        let file = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        for spec in imports_in(&text, &regexes) {
            if let Some(pkg) = package_of(&spec, &aliases) {
                used.entry(pkg).or_default().insert(file.clone());
            }
        }
    }
    if scanned == 0 {
        return out(true, "No source files (ts, tsx, js, jsx, vue, svelte) found to scan.");
    }

    // A package may be provided by its @types/ twin (`react` with only `@types/react` is still missing, so no special case),
    // but `@types/x` listed for an imported `x` is fine either way.
    let missing: Vec<(&String, &BTreeSet<String>)> = used.iter().filter(|(p, _)| !have.contains_key(*p)).collect();
    let unused: Vec<&String> = manifest
        .get("dependencies")
        .and_then(|v| v.as_object())
        .map(|o| o.keys().filter(|k| !used.contains_key(*k) && !k.starts_with("@types/")).collect())
        .unwrap_or_default();

    let mut lines = vec![format!("scanned {} source file(s); {} package(s) imported", scanned, used.len())];
    if missing.is_empty() {
        lines.push("missing from package.json: none".to_string());
    } else {
        lines.push(format!("MISSING from package.json ({}):", missing.len()));
        for (p, files) in &missing {
            let shown: Vec<&str> = files.iter().take(3).map(|s| s.as_str()).collect();
            lines.push(format!("  {}  ({} file{}: {}{})", p, files.len(), if files.len() == 1 { "" } else { "s" }, shown.join(", "), if files.len() > 3 { ", ..." } else { "" }));
        }
        let names: Vec<String> = missing.iter().map(|(p, _)| (*p).clone()).collect();
        lines.push(format!("install: {}", install_line(root, &names)));
    }
    if !unused.is_empty() {
        lines.push(format!("listed in dependencies but never imported ({}): {}", unused.len(), unused.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        lines.push("  (a config file, script or CLI may still use them: check before removing)".to_string());
    }
    out(missing.is_empty(), &lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specifiers_map_to_packages() {
        let none: Vec<String> = vec![];
        assert_eq!(package_of("lucide-react", &none).as_deref(), Some("lucide-react"));
        assert_eq!(package_of("@radix-ui/react-slot", &none).as_deref(), Some("@radix-ui/react-slot"));
        assert_eq!(package_of("lodash/fp", &none).as_deref(), Some("lodash"));
        assert_eq!(package_of("@scope/pkg/deep/file", &none).as_deref(), Some("@scope/pkg"));
        assert_eq!(package_of("./Button", &none), None);
        assert_eq!(package_of("../x", &none), None);
        assert_eq!(package_of("fs", &none), None);
        assert_eq!(package_of("node:path", &none), None);
        assert_eq!(package_of("@/components/ui/button", &none), None);
        assert_eq!(package_of("virtual:pwa-register", &none), None);
        let aliases = vec!["~".to_string(), "@app".to_string()];
        assert_eq!(package_of("~/lib/x", &aliases), None);
        assert_eq!(package_of("@app/store", &aliases), None);
    }

    #[test]
    fn imports_are_found_in_every_form_and_comments_are_skipped() {
        let re: Vec<Regex> = [r#"\bfrom\s+["']([^"']+)["']"#, r#"\bimport\s+["']([^"']+)["']"#, r#"\brequire\(\s*["']([^"']+)["']\s*\)"#, r#"\bimport\(\s*["']([^"']+)["']\s*\)"#]
            .iter()
            .map(|r| Regex::new(r).unwrap())
            .collect();
        let src = "import React from 'react';\nimport { a } from \"lucide-react\"\nimport 'swiper/css';\nconst x = require('dotenv');\nconst y = await import('react-slick');\n// import z from 'ignored'\nexport * from \"./local\";\n";
        let got = imports_in(src, &re);
        for want in ["react", "lucide-react", "swiper/css", "dotenv", "react-slick", "./local"] {
            assert!(got.iter().any(|g| g == want), "{} in {:?}", want, got);
        }
        assert!(!got.iter().any(|g| g == "ignored"));
    }

    #[test]
    fn missing_and_unused_are_reported_for_a_project() {
        let d = std::env::temp_dir().join(format!("mysid_deps_{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(d.join("src")).unwrap();
        fs::write(d.join("package.json"), r#"{"dependencies":{"react":"^18","left-pad":"1"},"devDependencies":{"vite":"5"}}"#).unwrap();
        fs::write(d.join("tsconfig.json"), "{ // c\n \"compilerOptions\": { \"paths\": { \"@/*\": [\"./src/*\"] } } }").unwrap();
        fs::write(d.join("src").join("App.tsx"), "import React from 'react';\nimport { Menu } from 'lucide-react';\nimport B from '@/components/B';\nimport fs from 'fs';\n").unwrap();
        let r = execute_deps(&json!({ "workspace_root": d.to_string_lossy() }), None);
        let t = serde_json::from_str::<Value>(&r).unwrap()["output"].as_str().unwrap().to_string();
        assert!(t.contains("lucide-react") && t.contains("App.tsx") && t.contains("npm install lucide-react"), "{}", t);
        assert!(!t.contains("MISSING from package.json (2)"), "{}", t);
        assert!(t.contains("left-pad"), "{}", t);
        let _ = fs::remove_dir_all(&d);
    }
}
