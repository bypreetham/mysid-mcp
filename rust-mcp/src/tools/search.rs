use ignore::{WalkBuilder, WalkState};
use regex::bytes::{Regex, RegexBuilder};
use serde_json::Value;
use std::fs;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use super::workspace::{resolve_file_in_workspace, resolve_root};
use crate::adapters::detect_adapter;

use crate::textindex::{BINARY_EXTS, MAX_FILE_BYTES};

/// Hit lines are cut here so a minified file can't flood the output.
const MAX_LINE_CHARS: usize = 240;
/// A file the user names explicitly is searched even if it is large (e.g. an R8 mapping.txt).
const MAX_NAMED_FILE_BYTES: u64 = 256_000_000;
/// Once this many hits are collected the walk stops; results are then sorted and trimmed.
const SOFT_STOP_HITS: usize = 500;

struct Options {
    max_results: usize,
    context: usize,
    /// Matches shown per file (0 = no limit).
    per_file: usize,
    /// Lower-case extensions without dots; empty means "all text files".
    types: Vec<String>,
    /// `--group`: the file name once, then `  line: text` for each hit (saves tokens on big result sets).
    group: bool,
    /// `--all`: also search prose files (md, txt, rst), which a code search leaves out by default.
    prose: bool,
    /// `--vendor`: also search external/, third_party/, vendor/ folders (copied libraries), which are left out by default.
    vendor: bool,
    /// `--count`: how many matches in each file, not the matches themselves.
    count: bool,
    /// `--offset n`: skip the first n matches (the next page).
    offset: usize,
}

impl Options {
    /// Hits after which the walk stops: enough for the page asked for, or a very high bound when counting.
    fn soft_stop(&self) -> usize {
        if self.count { 500_000 } else { SOFT_STOP_HITS.max(self.offset + self.max_results + 100) }
    }
}

/// Folders of copied third-party code: searching them buries the project's own hits.
const VENDOR_DIRS: &[&str] = &["external", "externals", "extern", "third_party", "third-party", "thirdparty", "3rdparty", "3rd-party", "3rd_party", "vendor", "contrib"];

fn in_vendor(path: &Path) -> bool {
    path.components().any(|c| VENDOR_DIRS.contains(&c.as_os_str().to_string_lossy().to_lowercase().as_str()))
}

struct Hit {
    line: usize,
    text: String,
    before: Vec<(usize, String)>,
    after: Vec<(usize, String)>,
}

struct FileHits {
    rel: String,
    hits: Vec<Hit>,
}

pub fn execute_search(params: &Value, default_root: Option<&str>) -> String {
    let project_path = match resolve_root(params, "project_path", default_root) {
        Ok(p) => p,
        Err(e) => return e,
    };
    let project_dir = Path::new(&project_path);
    if !project_dir.exists() {
        return format!("ERROR: Project directory does not exist: {}", project_path);
    }

    let mut terms = collect_terms(params);
    if terms.is_empty() {
        return "ERROR: search() requires at least one term. Example: search(timer)".to_string();
    }
    // Extra words that name existing files or directories scope the search to them.
    let scopes = match extract_scopes(params, project_dir, &project_path, &mut terms) {
        Ok(s) => s,
        Err(e) => return e,
    };

    // Both spellings work: the CLI/MCP names (types, max, case, regex, context) and the
    // long internal ones.
    let opts = Options {
        max_results: u64_param(params, &["max_results", "max"]).unwrap_or(15).clamp(1, 500) as usize,
        context: if bool_param(params, &["count", "counts"]).unwrap_or(false) { 0 } else { u64_param(params, &["context_lines", "context"]).unwrap_or(0).min(2) as usize },
        per_file: u64_param(params, &["per_file", "per-file"]).unwrap_or(0) as usize,
        types: list_param(params, &["file_types", "types"]),
        group: bool_param(params, &["group", "grouped"]).unwrap_or(false),
        prose: bool_param(params, &["all", "prose"]).unwrap_or(false),
        // Naming a folder inside external/ is asking for it.
        vendor: bool_param(params, &["vendor"]).unwrap_or(false) || scopes.iter().any(|s| in_vendor(s)),
        count: bool_param(params, &["count", "counts"]).unwrap_or(false),
        offset: u64_param(params, &["offset", "skip"]).unwrap_or(0) as usize,
    };
    let case_sensitive = bool_param(params, &["case_sensitive", "case"]).unwrap_or(false);
    let explicit_regex = bool_param(params, &["is_regex", "regex"]);

    let regex = match build_regex(&terms, explicit_regex, case_sensitive) {
        Ok(r) => r,
        Err(e) => return format!("ERROR: invalid search pattern: {e}"),
    };

    let adapter = detect_adapter(project_dir);

    let collected: Mutex<Vec<FileHits>> = Mutex::new(Vec::new());
    let total_hits = AtomicUsize::new(0);

    // Whole-workspace searches go through the text index: files with a fresh filter are read only if the
    // pattern may be in them, unseen or changed files are read and their filter is saved from that read.
    // Named files/directories, or an index problem, use the plain walk below.
    let t_search = std::time::Instant::now();
    // (The index never holds copied-library folders, so --vendor uses the walk too.)
    let indexed = if scopes.is_empty() && !opts.vendor { crate::textindex::plan(project_dir, regex.as_str()) } else { None };
    if let Some(plan) = &indexed {
        scan_plan(project_dir, plan, &regex, &opts, &total_hits, &collected);
    }

    let root = project_dir.to_path_buf();
    // Files/directories named on the command line replace the whole-workspace walk.
    let roots: Vec<PathBuf> = if scopes.is_empty() { vec![project_dir.to_path_buf()] } else { scopes.clone() };
    let named_files: HashSet<PathBuf> = scopes.iter().filter(|p| p.is_file()).cloned().collect();
    let mut builder = WalkBuilder::new(&roots[0]);
    for extra in &roots[1..] {
        builder.add(extra);
    }
    let include_vendor = opts.vendor;
    builder
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .filter_entry(move |entry| {
            if entry.depth() == 0 {
                return true; // a root the user named is never skipped
            }
            let name = entry.file_name().to_string_lossy();
            // `--vendor` (or a folder named inside one) overrides the stack's skip of copied libraries.
            if include_vendor && entry.file_type().map_or(false, |t| t.is_dir()) && VENDOR_DIRS.contains(&name.to_lowercase().as_str()) {
                return true;
            }
            !adapter.is_skipped_entry(&relative(entry.path(), &root), name.as_ref())
        });
    let walker = builder.build_parallel();

    if indexed.is_none() { walker.run(|| {
        Box::new(|entry| {
            if total_hits.load(Ordering::Relaxed) >= opts.soft_stop() {
                return WalkState::Quit;
            }
            let Ok(entry) = entry else { return WalkState::Continue };
            if !entry.file_type().map_or(false, |t| t.is_file()) {
                return WalkState::Continue;
            }
            let path = entry.path();
            let named = named_files.contains(path);
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

            if named {
                // The user asked for this file: skip the extension and size heuristics.
                if entry.metadata().map_or(false, |m| m.len() > MAX_NAMED_FILE_BYTES) {
                    return WalkState::Continue;
                }
            } else {
                if BINARY_EXTS.contains(&ext.as_str()) {
                    return WalkState::Continue;
                }
                if opts.types.is_empty() {
                    // Default: keep prose out of code search results.
                    if !opts.prose && matches!(ext.as_str(), "md" | "txt" | "rst") {
                        return WalkState::Continue;
                    }
                } else if !opts.types.iter().any(|t| *t == ext) {
                    return WalkState::Continue;
                }
                if entry.metadata().map_or(false, |m| m.len() > MAX_FILE_BYTES) {
                    return WalkState::Continue;
                }
            }

            if let Some(hits) = scan_file(path, &regex, &opts) {
                total_hits.fetch_add(hits.len(), Ordering::Relaxed);
                collected.lock().unwrap().push(FileHits { rel: relative(path, project_dir), hits });
            }
            WalkState::Continue
        })
    }); }
    if crate::index::timing_enabled() {
        eprintln!("[timing] search ({}): {:?}", if indexed.is_some() { "indexed" } else { "full scan" }, t_search.elapsed());
    }

    let mut files = collected.into_inner().unwrap();
    // Parallel completion order is arbitrary; sort so identical queries print identical output.
    files.sort_by(|a, b| a.rel.cmp(&b.rel));

    let found: usize = files.iter().map(|f| f.hits.len()).sum();
    if found == 0 {
        let mut hints: Vec<&str> = Vec::new();
        if opts.types.is_empty() && !opts.prose {
            hints.push("md, txt and rst files are not searched by default: add --all, or --types md,txt");
        }
        if !opts.vendor {
            hints.push("external/, third_party/ and vendor/ folders are not searched by default: add --vendor");
        }
        return if hints.is_empty() { "No matches found.".to_string() } else { format!("No matches found.\n({})", hints.join("; ")) };
    }

    if opts.count {
        let mut rows: Vec<(usize, &str)> = files.iter().map(|f| (f.hits.len(), f.rel.as_str())).collect();
        rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
        let plus = if total_hits.load(Ordering::Relaxed) >= opts.soft_stop() { "+" } else { "" };
        let limit = u64_param(params, &["max_results", "max"]).map_or(60, |m| m as usize).clamp(1, 500);
        let mut out = vec![format!("{}{} match line(s) in {} file(s):", found, plus, rows.len())];
        out.extend(rows.iter().take(limit).map(|(n, rel)| format!("{:>6}  {}", n, rel)));
        if rows.len() > limit {
            out.push(format!("... {} more file(s); --max {} shows more", rows.len() - limit, rows.len().min(500)));
        }
        return out.join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    let mut shown = 0;
    let mut passed = 0;
    'files: for f in &files {
        let mut in_file = 0usize;
        let mut skipped = 0usize;
        for hit in &f.hits {
            if passed < opts.offset {
                passed += 1;
                continue;
            }
            if shown >= opts.max_results {
                break 'files;
            }
            if opts.per_file > 0 && in_file >= opts.per_file {
                skipped += 1;
                continue;
            }
            in_file += 1;
            shown += 1;
            if opts.group {
                if in_file == 1 {
                    out.push(f.rel.clone());
                }
                for (n, text) in &hit.before {
                    out.push(format!("  {}- {}", n, text));
                }
                out.push(format!("  {}: {}", hit.line, hit.text));
                for (n, text) in &hit.after {
                    out.push(format!("  {}- {}", n, text));
                }
                continue;
            }
            for (n, text) in &hit.before {
                out.push(format!("{}-{}: {}", f.rel, n, text));
            }
            out.push(format!("{}:{}: {}", f.rel, hit.line, hit.text));
            for (n, text) in &hit.after {
                out.push(format!("{}-{}: {}", f.rel, n, text));
            }
        }
        if skipped > 0 {
            out.push(format!("{}: +{} more match(es) in this file (--per-file {} to change)", f.rel, skipped, opts.per_file));
        }
    }
    if found > shown + opts.offset {
        let next = opts.offset + shown;
        let plus = if total_hits.load(Ordering::Relaxed) >= opts.soft_stop() { "+" } else { "" };
        out.push(format!(
            "... showing {}-{} of {}{} matches; --offset {} for the next page, --count for a per-file summary, or narrow with --types / a more specific query",
            opts.offset + 1, next, found, plus, next
        ));
    }
    out.join("\n")
}

/// Reads the files the text plan lists, on all cores, applying the same type filter as the walk, and saves the
/// filters of files it had not seen (in batches, so a long first search keeps what it learned).
fn scan_plan(
    root: &Path,
    plan: &crate::textindex::Plan,
    regex: &Regex,
    opts: &Options,
    total_hits: &AtomicUsize,
    collected: &Mutex<Vec<FileHits>>,
) {
    let wanted: Vec<&crate::textindex::Item> = plan
        .items
        .iter()
        .filter(|it| {

            let ext = Path::new(it.rel.as_str()).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
            if opts.types.is_empty() {
                opts.prose || !matches!(ext.as_str(), "md" | "txt" | "rst")
            } else {
                opts.types.iter().any(|t| *t == ext)
            }
        })
        .collect();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(wanted.len().max(1));
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                let mut learned: Vec<crate::textindex::Learned> = Vec::new();
                loop {
                    if total_hits.load(Ordering::Relaxed) >= opts.soft_stop() {
                        break;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = wanted.get(i) else { break };
                    let Ok(buf) = fs::read(root.join(&item.rel)) else { continue };
                    if item.learn {
                        learned.push(crate::textindex::Learned {
                            rel: item.rel.clone(),
                            mtime: item.mtime,
                            size: item.size,
                            bloom: crate::textindex::build_bloom(&buf).unwrap_or_default(),
                        });
                        if learned.len() >= 500 {
                            plan.save(&mut learned);
                        }
                    }
                    if let Some(hits) = scan_buf(&buf, regex, opts) {
                        total_hits.fetch_add(hits.len(), Ordering::Relaxed);
                        collected.lock().unwrap().push(FileHits { rel: item.rel.clone(), hits });
                    }
                }
                plan.save(&mut learned);
            });
        }
    });
    plan.finish();
}

/// Matches the whole file buffer at once (no per-line regex calls, no String/Vec<&str> building)
/// and only works out line numbers and text for the lines that actually hit.
fn scan_file(path: &Path, regex: &Regex, opts: &Options) -> Option<Vec<Hit>> {
    let buf = fs::read(path).ok()?;
    scan_buf(&buf, regex, opts)
}

fn scan_buf(buf: &[u8], regex: &Regex, opts: &Options) -> Option<Vec<Hit>> {
    if buf[..buf.len().min(8192)].contains(&0) {
        return None; // binary
    }

    let mut hits: Vec<Hit> = Vec::new();
    let mut last_line_start = usize::MAX;
    let mut counted_to = 0usize;
    let mut line_no = 1usize;

    for m in regex.find_iter(&buf) {
        let start = buf[..m.start()].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
        if start == last_line_start {
            continue; // several matches on one line count once
        }
        last_line_start = start;
        let end = buf[m.start()..].iter().position(|&b| b == b'\n').map_or(buf.len(), |p| m.start() + p);

        line_no += buf[counted_to..start].iter().filter(|&&b| b == b'\n').count();
        counted_to = start;

        let mut before = Vec::new();
        let mut cursor = start;
        for k in 1..=opts.context {
            let Some((s, e)) = prev_line(&buf, cursor) else { break };
            before.push((line_no - k, clip(&buf[s..e])));
            cursor = s;
        }
        before.reverse();

        let mut after = Vec::new();
        let mut cursor = end;
        for k in 1..=opts.context {
            let Some((s, e)) = next_line(&buf, cursor) else { break };
            after.push((line_no + k, clip(&buf[s..e])));
            cursor = e;
        }

        hits.push(Hit { line: line_no, text: clip_at(&buf[start..end], m.start() - start), before, after });
        if hits.len() >= opts.soft_stop() {
            break; // one huge file must not hold everything in memory
        }
    }
    if hits.is_empty() { None } else { Some(hits) }
}

/// Previous line before the line starting at `line_start`, as a byte range.
fn prev_line(buf: &[u8], line_start: usize) -> Option<(usize, usize)> {
    if line_start == 0 {
        return None;
    }
    let end = line_start - 1; // the '\n' that ends the previous line
    let start = buf[..end].iter().rposition(|&b| b == b'\n').map_or(0, |p| p + 1);
    Some((start, end))
}

/// Next line after the line ending at `line_end` (the index of its '\n' or the buffer end).
fn next_line(buf: &[u8], line_end: usize) -> Option<(usize, usize)> {
    let start = line_end + 1;
    if start >= buf.len() {
        return None;
    }
    let end = buf[start..].iter().position(|&b| b == b'\n').map_or(buf.len(), |p| start + p);
    Some((start, end))
}

/// A long line (minified bundles, generated glue) is cut to a window around the match, not its first characters.
fn clip_at(bytes: &[u8], match_at: usize) -> String {
    let text = String::from_utf8_lossy(bytes);
    let total = text.chars().count();
    if total <= MAX_LINE_CHARS + 20 {
        return text.trim().to_string();
    }
    let at = String::from_utf8_lossy(&bytes[..match_at.min(bytes.len())]).chars().count();
    let from = at.saturating_sub(60).min(total - MAX_LINE_CHARS);
    let window: String = text.chars().skip(from).take(MAX_LINE_CHARS).collect();
    let after = total - from - MAX_LINE_CHARS;
    format!("{}{}{}", if from > 0 { format!("...({} chars) ", from) } else { String::new() }, window.trim(), if after > 0 { format!(" ...(+{} chars)", after) } else { String::new() })
}

fn clip(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    if trimmed.chars().count() > MAX_LINE_CHARS {
        let cut: String = trimmed.chars().take(MAX_LINE_CHARS).collect();
        format!("{}...", cut)
    } else {
        trimmed.to_string()
    }
}

/// Builds the matcher. Several terms are an OR of literals. A single term is literal unless
/// `--regex` was given or it visibly uses regex syntax (`a|b`, `\b`, `.*`, ...), which agents
/// write all the time; if such a pattern doesn't compile it falls back to a literal search.
fn build_regex(terms: &[String], explicit_regex: Option<bool>, case_sensitive: bool) -> Result<Regex, String> {
    // multi_line: `^` and `$` match at every line, as in grep (the file is searched as one buffer).
    let build = |pattern: &str| {
        RegexBuilder::new(pattern).case_insensitive(!case_sensitive).multi_line(true).build()
    };

    if terms.len() > 1 {
        let joined = terms.iter().map(|t| regex::escape(t)).collect::<Vec<_>>().join("|");
        return build(&joined).map_err(|e| e.to_string());
    }

    let term = &terms[0];
    let wants_regex = explicit_regex.unwrap_or_else(|| looks_like_regex(term));
    if wants_regex {
        // grep's `\|` is alternation; in this regex syntax it would be a literal bar, which nobody means.
        let pattern = term.replace("\\|", "|");
        match build(&pattern) {
            Ok(r) => return Ok(r),
            Err(e) if explicit_regex == Some(true) => return Err(e.to_string()),
            Err(_) => {} // auto-detected but invalid: treat as literal text
        }
    }

    let is_symbol = term.chars().all(|c| c.is_alphanumeric() || c == '_');
    let pattern = if is_symbol {
        format!(r"\b{}\b", regex::escape(term))
    } else {
        regex::escape(term)
    };
    build(&pattern).map_err(|e| e.to_string())
}

fn looks_like_regex(q: &str) -> bool {
    q.contains('|')
        || ["\\b", "\\w", "\\d", "\\s", ".*", ".+"].iter().any(|m| q.contains(m))
        || q.starts_with('^')
        || q.ends_with('$')
}

/// Extra command-line words (`search "^class" LibraryScanner.kt src/ui`) that name existing files or
/// directories become the places to search; any other extra word stays part of the query.
fn extract_scopes(params: &Value, root: &Path, root_str: &str, terms: &mut Vec<String>) -> Result<Vec<PathBuf>, String> {
    let extra: Vec<String> = params
        .get("extra")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.trim().to_string())).collect())
        .unwrap_or_default();

    let mut scopes: Vec<PathBuf> = Vec::new();
    let mut words: Vec<String> = Vec::new();
    for tok in extra.into_iter().filter(|t| !t.is_empty()) {
        let has_sep = tok.contains('/') || tok.contains('\\');
        let has_ext = tok.rsplit_once('.').map_or(false, |(stem, ext)| {
            !stem.is_empty() && !ext.is_empty() && ext.len() <= 6 && ext.chars().all(|c| c.is_ascii_alphanumeric())
        });
        if !(has_sep || has_ext) {
            words.push(tok);
            continue;
        }
        let direct = root.join(&tok);
        if direct.exists() {
            scopes.push(direct);
            continue;
        }
        match resolve_file_in_workspace(root, &tok, root_str) {
            Ok(p) => scopes.push(p),
            Err(_) if has_sep => return Err(format!("ERROR: path not found: {}", tok)),
            Err(_) => words.push(tok), // `foo.bar` that is no file: part of the query
        }
    }
    if !words.is_empty() {
        if terms.len() == 1 {
            terms[0] = format!("{} {}", terms[0], words.join(" "));
        } else {
            terms.extend(words);
        }
    }
    Ok(scopes)
}

fn collect_terms(params: &Value) -> Vec<String> {
    if let Some(query) = params.get("query").and_then(|v| v.as_str()) {
        let q = query.trim();
        if !q.is_empty() {
            return vec![q.to_string()];
        }
    }
    let mut terms = Vec::new();
    if let Some(args) = params.get("args").and_then(|v| v.as_array()) {
        for arg in args {
            if let Some(s) = arg.as_str() {
                if !s.trim().is_empty() {
                    terms.push(s.trim().to_string());
                }
            }
        }
    }
    terms
}

fn bool_param(params: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter().find_map(|k| params.get(*k).and_then(|v| v.as_bool()))
}

fn u64_param(params: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|k| params.get(*k).and_then(|v| v.as_u64()))
}

/// A string ("kt,cpp") or an array (["kt", "cpp"]) of extensions.
fn list_param(params: &Value, keys: &[&str]) -> Vec<String> {
    let raw: Vec<String> = keys
        .iter()
        .find_map(|k| params.get(*k))
        .map(|v| match v {
            Value::String(s) => s.split(',').map(|p| p.to_string()).collect(),
            Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    raw.iter()
        .map(|s| s.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect()
}

fn relative(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => path.to_string_lossy().replace('\\', "/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn terms(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn alternation_in_a_single_term_is_treated_as_regex() {
        let re = build_regex(&terms(&[r"WorkManager|Worker\b"]), None, false).unwrap();
        assert!(re.is_match(b"val w = WorkManager.getInstance()"));
        assert!(re.is_match(b"class SongWorker : Worker"));
        assert!(!re.is_match(b"class Workers"));
    }

    #[test]
    fn plain_text_and_symbols_stay_literal() {
        let re = build_regex(&terms(&["foo("]), None, false).unwrap();
        assert!(re.is_match(b"call foo(1)"));
        let sym = build_regex(&terms(&["Song"]), None, true).unwrap();
        assert!(sym.is_match(b"Song x") && !sym.is_match(b"SongList") && !sym.is_match(b"song"));
    }

    #[test]
    fn invalid_auto_detected_regex_falls_back_to_literal_but_explicit_errors() {
        assert!(build_regex(&terms(&["a|(b"]), None, false).is_ok());
        assert!(build_regex(&terms(&["a|(b"]), Some(true), false).is_err());
    }

    #[test]
    fn scan_reports_line_numbers_context_and_one_hit_per_line() {
        let dir = std::env::temp_dir().join(format!("mysid-search-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.kt");
        std::fs::write(&file, "one\ntwo needle needle\r\nthree\nfour\nneedle five\n").unwrap();

        let re = build_regex(&terms(&["needle"]), None, false).unwrap();
        let opts = Options { max_results: 10, context: 1, per_file: 0, types: vec![], group: false, prose: false, vendor: false, count: false, offset: 0 };
        let hits = scan_file(&file, &re, &opts).unwrap();

        assert_eq!(hits.len(), 2, "two matches on one line count once");
        assert_eq!((hits[0].line, hits[0].text.as_str()), (2, "two needle needle"));
        assert_eq!(hits[0].before, vec![(1, "one".to_string())]);
        assert_eq!(hits[0].after, vec![(3, "three".to_string())]);
        assert_eq!(hits[1].line, 5);
        assert!(hits[1].after.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn anchors_match_at_every_line_not_just_the_start_of_the_file() {
        let re = build_regex(&terms(&["^class"]), None, false).unwrap();
        assert!(re.is_match(b"package x

class Foo {
}
"));
        let end = build_regex(&terms(&["\\{$"]), Some(true), false).unwrap();
        assert!(end.is_match(b"fun a() {
  x()
}
"));
    }

    #[test]
    fn extra_words_that_are_files_become_scopes_and_other_words_stay_in_the_query() {
        let dir = std::env::temp_dir().join(format!("mysid-scope-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("A.kt"), "class A\n").unwrap();
        let root = dir.to_string_lossy().to_string();

        let mut t = vec!["foo".to_string()];
        let params = json!({ "extra": ["src/A.kt", "bar"] });
        let scopes = extract_scopes(&params, &dir, &root, &mut t).unwrap();
        assert_eq!(scopes, vec![dir.join("src/A.kt")]);
        assert_eq!(t, vec!["foo bar".to_string()], "non-path words stay part of the query");

        let missing = json!({ "extra": ["no/such/file.kt"] });
        assert!(extract_scopes(&missing, &dir, &root, &mut vec!["x".to_string()]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn counts_per_file_and_pages_through_a_long_result() {
        let dir = std::env::temp_dir().join(format!("mysid_search_count_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("a.kt"), (0..40).map(|i| format!("val hit_{} = 1\n", i)).collect::<String>()).unwrap();
        std::fs::write(dir.join("src").join("b.kt"), "val hit_x = 1\nval hit_y = 2\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let run = |extra: serde_json::Value| {
            let mut p = serde_json::json!({ "project_path": root, "workspace_root": root, "query": "hit_", "regex": true });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            execute_search(&p, Some(&root))
        };
        // A max above the old 25 is honoured.
        let all = run(serde_json::json!({ "max": 100, "group": true }));
        assert_eq!(all.matches("hit_").count(), 42, "{}", all);
        // Per-file counts, most first.
        let counts = run(serde_json::json!({ "count": true }));
        assert!(counts.starts_with("42 match line(s) in 2 file(s):"), "{}", counts);
        assert!(counts.find("a.kt").unwrap() < counts.find("b.kt").unwrap() && counts.contains("    40  src/a.kt"), "{}", counts);
        // Pages: the second page starts where the first stopped.
        let first = run(serde_json::json!({ "max": 30 }));
        assert!(first.contains("showing 1-30 of 42") && first.contains("--offset 30"), "{}", first);
        let second = run(serde_json::json!({ "max": 30, "offset": 30 }));
        assert!(second.contains("hit_30") && !second.contains("hit_29:") && !second.contains("... showing"), "{}", second);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn copied_libraries_are_skipped_unless_asked_for() {
        let dir = std::env::temp_dir().join(format!("mysid_search_vendor_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("external").join("opus")).unwrap();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src").join("a.cpp"), "int needle = 1;\n").unwrap();
        std::fs::write(dir.join("external").join("opus").join("b.c"), "int needle = 2;\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let run = |extra: serde_json::Value| {
            let mut p = serde_json::json!({ "project_path": root, "workspace_root": root, "query": "needle" });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            execute_search(&p, Some(&root))
        };
        let out = run(serde_json::json!({}));
        assert!(out.contains("a.cpp") && !out.contains("b.c"), "{}", out);
        assert!(run(serde_json::json!({ "vendor": true })).contains("b.c"));
        // Naming a folder inside external/ is asking for it.
        assert!(run(serde_json::json!({ "extra": ["external/opus"] })).contains("b.c"));
        // Nothing found: the answer says external/ was not searched.
        let none = execute_search(&serde_json::json!({ "project_path": root, "workspace_root": root, "query": "zzz_nothing" }), Some(&root));
        assert!(none.contains("--vendor"), "{}", none);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn grep_style_alternation_and_prose_files() {
        let dir = std::env::temp_dir().join(format!("mysid_search_alt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.kt"), "val alpha = 1\nval beta = 2\nval gamma = 3\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "alpha in prose\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let run = |q: &str, extra: serde_json::Value| {
            let mut p = serde_json::json!({ "project_path": root, "workspace_root": root, "query": q });
            for (k, v) in extra.as_object().unwrap() {
                p[k] = v.clone();
            }
            execute_search(&p, Some(&root))
        };
        // `\|` is alternation, as in grep.
        let out = run("alpha\\|gamma", serde_json::json!({ "regex": true }));
        assert!(out.contains("a.kt:1") && out.contains("a.kt:3") && !out.contains("a.kt:2"), "{}", out);
        // Prose is left out by default, with a hint; --all (or --types txt) includes it.
        let none = run("prose", serde_json::json!({}));
        assert!(none.starts_with("No matches found.") && none.contains("--all"), "{}", none);
        assert!(run("prose", serde_json::json!({ "all": true })).contains("notes.txt:1"));
        assert!(run("prose", serde_json::json!({ "types": "txt" })).contains("notes.txt:1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_param_accepts_string_or_array() {
        let a = serde_json::json!({ "types": "KT, .cpp" });
        let b = serde_json::json!({ "file_types": ["kt", ".cpp"] });
        assert_eq!(list_param(&a, &["file_types", "types"]), vec!["kt", "cpp"]);
        assert_eq!(list_param(&b, &["file_types", "types"]), vec!["kt", "cpp"]);
    }
}
