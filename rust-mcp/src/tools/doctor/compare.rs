//! `mysid doctor compare <old> <new>`: two copies of a project, side by side.

use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Folders of generated or downloaded files: left out unless `--everything` (their size is still reported).
pub(super) const GENERATED: &[&str] = &[".git", "node_modules", ".gradle", "build", ".cxx", "target", ".mysid", ".idea", ".dart_tool", "__pycache__", ".next", ".vs", "pods", "intermediates"];
/// Text files up to this size are read whole for the line-ending check and the line counts.
const TEXT_LIMIT: u64 = 8 * 1024 * 1024;
/// A modification time must differ by more than this to call one side newer (copies shift times a little).
const NEWER_SECS: u64 = 2;

#[derive(Clone)]
struct FileInfo {
    size: u64,
    mtime: Option<SystemTime>,
}

#[derive(Default)]
struct Tree {
    files: BTreeMap<String, FileInfo>,
    dirs: usize,
    /// Left-out generated folders: name -> (files, bytes).
    skipped: BTreeMap<String, (usize, u64)>,
    links: usize,
}

fn rel_join(rel: &str, name: &str) -> String {
    if rel.is_empty() { name.to_string() } else { format!("{}/{}", rel, name) }
}

fn count_tree(root: &Path) -> (usize, u64) {
    let (mut files, mut bytes) = (0usize, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(e.path());
            } else {
                files += 1;
                bytes += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    (files, bytes)
}

fn walk(root: &Path, everything: bool) -> Tree {
    let mut t = Tree::default();
    let mut stack: Vec<(PathBuf, String)> = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_symlink() {
                t.links += 1;
                continue;
            }
            if ft.is_dir() {
                if !everything && GENERATED.contains(&name.to_lowercase().as_str()) {
                    let (n, b) = count_tree(&e.path());
                    let s = t.skipped.entry(name.to_lowercase()).or_insert((0, 0));
                    s.0 += n;
                    s.1 += b;
                    continue;
                }
                t.dirs += 1;
                stack.push((e.path(), rel_join(&rel, &name)));
            } else if let Ok(m) = e.metadata() {
                t.files.insert(rel_join(&rel, &name), FileInfo { size: m.len(), mtime: m.modified().ok() });
            }
        }
    }
    t
}

#[derive(Debug, PartialEq)]
enum Kind {
    Same,
    /// Only the line endings differ (CRLF / LF).
    EolOnly,
    Changed,
}

struct Outcome {
    rel: String,
    kind: Kind,
    a: FileInfo,
    b: FileInfo,
    /// Line counts when both sides are text of a readable size.
    lines: Option<(usize, usize)>,
}

fn looks_text(buf: &[u8]) -> bool {
    !buf[..buf.len().min(8192)].contains(&0)
}

fn line_count(buf: &[u8]) -> usize {
    buf.iter().filter(|&&c| c == b'\n').count() + usize::from(!buf.is_empty() && buf.last() != Some(&b'\n'))
}

fn same_bytes(a: &Path, b: &Path) -> bool {
    let (Ok(mut fa), Ok(mut fb)) = (fs::File::open(a), fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let na = read_full(&mut fa, &mut ba);
        let nb = read_full(&mut fb, &mut bb);
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

fn read_full(f: &mut fs::File, buf: &mut [u8]) -> usize {
    let mut n = 0;
    while n < buf.len() {
        match f.read(&mut buf[n..]) {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n
}

fn compare_one(rel: &str, pa: &Path, pb: &Path, a: &FileInfo, b: &FileInfo) -> Outcome {
    let mk = |kind: Kind, lines: Option<(usize, usize)>| Outcome { rel: rel.to_string(), kind, a: a.clone(), b: b.clone(), lines };
    let readable = a.size <= TEXT_LIMIT && b.size <= TEXT_LIMIT;
    if a.size == b.size {
        if same_bytes(pa, pb) {
            return mk(Kind::Same, None);
        }
    }
    if readable {
        if let (Ok(da), Ok(db)) = (fs::read(pa), fs::read(pb)) {
            if looks_text(&da) && looks_text(&db) {
                let strip = |d: &[u8]| d.iter().copied().filter(|&c| c != b'\r').collect::<Vec<u8>>();
                let (sa, sb) = (strip(&da), strip(&db));
                if sa == sb {
                    return mk(Kind::EolOnly, None);
                }
                return mk(Kind::Changed, Some((line_count(&da), line_count(&db))));
            }
        }
    }
    mk(Kind::Changed, None)
}

pub(super) fn human(bytes: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{} B", bytes) } else { format!("{:.1} {}", v, U[i]) }
}

pub(super) fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn newer(a: &FileInfo, b: &FileInfo) -> Option<char> {
    let (ta, tb) = (a.mtime?, b.mtime?);
    if let Ok(d) = tb.duration_since(ta) {
        if d.as_secs() > NEWER_SECS {
            return Some('B');
        }
    }
    if let Ok(d) = ta.duration_since(tb) {
        if d.as_secs() > NEWER_SECS {
            return Some('A');
        }
    }
    None
}

fn top_of(rel: &str) -> String {
    match rel.split_once('/') {
        Some((first, _)) => format!("{}/", first),
        None => "(root files)".to_string(),
    }
}

/// Files that exist on one side only, shown as folders when the whole folder is on that side only.
fn collapse(only: &[(String, u64)], common_dirs: &HashSet<String>) -> Vec<(String, usize, u64)> {
    let mut groups: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (rel, size) in only {
        let parts: Vec<&str> = rel.split('/').collect();
        let mut key = rel.clone();
        for i in 1..parts.len() {
            let dir = parts[..i].join("/");
            if !common_dirs.contains(&dir) {
                key = format!("{}/", dir);
                break;
            }
        }
        let g = groups.entry(key).or_insert((0, 0));
        g.0 += 1;
        g.1 += size;
    }
    groups.into_iter().map(|(k, (n, b))| (k, n, b)).collect()
}

fn flag(params: &Value, keys: &[&str]) -> bool {
    keys.iter().any(|k| match params.get(*k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !matches!(s.trim().to_lowercase().as_str(), "false" | "0" | "no" | "off"),
        _ => false,
    })
}

fn resolve(p: &str, base: &Path) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() { path.to_path_buf() } else { base.join(path) }
}

fn label_of(p: &Path, other: &Path) -> String {
    let name = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.to_string_lossy().to_string());
    if name(p) == name(other) { String::new() } else { name(p) }
}

pub fn run(params: &Value, default_root: Option<&str>) -> String {
    let fail = |m: String| json!({ "success": false, "error": m }).to_string();
    let a_arg = params.get("a").and_then(|v| v.as_str());
    let b_arg = params.get("b").and_then(|v| v.as_str());
    let (Some(a_arg), Some(b_arg)) = (a_arg, b_arg) else {
        return fail("doctor compare needs two folders: mysid doctor compare E:\\proj\\v42 E:\\proj\\v43   (the first is the old one)".to_string());
    };
    let base = params.get("workspace_root").and_then(|v| v.as_str()).or(default_root).map(PathBuf::from).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let (pa, pb) = (resolve(a_arg, &base), resolve(b_arg, &base));
    for (name, p) in [("first", &pa), ("second", &pb)] {
        if !p.is_dir() {
            return fail(format!("the {} folder does not exist or is not a folder: {}", name, p.display()));
        }
    }
    if let (Ok(ca), Ok(cb)) = (pa.canonicalize(), pb.canonicalize()) {
        if ca == cb {
            return fail("both arguments are the same folder".to_string());
        }
    }
    let everything = flag(params, &["everything", "include_generated"]);
    let brief = flag(params, &["brief"]);
    let max = params.get("max").and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(25).clamp(1, 500) as usize;

    let (ta, tb) = std::thread::scope(|s| {
        let h = s.spawn(|| walk(&pa, everything));
        let b = walk(&pb, everything);
        (h.join().unwrap_or_default(), b)
    });

    // Common files, compared on all cores.
    let common: Vec<(&String, &FileInfo, &FileInfo)> = ta.files.iter().filter_map(|(k, a)| tb.files.get(k).map(|b| (k, a, b))).collect();
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16);
    let chunk = common.len().div_ceil(workers).max(1);
    let outcomes: Vec<Outcome> = std::thread::scope(|s| {
        let handles: Vec<_> = common
            .chunks(chunk)
            .map(|part| {
                let (pa, pb) = (&pa, &pb);
                s.spawn(move || part.iter().map(|(rel, a, b)| compare_one(rel, &pa.join(rel), &pb.join(rel), a, b)).collect::<Vec<Outcome>>())
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
    });

    let same = outcomes.iter().filter(|o| o.kind == Kind::Same).count();
    let eol: Vec<&Outcome> = outcomes.iter().filter(|o| o.kind == Kind::EolOnly).collect();
    let mut changed: Vec<&Outcome> = outcomes.iter().filter(|o| o.kind == Kind::Changed).collect();
    changed.sort_by(|x, y| (y.b.size as i64 - y.a.size as i64).abs().cmp(&(x.b.size as i64 - x.a.size as i64).abs()).then(x.rel.cmp(&y.rel)));
    let b_newer = changed.iter().filter(|o| newer(&o.a, &o.b) == Some('B')).count();
    let a_newer: Vec<&&Outcome> = changed.iter().filter(|o| newer(&o.a, &o.b) == Some('A')).collect();
    let mut only_a: Vec<(String, u64)> = ta.files.iter().filter(|(k, _)| !tb.files.contains_key(*k)).map(|(k, v)| (k.clone(), v.size)).collect();
    let mut only_b: Vec<(String, u64)> = tb.files.iter().filter(|(k, _)| !ta.files.contains_key(*k)).map(|(k, v)| (k.clone(), v.size)).collect();
    // A file missing from one place and present elsewhere with the same bytes was moved or renamed, not lost.
    let mut moved: Vec<(String, String, u64)> = Vec::new();
    {
        let mut by_size: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
        for (i, (_, size)) in only_b.iter().enumerate() {
            if *size > 0 {
                by_size.entry(*size).or_default().push(i);
            }
        }
        let mut used_b: HashSet<usize> = HashSet::new();
        let mut used_a: HashSet<usize> = HashSet::new();
        for (ia, (ra, size)) in only_a.iter().enumerate() {
            let Some(cands) = by_size.get(size) else { continue };
            for &ib in cands {
                if !used_b.contains(&ib) && same_bytes(&pa.join(ra), &pb.join(&only_b[ib].0)) {
                    used_b.insert(ib);
                    used_a.insert(ia);
                    moved.push((ra.clone(), only_b[ib].0.clone(), *size));
                    break;
                }
            }
        }
        let keep = |v: &mut Vec<(String, u64)>, used: &HashSet<usize>| {
            let mut i = 0;
            v.retain(|_| {
                i += 1;
                !used.contains(&(i - 1))
            });
        };
        keep(&mut only_a, &used_a);
        keep(&mut only_b, &used_b);
    }

    // Folders that hold at least one common file: a one-sided file inside one is listed alone, outside any is listed as a folder.
    let mut common_dirs: HashSet<String> = HashSet::new();
    for o in &outcomes {
        let parts: Vec<&str> = o.rel.split('/').collect();
        for i in 1..parts.len() {
            common_dirs.insert(parts[..i].join("/"));
        }
    }

    let (la, lb) = (label_of(&pa, &pb), label_of(&pb, &pa));
    let (la, lb) = (if la.is_empty() { "A".to_string() } else { la }, if lb.is_empty() { "B".to_string() } else { lb });
    let bytes = |t: &Tree| t.files.values().map(|f| f.size).sum::<u64>();
    let (sa, sb) = (bytes(&ta), bytes(&tb));
    let mut out: Vec<String> = Vec::new();
    out.push(format!("compare  {} = {}", la, pa.display()));
    out.push(format!("         {} = {}", lb, pb.display()));
    out.push(String::new());
    out.push(format!("{:<22}{:>16}{:>16}", "", format!("{} (old)", la), format!("{} (new)", lb)));
    out.push(format!("{:<22}{:>16}{:>16}", "files", thousands(ta.files.len()), thousands(tb.files.len())));
    out.push(format!("{:<22}{:>16}{:>16}", "folders", thousands(ta.dirs), thousands(tb.dirs)));
    out.push(format!("{:<22}{:>16}{:>16}", "size", human(sa), human(sb)));
    out.push(String::new());
    out.push(format!("{:<22}{:>8}", "identical", thousands(same)));
    out.push(format!(
        "{:<22}{:>8}{}",
        "changed",
        thousands(changed.len()),
        if changed.is_empty() { String::new() } else { format!("   ({} newer in {}, {} newer in {}, {} within {}s)", b_newer, lb, a_newer.len(), la, changed.len() - b_newer - a_newer.len(), NEWER_SECS) }
    ));
    out.push(format!("{:<22}{:>8}{}", "line endings only", thousands(eol.len()), if eol.is_empty() { "" } else { "   (CRLF vs LF, same text)" }));
    out.push(format!("{:<22}{:>8}{}", format!("only in {} (old)", la), thousands(only_a.len()), if only_a.is_empty() { "" } else { "   <- would be lost" }));
    out.push(format!("{:<22}{:>8}", format!("only in {} (new)", lb), thousands(only_b.len())));
    out.push(format!("{:<22}{:>8}{}", "moved / renamed", thousands(moved.len()), if moved.is_empty() { "" } else { "   (same bytes, different path: not lost)" }));
    let left_out: Vec<String> = ta
        .skipped
        .keys()
        .chain(tb.skipped.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|k| {
            let f = |t: &Tree| t.skipped.get(k).map_or("-".to_string(), |(n, b)| format!("{} files {}", thousands(*n), human(*b)));
            format!("  {}: {} {} | {} {}", k, la, f(&ta), lb, f(&tb))
        })
        .collect();
    if !left_out.is_empty() {
        out.push(String::new());
        out.push("not compared (generated folders; --everything includes them):".to_string());
        out.extend(left_out);
    }
    if ta.links + tb.links > 0 {
        out.push(format!("symbolic links skipped: {} in {}, {} in {}", ta.links, la, tb.links, lb));
    }

    // The verdict on deleting the old folder.
    out.push(String::new());
    if !only_a.is_empty() {
        out.push(format!("VERDICT: NOT SAFE to delete {} - {} file(s) exist only there ({}). Copy them first.", la, only_a.len(), human(only_a.iter().map(|x| x.1).sum())));
    } else if !a_newer.is_empty() {
        out.push(format!("VERDICT: CHECK before deleting {} - {} file(s) there are newer than the copy in {} (listed below).", la, a_newer.len(), lb));
    } else {
        out.push(format!(
            "VERDICT: nothing in {} is missing from {} and no {} file is newer: deleting {} loses no file and no newer edit.{}",
            la,
            lb,
            la,
            la,
            if changed.is_empty() { String::new() } else { format!(" {} file(s) differ - {} has the newer version of each.", changed.len(), lb) }
        ));
    }

    // Per top-level folder.
    #[derive(Default)]
    struct Row {
        fa: usize,
        fb: usize,
        same: usize,
        changed: usize,
        oa: usize,
        ob: usize,
    }
    let mut rows: BTreeMap<String, Row> = BTreeMap::new();
    for k in ta.files.keys() {
        rows.entry(top_of(k)).or_default().fa += 1;
    }
    for k in tb.files.keys() {
        rows.entry(top_of(k)).or_default().fb += 1;
    }
    for o in &outcomes {
        let r = rows.entry(top_of(&o.rel)).or_default();
        match o.kind {
            Kind::Same => r.same += 1,
            _ => r.changed += 1,
        }
    }
    for (k, _) in &only_a {
        rows.entry(top_of(k)).or_default().oa += 1;
    }
    for (k, _) in &only_b {
        rows.entry(top_of(k)).or_default().ob += 1;
    }
    let mut table: Vec<(&String, &Row)> = rows.iter().collect();
    table.sort_by_key(|(k, r)| (std::cmp::Reverse(r.changed + r.oa + r.ob), (*k).clone()));
    out.push(String::new());
    out.push(format!("by top-level folder:   {:>7}{:>7}{:>7}{:>8}{:>8}{:>8}", la.chars().take(6).collect::<String>(), lb.chars().take(6).collect::<String>(), "same", "differ", "only-A", "only-B"));
    for (k, r) in table.iter().take(if brief { 40 } else { 25 }) {
        out.push(format!("  {:<20}{:>7}{:>7}{:>7}{:>8}{:>8}{:>8}", k.chars().take(20).collect::<String>(), r.fa, r.fb, r.same, r.changed, r.oa, r.ob));
    }
    if table.len() > 25 && !brief {
        out.push(format!("  ... {} more folder(s)", table.len() - 25));
    }

    if !brief {
        let list = |out: &mut Vec<String>, title: String, lines: Vec<String>| {
            if lines.is_empty() {
                return;
            }
            out.push(String::new());
            out.push(format!("{} ({}):", title, lines.len()));
            let n = lines.len();
            out.extend(lines.into_iter().take(max).map(|l| format!("  {}", l)));
            if n > max {
                out.push(format!("  ... +{} more (--max {} shows more)", n - max, n.min(500)));
            }
        };
        list(
            &mut out,
            format!("only in {} (old) - would be lost", la),
            collapse(&only_a, &common_dirs).into_iter().map(|(k, n, b)| if k.ends_with('/') { format!("{}  ({} files, {})", k, n, human(b)) } else { format!("{}  ({})", k, human(b)) }).collect(),
        );
        list(
            &mut out,
            format!("changed - largest size change first"),
            changed
                .iter()
                .map(|o| {
                    let delta = o.b.size as i64 - o.a.size as i64;
                    format!(
                        "{}  {} -> {} ({}{}){}{}",
                        o.rel,
                        human(o.a.size),
                        human(o.b.size),
                        if delta >= 0 { "+" } else { "-" },
                        human(delta.unsigned_abs()),
                        o.lines.map_or(String::new(), |(x, y)| format!("  lines {} -> {}", x, y)),
                        match newer(&o.a, &o.b) {
                            Some('A') => format!("  [newer in {}]", la),
                            Some('B') => format!("  [newer in {}]", lb),
                            _ => String::new(),
                        }
                    )
                })
                .collect(),
        );
        list(&mut out, "line endings only".to_string(), eol.iter().map(|o| o.rel.clone()).collect());
        list(&mut out, "moved / renamed (same content)".to_string(), moved.iter().map(|(x, y, s)| format!("{} -> {}  ({})", x, y, human(*s))).collect());
        list(
            &mut out,
            format!("only in {} (new)", lb),
            collapse(&only_b, &common_dirs).into_iter().map(|(k, n, b)| if k.ends_with('/') { format!("{}  ({} files, {})", k, n, human(b)) } else { format!("{}  ({})", k, human(b)) }).collect(),
        );
    }
    json!({ "success": true, "output": out.join("\n") }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn put(root: &Path, rel: &str, content: &[u8]) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("mysid_doctor_{}_{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let (a, b) = (base.join("v42"), base.join("v43"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        (base, a, b)
    }

    fn out_of(a: &Path, b: &Path, extra: Value) -> String {
        let mut p = json!({ "sub": "compare", "a": a.to_string_lossy(), "b": b.to_string_lossy() });
        for (k, v) in extra.as_object().unwrap() {
            p[k] = v.clone();
        }
        let r: Value = serde_json::from_str(&run(&p, None)).unwrap();
        r["output"].as_str().or_else(|| r["error"].as_str()).unwrap().to_string()
    }

    #[test]
    fn the_report_counts_identical_changed_line_endings_and_one_sided_files() {
        let (base, a, b) = fixture("report");
        put(&a, "src/same.kt", b"fun a() {}\n");
        put(&b, "src/same.kt", b"fun a() {}\n");
        put(&a, "src/edited.kt", b"one\ntwo\n");
        put(&b, "src/edited.kt", b"one\ntwo\nthree\nfour\n");
        put(&a, "src/crlf.kt", b"x\r\ny\r\n");
        put(&b, "src/crlf.kt", b"x\ny\n");
        put(&a, "src/gone.kt", b"removed in the new one\n");
        put(&a, "old_only/a.txt", b"1");
        put(&a, "old_only/b.txt", b"22");
        put(&b, "new_only/c.txt", b"333");
        put(&a, "src/Mover.kt", b"class Mover\n");
        put(&b, "src/impl/Mover.kt", b"class Mover\n");
        put(&a, "node_modules/x/index.js", b"module");
        put(&b, "node_modules/x/index.js", b"module2");
        let o = out_of(&a, &b, json!({}));
        assert!(o.contains("files") && o.contains("identical") && o.contains("line endings only"), "{}", o);
        let line = |name: &str| o.lines().find(|l| l.trim_start().starts_with(name)).unwrap_or_else(|| panic!("no '{}' line in\n{}", name, o)).to_string();
        assert!(line("identical").trim_end().ends_with('1'), "{}", line("identical"));
        assert!(line("changed").contains("1"), "{}", line("changed"));
        assert!(line("line endings only").contains("1"), "{}", line("line endings only"));
        assert!(line("moved / renamed").contains("1") && o.contains("src/Mover.kt -> src/impl/Mover.kt"), "{}", o);
        assert!(line("only in v42 (old)").contains("3"), "{}", line("only in v42 (old)"));
        assert!(line("only in v43 (new)").contains("1"), "{}", line("only in v43 (new)"));
        // Counts: A has 3 + 1 + 2 = 6 files outside node_modules? same, edited, crlf, gone, old_only x2 = 6.
        assert!(line("files").contains("7") && line("files").contains("5"), "{}", line("files"));
        assert!(o.contains("lines 2 -> 4"), "{}", o);
        // A folder that exists on one side only is listed once.
        assert!(o.contains("old_only/  (2 files") && o.contains("src/gone.kt"), "{}", o);
        assert!(o.contains("new_only/  (1 files"), "{}", o);
        assert!(o.contains("VERDICT: NOT SAFE to delete v42"), "{}", o);
        assert!(o.contains("not compared") && o.contains("node_modules"), "{}", o);
        // --everything compares them too.
        let all = out_of(&a, &b, json!({ "everything": true }));
        assert!(!all.contains("not compared") && all.contains("node_modules/x/index.js"), "{}", all);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn the_verdict_says_when_the_old_folder_can_go_and_when_a_file_there_is_newer() {
        let (base, a, b) = fixture("verdict");
        put(&a, "m.txt", b"old text\n");
        put(&b, "m.txt", b"new text, longer\n");
        put(&a, "k.txt", b"same\n");
        put(&b, "k.txt", b"same\n");
        // Make B's copy of m.txt clearly newer.
        let later = SystemTime::now() + Duration::from_secs(120);
        fs::File::options().write(true).open(b.join("m.txt")).unwrap().set_modified(later).unwrap();
        let o = out_of(&a, &b, json!({ "brief": true }));
        assert!(o.contains("VERDICT: nothing in v42 is missing from v43") && o.contains("loses no file"), "{}", o);
        assert!(!o.contains("changed - largest"), "brief has no lists: {}", o);
        // Now the old folder's copy is the newer one.
        fs::File::options().write(true).open(a.join("m.txt")).unwrap().set_modified(SystemTime::now() + Duration::from_secs(600)).unwrap();
        let o = out_of(&a, &b, json!({}));
        assert!(o.contains("VERDICT: CHECK before deleting v42") && o.contains("[newer in v42]"), "{}", o);
        // Bad input.
        assert!(out_of(&a, &a, json!({})).contains("same folder"));
        assert!(out_of(&a, &base.join("nope"), json!({})).contains("does not exist"));
        let _ = fs::remove_dir_all(&base);
    }
}
