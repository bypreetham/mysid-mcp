use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::time::Duration;

use super::worker::{fmt_secs, now_ms, Status};

/// Distinct warning/error messages listed in a report; identical ones across files are grouped.
const MAX_WARNING_GROUPS: usize = 6;
const MAX_ERROR_GROUPS: usize = 12;
const MAX_CAUSE_LINES: usize = 6;
const MAX_MESSAGE_CHARS: usize = 170;

/// Updates live progress from one line of `gradlew --console=plain` output.
pub fn track(p: &mut Status, line: &str) {
    let l = line.trim_end();
    if let Some(rest) = l.strip_prefix("> Task ") {
        p.tasks_done += 1;
        let cached = ["UP-TO-DATE", "NO-SOURCE", "FROM-CACHE", "SKIPPED"].iter().any(|m| rest.contains(m));
        if !cached {
            p.tasks_executed += 1;
        }
        let task = rest.split_whitespace().next().unwrap_or("");
        p.current = task.to_string();
        if let Some(stage) = stage_of(task) {
            enter_stage(p, stage);
        }
    } else if l.starts_with("> Configure project") {
        p.current = "configuring".to_string();
        enter_stage(p, "configuring");
    } else if l.starts_with("e: ") || l.contains(": error:") {
        p.errors += 1;
    } else if l.starts_with("w: ") || l.contains(": warning:") {
        p.warnings += 1;
    }
}

/// Pipeline stage a Gradle task belongs to; None for tasks that don't change the stage
/// (so unrelated housekeeping tasks don't make the stage flicker).
pub fn stage_of(task: &str) -> Option<&'static str> {
    let name = task.rsplit(':').next().unwrap_or(task).to_ascii_lowercase();
    let has = |k: &str| name.contains(k);
    Some(if has("externalnativebuild") || has("cmake") || has("ndk") {
        "native C++"
    } else if has("kapt") || has("ksp") {
        "annotation processing"
    } else if has("compile") && has("kotlin") {
        "Kotlin compile"
    } else if has("compile") && has("java") {
        "Java compile"
    } else if has("r8") || has("minify") || has("shrink") || has("proguard") {
        "R8 shrink"
    } else if has("dex") {
        "dexing"
    } else if has("lint") {
        "lint"
    } else if has("test") {
        "tests"
    } else if has("nativelibs") || has("strip") || has("package") || has("assemble") || has("bundle") || has("sign") || has("zipalign") {
        "packaging"
    } else if has("manifest") || has("resource") || has("aapt") || has("databinding") || has("buildconfig") || has("metadata")
        || has("prebuild") || has("generate") || has("merge") || has("check")
    {
        "resources/manifest"
    } else {
        return None;
    })
}

/// Moves to `stage`, adding the time spent in the previous one to the per-stage totals.
pub fn enter_stage(p: &mut Status, stage: &str) {
    if p.stage == stage {
        return;
    }
    close_stage(p, now_ms());
    p.stage = stage.to_string();
    p.stage_started_ms = now_ms();
    p.stage_log.push((now_ms().saturating_sub(p.started_ms) / 1000, stage.to_string(), p.current.clone()));
}

/// Books the time since the current stage began into its total.
pub fn close_stage(p: &mut Status, now: u64) {
    if p.stage.is_empty() || p.stage_started_ms == 0 {
        return;
    }
    let spent = now.saturating_sub(p.stage_started_ms);
    match p.stage_ms.iter_mut().find(|(name, _)| *name == p.stage) {
        Some(entry) => entry.1 += spent,
        None => p.stage_ms.push((p.stage.clone(), spent)),
    }
    p.stage_started_ms = now;
}

/// `stages: configuring 12s | Kotlin compile 1m10s | native C++ 2m05s` (stages under 2s omitted).
fn stage_line(p: &Status) -> Option<String> {
    let parts: Vec<String> = p
        .stage_ms
        .iter()
        .filter(|(_, ms)| *ms >= 2_000)
        .map(|(name, ms)| format!("{} {}", name, fmt_secs(ms / 1000)))
        .collect();
    if parts.is_empty() { None } else { Some(format!("stages: {}", parts.join(" | "))) }
}

/// Splits a cleaned diagnostic into (location, message): `app/A.kt:3:1 msg` or `x.cpp:9:2: warning: msg`.
fn split_diag(entry: &str) -> (String, String) {
    for marker in [": warning:", ": error:"] {
        if let Some(pos) = entry.find(marker) {
            return (entry[..pos].to_string(), entry[pos + marker.len()..].trim().to_string());
        }
    }
    if let Some((first, rest)) = entry.split_once(' ') {
        if first.contains(':') && first.chars().any(|c| c.is_ascii_digit()) {
            return (first.to_string(), rest.trim().to_string());
        }
    }
    (String::new(), entry.to_string())
}

/// Collapses diagnostics to one line per distinct message, most frequent first:
/// `12x 'foo' is deprecated  (A.kt:3, B.kt:9, +10)`. Keeps an LLM's token cost flat when the same
/// warning repeats across files. Overflow is summarized with a pointer to the full log.
pub fn group_diagnostics(entries: &[String], max_groups: usize) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
    for e in entries {
        let (loc, msg) = split_diag(e);
        groups.entry(msg.clone()).or_insert_with(|| {
            order.push(msg.clone());
            Vec::new()
        });
        if let Some(list) = groups.get_mut(&msg) {
            list.push(loc);
        }
    }
    let mut ranked: Vec<(&String, &Vec<String>)> = order.iter().map(|m| (m, &groups[m])).collect();
    ranked.sort_by(|a, b| b.1.len().cmp(&a.1.len())); // stable: ties keep first-seen order

    let mut lines: Vec<String> = ranked
        .iter()
        .take(max_groups)
        .map(|(msg, locs)| {
            let shown: Vec<&str> = locs.iter().filter(|l| !l.is_empty()).take(2).map(|l| l.as_str()).collect();
            if locs.len() == 1 {
                if shown.is_empty() { (*msg).clone() } else { format!("{} {}", shown[0], msg) }
            } else {
                let more = locs.len().saturating_sub(shown.len());
                let where_ = if shown.is_empty() {
                    String::new()
                } else if more > 0 {
                    format!("  ({}, +{})", shown.join(", "), more)
                } else {
                    format!("  ({})", shown.join(", "))
                };
                format!("{}x {}{}", locs.len(), msg, where_)
            }
        })
        .collect();
    if ranked.len() > max_groups {
        lines.push(format!("... +{} more kinds (full list: .mysid/build.log)", ranked.len() - max_groups));
    }
    lines
}

/// Final report: counts, the diagnostics that matter, and the APK. On failure only errors and the
/// "What went wrong" cause are shown; on success the first few warnings.
pub fn summarize(
    output: &str,
    ok: bool,
    elapsed: Duration,
    status: &Status,
    workspace_root: &str,
    since_ms: u64,
    release: bool,
) -> String {
    let root = workspace_root.replace('\\', "/");
    let mut warnings: Vec<String> = Vec::new();
    let mut errors: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();

    for raw in output.lines() {
        let line = raw.trim();
        let (is_error, body) = if let Some(b) = line.strip_prefix("e: ") {
            (true, b)
        } else if let Some(b) = line.strip_prefix("w: ") {
            (false, b)
        } else if line.contains(": error:") {
            (true, line)
        } else if line.contains(": warning:") {
            (false, line)
        } else {
            continue;
        };
        let entry = clip(&relativize(body, &root));
        if seen.insert(entry.clone()) {
            if is_error { errors.push(entry) } else { warnings.push(entry) }
        }
    }

    let cause = what_went_wrong(output);
    let failed_task = output
        .lines()
        .find_map(|l| l.split("Execution failed for task '").nth(1))
        .and_then(|rest| rest.split('\'').next())
        .map(|t| t.to_string());

    let tasks = if status.tasks_done > 0 {
        format!("{} tasks ({} executed)", status.tasks_done, status.tasks_executed)
    } else {
        String::new()
    };

    let (word, stack) = if release { ("RELEASE", "Android") } else { ("BUILD", "Android/Gradle") };
    let mut out = if ok {
        let mut parts = vec![];
        if status.bump_to > 0 {
            parts.push(format!("versionCode {} -> {}", status.bump_from, status.bump_to));
        }
        if !tasks.is_empty() {
            parts.push(tasks);
        }
        parts.push(format!("{} warning(s)", warnings.len()));
        format!("{} OK ({}, {}) - {}", word, stack, fmt_secs(elapsed.as_secs()), parts.join(", "))
    } else {
        let what = failed_task.map(|t| format!("{} failed, ", t)).unwrap_or_default();
        format!(
            "{} FAILED ({}, {}) - {}{} error(s)",
            word,
            stack,
            fmt_secs(elapsed.as_secs()),
            what,
            errors.len()
        )
    };

    if ok {
        for w in group_diagnostics(&warnings, MAX_WARNING_GROUPS) {
            out.push_str(&format!("\n  {}", w));
        }
        let root = Path::new(workspace_root);
        if release {
            match newest_artifact(root, since_ms, "bundle", "aab") {
                Some(aab) => out.push_str(&format!("\nAAB: {}", aab)),
                None => out.push_str("\nAAB: not found under */build/outputs/bundle/"),
            }
            out.push_str(&format!("\nR8 mapping: {}", mapping_status(root)));
        } else if let Some(apk) = newest_artifact(root, since_ms, "apk", "apk") {
            out.push_str(&format!("\nAPK: {}", apk));
        }
    } else {
        for e in group_diagnostics(&errors, MAX_ERROR_GROUPS) {
            out.push_str(&format!("\n  {}", e));
        }
        if !cause.is_empty() {
            out.push_str("\ncause:");
            for c in cause.iter().take(MAX_CAUSE_LINES) {
                out.push_str(&format!("\n  {}", clip(c)));
            }
        }
    }
    if let Some(line) = stage_line(status) {
        out.push_str(&format!("\n{}", line));
    }
    out
}

/// The block Gradle prints under `* What went wrong:`.
fn what_went_wrong(output: &str) -> Vec<String> {
    let mut lines = output.lines().skip_while(|l| !l.trim_start().starts_with("* What went wrong:")).skip(1);
    let mut block = Vec::new();
    for l in lines.by_ref() {
        let t = l.trim();
        if t.starts_with("* ") || t.starts_with("BUILD FAILED") {
            break;
        }
        if !t.is_empty() {
            block.push(t.to_string());
        }
    }
    block
}

/// `file:///E:/proj/app/Foo.kt:12:5 msg` -> `app/Foo.kt:12:5 msg` (workspace-relative, `/` separators).
fn relativize(text: &str, root: &str) -> String {
    let normalized = text.replace('\\', "/").replace("file:///", "");
    let lower = normalized.to_ascii_lowercase();
    let root_lower = root.trim_end_matches('/').to_ascii_lowercase();
    match lower.find(&root_lower) {
        Some(pos) if !root_lower.is_empty() => {
            let mut rest = normalized[pos + root_lower.len()..].to_string();
            if rest.starts_with('/') {
                rest.remove(0);
            }
            format!("{}{}", &normalized[..pos], rest)
        }
        _ => normalized,
    }
}

fn clip(s: &str) -> String {
    if s.chars().count() > MAX_MESSAGE_CHARS {
        let cut: String = s.chars().take(MAX_MESSAGE_CHARS).collect();
        format!("{}...", cut)
    } else {
        s.to_string()
    }
}

/// R8 mapping file state. An empty mapping.txt is as bad as a missing one: crash reports from the
/// release build cannot be de-obfuscated, so it is called out instead of reported as "present".
fn mapping_status(root: &Path) -> String {
    let sizes: Vec<u64> = fs::read_dir(root).map_or(Vec::new(), |entries| {
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| fs::metadata(e.path().join("build").join("outputs").join("mapping").join("release").join("mapping.txt")).ok())
            .map(|m| m.len())
            .collect()
    });
    match sizes.iter().max() {
        None => "missing".to_string(),
        Some(0) => "EMPTY (0 bytes) - R8 wrote no mapping, so release crash reports cannot be de-obfuscated".to_string(),
        Some(n) => format!("present ({:.1} MB)", *n as f64 / 1_048_576.0),
    }
}

/// Newest `*.<ext>` under `<module>/build/outputs/<kind>/`: its path, mtime (ms) and size.
pub fn newest_artifact_path(root: &Path, _since_ms: u64, kind: &str, ext: &str) -> Option<(std::path::PathBuf, u64, u64)> {
    let mut best: Option<(u64, std::path::PathBuf, u64)> = None;
    let mut stack: Vec<(std::path::PathBuf, usize)> = fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| (e.path().join("build").join("outputs").join(kind), 0))
        .collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for e in entries.filter_map(|e| e.ok()) {
            let p = e.path();
            if p.is_dir() && depth < 3 {
                stack.push((p, depth + 1));
            } else if p.extension().and_then(|x| x.to_str()) == Some(ext) {
                let Ok(meta) = e.metadata() else { continue };
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                if best.as_ref().map_or(true, |b| mtime > b.0) {
                    best = Some((mtime, p, meta.len()));
                }
            }
        }
    }
    best.map(|(mtime, path, len)| (path, mtime, len))
}

/// `rel/path (12.3 MB)`, flagged when it predates this build.
fn newest_artifact(root: &Path, since_ms: u64, kind: &str, ext: &str) -> Option<String> {
    let (path, mtime, len) = newest_artifact_path(root, since_ms, kind, ext)?;
    let rel = path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().to_string());
    let note = if mtime + 5_000 < since_ms { " (not rebuilt - up to date)" } else { "" };
    Some(format!("{} ({:.1} MB){}", rel, len as f64 / 1_048_576.0, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_tasks_and_diagnostics() {
        let mut s = Status::default();
        track(&mut s, "> Configure project :app");
        assert_eq!(s.current, "configuring");
        track(&mut s, "> Task :app:preBuild UP-TO-DATE");
        track(&mut s, "> Task :app:compileDebugKotlin");
        track(&mut s, "w: file:///E:/p/app/A.kt:3:1 unused variable");
        track(&mut s, "e: file:///E:/p/app/A.kt:9:5 unresolved reference: x");
        assert_eq!((s.tasks_done, s.tasks_executed), (2, 1));
        assert_eq!(s.current, ":app:compileDebugKotlin");
        assert_eq!((s.warnings, s.errors), (1, 1));
    }

    #[test]
    fn maps_tasks_to_stages_and_ignores_unknown_ones() {
        assert_eq!(stage_of(":app:compileDebugKotlin"), Some("Kotlin compile"));
        assert_eq!(stage_of(":app:compileDebugJavaWithJavac"), Some("Java compile"));
        assert_eq!(stage_of(":app:buildCMakeDebug[arm64-v8a]"), Some("native C++"));
        assert_eq!(stage_of(":app:minifyReleaseWithR8"), Some("R8 shrink"));
        assert_eq!(stage_of(":app:dexBuilderDebug"), Some("dexing"));
        assert_eq!(stage_of(":app:packageDebug"), Some("packaging"));
        assert_eq!(stage_of(":app:processDebugManifest"), Some("resources/manifest"));
        assert_eq!(stage_of(":somethingUnrelated"), None);
    }

    #[test]
    fn stage_log_records_the_first_task_of_each_stage() {
        let mut s = Status::default();
        track(&mut s, "> Configure project :app");
        track(&mut s, "> Task :app:kspDebugKotlin");
        track(&mut s, "> Task :app:kspDebugKotlin2");
        track(&mut s, "> Task :app:compileDebugKotlin");
        let log: Vec<(&str, &str)> = s.stage_log.iter().map(|(_, st, t)| (st.as_str(), t.as_str())).collect();
        assert_eq!(
            log,
            vec![
                ("configuring", "configuring"),
                ("annotation processing", ":app:kspDebugKotlin"),
                ("Kotlin compile", ":app:compileDebugKotlin"),
            ]
        );
    }

    #[test]
    fn stage_changes_are_tracked_and_timed() {
        let mut s = Status::default();
        track(&mut s, "> Configure project :app");
        assert_eq!(s.stage, "configuring");
        track(&mut s, "> Task :app:compileDebugKotlin");
        assert_eq!(s.stage, "Kotlin compile");
        track(&mut s, "> Task :app:somethingUnrelated");
        assert_eq!(s.stage, "Kotlin compile", "unknown tasks keep the current stage");
        s.stage_started_ms = 1_000;
        close_stage(&mut s, 4_500);
        let kotlin = s.stage_ms.iter().find(|(n, _)| n == "Kotlin compile").unwrap().1;
        assert_eq!(kotlin, 3_500);
        assert_eq!(stage_line(&s).unwrap(), "stages: Kotlin compile 3s");
    }

    #[test]
    fn repeated_warnings_collapse_into_counted_groups() {
        let entries: Vec<String> = vec![
            "app/A.kt:3:1 'foo' is deprecated".into(),
            "app/B.kt:9:5 'foo' is deprecated".into(),
            "app/C.kt:1:1 'foo' is deprecated".into(),
            "app/D.kt:2:2 unused variable 'x'".into(),
            "app/e.cpp:7:3: warning: unused parameter 'p' [-Wunused-parameter]".into(),
        ];
        let lines = group_diagnostics(&entries, 2);
        assert_eq!(lines[0], "3x 'foo' is deprecated  (app/A.kt:3:1, app/B.kt:9:5, +1)");
        assert_eq!(lines[1], "app/D.kt:2:2 unused variable 'x'");
        assert_eq!(lines[2], "... +1 more kinds (full list: .mysid/build.log)");
    }

    #[test]
    fn failure_summary_shows_errors_and_cause_not_warnings() {
        let out = "> Task :app:compileDebugKotlin FAILED\n\
w: file:///E:/p/app/A.kt:3:1 unused variable\n\
e: file:///E:/p/app/A.kt:9:5 unresolved reference: x\n\
FAILURE: Build failed with an exception.\n\
* What went wrong:\n\
Execution failed for task ':app:compileDebugKotlin'.\n\
> Compilation error. See log for more details\n\
\n\
* Try:\n";
        let mut st = Status::default();
        st.tasks_done = 12;
        let text = summarize(out, false, Duration::from_secs(65), &st, "E:\\p", 0, false);
        assert!(text.starts_with("BUILD FAILED (Android/Gradle, 1m05s) - :app:compileDebugKotlin failed, 1 error(s)"), "{}", text);
        assert!(text.contains("app/A.kt:9:5 unresolved reference: x"), "{}", text);
        assert!(text.contains("cause:") && text.contains("Compilation error"), "{}", text);
        assert!(!text.contains("unused variable"), "{}", text);
    }

    #[test]
    fn success_summary_lists_warnings_with_relative_paths() {
        let out = "w: file:///E:/p/app/A.kt:3:1 unused variable\nw: file:///E:/p/app/A.kt:3:1 unused variable\n";
        let mut st = Status::default();
        st.tasks_done = 40;
        st.tasks_executed = 3;
        let text = summarize(out, true, Duration::from_secs(192), &st, "E:\\p", 0, false);
        assert!(text.starts_with("BUILD OK (Android/Gradle, 3m12s) - 40 tasks (3 executed), 1 warning(s)"), "{}", text);
        assert!(text.contains("  app/A.kt:3:1 unused variable"), "{}", text);
    }
}
