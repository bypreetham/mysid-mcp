//! `mysid doctor`: checks of a project folder. `compare`: can the old folder go? `health`: where the space is. `clean`: remove node_modules.

pub mod cleaner;
pub mod compare;
pub mod health;

use serde_json::{json, Value};

pub const HELP: &str = "mysid doctor: project checks (compare and health only read; clean deletes only with --yes)\n\n\
Usage:\n  \
  mysid doctor compare <old-folder> <new-folder>   what changed between two copies of a project, and whether the old one can go\n  \
  mysid health [folder] [--top 5] [--depth 1]      where the space goes: total size, the largest folders, the largest files\n  \
  mysid clean [folder] [--yes]                     delete the node_modules of a Node / React project (preview unless --yes)\n\n\
Options (after the two folders):\n  \
  --max <n>        entries listed per group (default 25)\n  \
  --brief          the totals, the verdict and the per-folder table only\n  \
  --everything     also compare generated folders (node_modules, build, .git, .gradle, .cxx, target ...), left out by default\n\n\
The first folder is treated as the old one. The answer has: files, folders and size of each side; how many files are identical,\n\
changed (with lines and which side is newer), differ only in line endings (CRLF / LF), or exist in one side only (a whole\n\
folder that exists in one side only is listed once); a table per top-level folder; and a verdict on deleting the old folder.\n\
Nothing is written, moved or deleted. Content is compared byte by byte, not by date or size alone.\n";

pub fn execute_doctor(params: &Value, default_root: Option<&str>) -> String {
    let sub = params.get("sub").or_else(|| params.get("action")).and_then(|v| v.as_str()).unwrap_or("").to_lowercase();
    match sub.as_str() {
        "compare" | "diff" | "cmp" => compare::run(params, default_root),
        "health" => health::run(params, default_root),
        "clean" => cleaner::run(params, default_root),
        "" | "help" => json!({ "success": true, "output": HELP }).to_string(),
        other => json!({ "success": false, "error": format!("unknown doctor check '{}'. Available: compare, health, clean. (mysid doctor compare <old> <new>, mysid health <folder>, mysid clean <folder>)", other) }).to_string(),
    }
}
