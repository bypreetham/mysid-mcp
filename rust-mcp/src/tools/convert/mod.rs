//! `mysid convert <source> <destination>`: a TypeScript / React / Tailwind project into a Flutter project.
//!
//! The pipeline: `parse` reads every `.ts` / `.tsx` file into a syntax tree (`ast`); `tailwind` turns class names into a
//! `Style`; `types`, `expr`, `widgets` and `component` lower the tree to Dart (JSX + Style -> widgets, hooks -> State);
//! `project` decides the file layout and writes `lib/`; `shim` is the small runtime that ships with the output.
//! Converters are registered in `CONVERTERS` the way version control commands are in `tools/vcs`.

pub mod ast;
pub mod component;
pub mod cx;
pub mod expr;
pub mod firebase;
pub mod parse;
pub mod project;
pub mod shim;
pub mod tailwind;
pub mod types;
pub mod widgets;

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

pub struct Converter {
    /// the target name: `mysid convert a b --to flutter`
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub summary: &'static str,
    pub run: fn(&Path, &Path, &Options) -> Result<String, String>,
}

pub struct Options {
    pub dry_run: bool,
    pub analyze: bool,
    pub notes: bool,
}

pub const CONVERTERS: &[Converter] = &[Converter {
    name: "flutter",
    aliases: &["dart"],
    summary: "React + TypeScript + Tailwind  ->  a Flutter project: pages, components, hooks, utils, services, models",
    run: react_to_flutter,
}];

pub fn find(name: &str) -> Option<&'static Converter> {
    CONVERTERS.iter().find(|c| c.name == name || c.aliases.contains(&name))
}

pub const HELP: &str = "mysid convert: a React / TypeScript / Tailwind project into a Flutter project

Usage:
  mysid convert <react-project> <flutter-project>            replace <flutter-project>/lib with the converted app
  mysid convert <react-project> <flutter-project> --dry-run  read and convert, print the report, write nothing
  mysid convert <react-project> <flutter-project> --analyze  also run `dart analyze lib` on the result
  mysid convert <react-project> <flutter-project> --notes    list every construct that needed a stand-in

What is converted (the folder structure and file names are kept, in snake_case):
  pages/ -> lib/pages/   components/ -> lib/components/   hooks/ + contexts/ -> lib/hooks/
  utils/ + lib/ -> lib/utils/   services/ -> lib/services/   types/ -> lib/models/   App.tsx -> lib/app.dart
  interfaces -> classes (copyWith / fromMap / toMap), useState/useRef/useEffect -> State, JSX + Tailwind -> widgets,
  react-router -> MaterialApp named routes, lucide-react icons -> Material icons.
The old lib is kept in <flutter-project>/.mysid/lib-backup-<time>. Browser-only APIs (window, indexedDB, ...) and npm packages
with no Dart twin (firebase, qrcode, ...) become stand-ins that throw UnimplementedError, listed in the report.
";

fn read_title(react: &Path) -> String {
    std::fs::read_to_string(react.join("package.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get("name").and_then(|n| n.as_str()).map(String::from))
        .unwrap_or_else(|| "App".to_string())
}

fn react_to_flutter(src: &Path, dest: &Path, opts: &Options) -> Result<String, String> {
    if !dest.join("pubspec.yaml").exists() {
        return Err(format!("{} is not a Flutter project (no pubspec.yaml). Create one with `flutter create` first.", dest.display()));
    }
    let src_root = if src.join("src").is_dir() { src.join("src") } else { src.to_path_buf() };
    let (files, skipped, mut problems) = project::scan(&src_root);
    if files.is_empty() {
        return Err(format!("no .ts / .tsx files under {}", src_root.display()));
    }
    let info = project::build_info(&files);
    let mut conv = project::convert_project(&files, &info, &read_title(src));
    conv.problems.append(&mut problems);

    let mut out = Vec::new();
    out.push(format!("convert: {} ({} files) -> {}", src_root.display(), files.len(), dest.join("lib").display()));
    let mut by_dir: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for (rel, _) in &conv.files {
        let top = rel.split('/').next().filter(|_| rel.contains('/')).unwrap_or("(root)");
        *by_dir.entry(top.to_string()).or_insert(0) += 1;
    }
    out.push(format!("  {} Dart files: {}", conv.files.len(), by_dir.iter().map(|(d, n)| format!("{} {}", d, n)).collect::<Vec<_>>().join(", ")));
    if !conv.problems.is_empty() {
        out.push(format!("  {} source problem(s): the file is converted up to the problem", conv.problems.len()));
        for p in conv.problems.iter().take(10) {
            out.push(format!("    {}", p));
        }
    }
    let css: Vec<&String> = skipped.iter().filter(|s| s.ends_with(".css")).collect();
    if !css.is_empty() {
        out.push(format!("  not converted (styles are applied from the class names in the markup): {}", css.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    let js: Vec<&String> = skipped.iter().filter(|s| s.ends_with(".js")).collect();
    if !js.is_empty() {
        out.push(format!("  not converted (JavaScript): {}", js.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
    }
    if conv.notes.is_empty() {
        out.push("  no stand-ins were needed".to_string());
    } else if opts.notes {
        out.push(format!("  {} stand-in(s) / approximations:", conv.notes.len()));
        for n in &conv.notes {
            out.push(format!("    {}", n));
        }
    } else {
        out.push(format!("  {} stand-in(s) / approximations (see them all with --notes)", conv.notes.len()));
    }
    if !conv.deps.is_empty() {
        out.push(format!("  Flutter packages needed: {}", conv.deps.join(", ")));
    }
    if opts.dry_run {
        out.push("dry run: nothing was written".to_string());
        return Ok(out.join("\n"));
    }
    let backup = project::write_lib(dest, &conv.files)?;
    out.push(format!("wrote {} files to {}", conv.files.len(), dest.join("lib").display()));
    if let Some(b) = backup {
        out.push(format!("the previous lib is in {}", b.display()));
    }
    out.push(add_packages(dest, &conv.deps));
    if opts.analyze {
        out.push(analyze(dest));
    }
    out.push(format!("next: cd {} && flutter run   (flutter analyze lists what still needs hand work)", dest.display()));
    Ok(out.join("\n"))
}

/// `flutter pub add` for the packages the converted code imports and the pubspec does not list yet.
fn add_packages(dest: &Path, deps: &[String]) -> String {
    let pubspec = std::fs::read_to_string(dest.join("pubspec.yaml")).unwrap_or_default();
    let missing: Vec<&String> = deps.iter().filter(|d| !pubspec.contains(&format!("{}:", d))).collect();
    if missing.is_empty() {
        return String::new();
    }
    let mut cmd = std::process::Command::new(if cfg!(windows) { "flutter.bat" } else { "flutter" });
    cmd.arg("pub").arg("add").args(missing.iter().map(|d| d.as_str())).current_dir(dest);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let list = missing.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(" ");
    match cmd.output() {
        Ok(o) if o.status.success() => format!("added to pubspec.yaml: {} (flutter pub add)", list),
        Ok(o) => format!("could not add {} ({}): run `flutter pub add {}` in {}", list, String::from_utf8_lossy(&o.stderr).lines().next().unwrap_or("failed"), list, dest.display()),
        Err(e) => format!("could not run flutter ({}): run `flutter pub add {}` in {}", e, list, dest.display()),
    }
}

fn analyze(dest: &Path) -> String {
    let mut cmd = std::process::Command::new(if cfg!(windows) { "dart.bat" } else { "dart" });
    cmd.args(["analyze", "lib"]).current_dir(dest);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    match cmd.output() {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string();
            let errors = text.lines().filter(|l| l.trim_start().starts_with("error")).count();
            let warnings = text.lines().filter(|l| l.trim_start().starts_with("warning")).count();
            let tail: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with("error")).take(15).collect();
            format!("dart analyze lib: {} error(s), {} warning(s){}", errors, warnings, if tail.is_empty() { String::new() } else { format!("\n  {}", tail.join("\n  ")) })
        }
        Err(e) => format!("dart analyze could not run ({})", e),
    }
}

fn path_param(params: &Value, keys: &[&str], default_root: Option<&str>) -> Option<PathBuf> {
    let raw = keys.iter().find_map(|k| params.get(*k).and_then(|v| v.as_str()))?;
    let p = PathBuf::from(raw);
    Some(if p.is_absolute() { p } else { default_root.map(|r| Path::new(r).join(&p)).unwrap_or(p) })
}

/// The command: `mysid convert <source> <destination> [--to flutter] [--dry-run] [--analyze] [--notes]`
pub fn execute_convert(params: &Value, default_root: Option<&str>) -> String {
    let reply = |ok: bool, t: String| json!({ "success": ok, "output": t }).to_string();
    let flag = |k: &str| params.get(k).map_or(false, |v| v.as_bool().unwrap_or_else(|| v.as_str().map_or(false, |s| s.is_empty() || s == "true")));
    let target = params.get("to").and_then(|v| v.as_str()).unwrap_or("flutter");
    let Some(conv) = find(target) else {
        let names: Vec<&str> = CONVERTERS.iter().map(|c| c.name).collect();
        return reply(false, format!("NOT CONVERTED: no converter named `{}` (available: {})", target, names.join(", ")));
    };
    let (Some(src), Some(dest)) = (path_param(params, &["source", "from"], default_root), path_param(params, &["dest", "destination"], default_root)) else {
        return reply(false, format!("NOT CONVERTED: usage: mysid convert <react-project> <flutter-project>\n\n{}", HELP));
    };
    if !src.exists() {
        return reply(false, format!("NOT CONVERTED: the source {} does not exist", src.display()));
    }
    let opts = Options { dry_run: flag("dry_run") || flag("dry-run") || flag("preview"), analyze: flag("analyze"), notes: flag("notes") };
    match (conv.run)(&src, &dest, &opts) {
        Ok(t) => reply(true, t),
        Err(e) => reply(false, format!("NOT CONVERTED: {}", e)),
    }
}

/// What `mysid help convert` prints: the usage and the registered converters.
pub fn overview() -> String {
    let mut out = String::from(HELP);
    out.push_str("\nconverters (--to <name>):\n");
    for c in CONVERTERS {
        out.push_str(&format!("  {:<10} {}\n", c.name, c.summary));
    }
    out
}

#[cfg(test)]
mod real_project {
    /// Reads every source file of a real React project when it is on this machine (skipped otherwise).
    #[test]
    fn the_react_sample_project_parses_without_problems() {
        let root = std::path::Path::new(r"E:\flutter\qr-web\src");
        if !root.exists() {
            return;
        }
        let (files, _, problems) = super::project::scan(root);
        assert!(!files.is_empty());
        assert!(problems.is_empty(), "{} problems:\n{}", problems.len(), problems.join("\n"));
    }
}

#[cfg(test)]
mod end_to_end {
    use super::*;

    /// A tiny React project and a Flutter scaffold in a fresh temp folder.
    fn sample(name: &str) -> (PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("mysid-convert-{}-{}", name, std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (react, flutter) = (base.join("react"), base.join("flutter"));
        let write = |p: PathBuf, text: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        write(react.join("package.json"), "{\"name\":\"tiny\"}");
        write(
            react.join("src/components/Button.tsx"),
            "import React from 'react';\ninterface ButtonProps { label: string; onClick?: () => void; }\nexport const Button: React.FC<ButtonProps> = ({ label, onClick }) => (\n  <button className=\"px-4 py-2 bg-blue-600 text-white rounded-lg\" onClick={onClick}>{label}</button>\n);\n",
        );
        write(
            react.join("src/pages/Counter.tsx"),
            "import React, { useState } from 'react';\nimport { Button } from '../components/Button';\nexport const Counter: React.FC = () => {\n  const [n, setN] = useState(0);\n  return (\n    <div className=\"p-4 flex items-center gap-2\">\n      <span className=\"text-lg font-bold\">Count: {n}</span>\n      <Button label=\"Add\" onClick={() => setN(n + 1)} />\n    </div>\n  );\n};\n",
        );
        write(react.join("src/utils/format.ts"), "export function pad(n: number): string { return n < 10 ? `0${n}` : `${n}`; }\n");
        write(flutter.join("pubspec.yaml"), "name: tiny_app\n");
        write(flutter.join("lib/main.dart"), "void main() {}\n");
        (react, flutter)
    }

    fn convert_sample(react: &Path) -> std::collections::HashMap<String, String> {
        let (files, _, problems) = project::scan(&react.join("src"));
        assert!(problems.is_empty(), "{:?}", problems);
        let info = project::build_info(&files);
        project::convert_project(&files, &info, "tiny").files.into_iter().collect()
    }

    #[test]
    fn the_folder_structure_and_file_names_are_kept() {
        let (react, _) = sample("layout");
        let out = convert_sample(&react);
        for f in ["components/button.dart", "pages/counter.dart", "utils/format.dart", "utils/js_compat.dart"] {
            assert!(out.contains_key(f), "missing {}: {:?}", f, out.keys().collect::<Vec<_>>());
        }
    }

    #[test]
    fn hooks_tailwind_and_props_become_flutter() {
        let (react, _) = sample("code");
        let out = convert_sample(&react);
        let counter = &out["pages/counter.dart"];
        assert!(counter.contains("class Counter extends StatefulWidget"), "{}", counter);
        assert!(counter.contains("int n = 0;") || counter.contains("n = 0;"), "{}", counter);
        assert!(counter.contains("setState(() { n = (n + 1); })"), "{}", counter);
        assert!(counter.contains("FontWeight.w700") && counter.contains("EdgeInsets.all(16)") && counter.contains("spacing: 8"), "{}", counter);
        assert!(counter.contains("import '../components/button.dart';"), "{}", counter);
        let button = &out["components/button.dart"];
        assert!(button.contains("required this.label") && button.contains("Color(0xFF2563EB)") && button.contains("BorderRadius.circular(8)"), "{}", button);
        assert!(out["utils/format.dart"].contains("pad(num n)"), "{}", out["utils/format.dart"]);
    }

    #[test]
    fn convert_replaces_lib_and_keeps_a_backup_and_dry_run_writes_nothing() {
        let (react, flutter) = sample("write");
        let run = |extra: &[(&str, Value)]| {
            let mut p = json!({ "source": react.to_string_lossy(), "dest": flutter.to_string_lossy() });
            for (k, v) in extra {
                p[*k] = v.clone();
            }
            serde_json::from_str::<Value>(&execute_convert(&p, None)).unwrap()
        };
        let dry = run(&[("dry_run", json!(true))]);
        assert_eq!(dry["success"], true, "{}", dry);
        assert_eq!(std::fs::read_to_string(flutter.join("lib/main.dart")).unwrap(), "void main() {}\n", "a dry run must not touch lib");
        let real = run(&[]);
        assert_eq!(real["success"], true, "{}", real);
        assert!(flutter.join("lib/pages/counter.dart").exists() && flutter.join("lib/utils/js_compat.dart").exists());
        let backups: Vec<_> = std::fs::read_dir(flutter.join(".mysid")).unwrap().flatten().collect();
        assert!(backups.iter().any(|b| b.file_name().to_string_lossy().starts_with("lib-backup-")), "the old lib is kept");
        let bad = serde_json::from_str::<Value>(&execute_convert(&json!({ "source": react.to_string_lossy(), "dest": react.to_string_lossy() }), None)).unwrap();
        assert_eq!(bad["success"], false, "a destination without pubspec.yaml is refused");
    }
}
