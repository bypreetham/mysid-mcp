//! Persistent code index.
//!
//! ```text
//! parsers -> extract -> domain model -> DAOs (SQLite, WAL: <workspace>/.mysid/index.db)
//!                                          \-> resolve (in-memory HashMaps) -> edges
//! ```
//!
//! The index is a cache of what the parsers see, kept current incrementally:
//! - only files whose mtime or size changed are re-parsed (`refresh`), or exactly the files named
//!   (`refresh_paths`, for a file watcher);
//! - call edges are re-resolved only for the symbols that could be affected: those in changed
//!   files, and those elsewhere that call a name defined or removed by the change.
//!
//! Consumers read through targeted queries (`candidates`, `neighborhood`, `members`, `has_path`),
//! so a lookup costs the same on a 300-file project and a 30,000-file one. `snapshot` loads everything
//! for the few views that need it (the workspace map).

pub mod dao;
mod extract;
pub mod model;
mod resolve;
pub(crate) mod scan;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use dao::{in_tx, Db, EdgeDao, FileDao, Result, SignatureDao, SymbolDao};
pub use dao::Outline;
use model::{FileId, EdgeKind};
pub use model::{Edge, File, Located, Symbol, SymbolId, SymbolKind};

const DB_FILE: &str = "index.db";

/// `MYSID_TIMING=1` prints where the time goes (to stderr), for diagnosing slow calls.
pub fn timing_enabled() -> bool {
    std::env::var_os("MYSID_TIMING").is_some()
}

/// When at least 1/N of the indexed files changed, re-resolving everything beats going symbol by symbol.
const FULL_RESOLVE_RATIO: usize = 4;
const FUZZY_LIMIT: usize = 8;

/// Everything the index knows, loaded in three queries.
pub struct Snapshot {
    pub files: Vec<File>,
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
}

/// A symbol set plus the CALLS edges among it: what a `graph` query needs, nothing more.
pub struct Neighborhood {
    pub symbols: Vec<Located>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

enum Change {
    Upsert(scan::ScannedFile),
    /// A file, or a directory (everything beneath it).
    Remove(String),
}

pub struct Index {
    db: Db,
}

fn remove_db_files(path: &Path) {
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        let _ = std::fs::remove_file(PathBuf::from(p));
    }
}

impl Index {
    /// Opens (creating if needed) the workspace index and brings it up to date.
    /// Never fails: if `.mysid/index.db` is unusable it is rebuilt, and as a last resort the
    /// index lives in memory for this run.
    pub fn open(root: &Path) -> Index {
        if let Ok(dir) = crate::tools::build::log::ensure_dir(root) {
            let path = dir.join(DB_FILE);
            for attempt in 0..2 {
                if let Ok(db) = Db::open(&path) {
                    let index = Index { db };
                    if index.refresh(root).is_ok() {
                        return index;
                    }
                }
                if attempt == 0 {
                    // Corrupt or incompatible file: it is only a cache, start over.
                    remove_db_files(&path);
                }
            }
        }
        let index = Index { db: Db::in_memory().expect("in-memory sqlite") };
        let _ = index.refresh(root);
        index
    }

    /// Re-indexes new/changed files and drops deleted ones by walking the workspace.
    /// A no-change refresh takes no write lock.
    pub fn refresh(&self, root: &Path) -> Result<RefreshStats> {
        let t0 = std::time::Instant::now();
        let scanned = scan::scan(root);
        let t_scan = t0.elapsed();
        let conn = &self.db.conn;

        let known = FileDao::new(conn).all()?;
        let known_by_path: HashMap<&str, &File> = known.iter().map(|f| (f.path.as_str(), f)).collect();
        let on_disk: HashSet<&str> = scanned.iter().map(|s| s.rel.as_str()).collect();

        let dirty = scanned
            .iter()
            .any(|s| known_by_path.get(s.rel.as_str()).map_or(true, |f| f.mtime != s.mtime || f.size != s.size))
            || known.iter().any(|f| !on_disk.contains(f.path.as_str()));
        if timing_enabled() {
            eprintln!("[timing] file walk+stat: {:?} ({} files), diff vs index: {:?}", t_scan, scanned.len(), t0.elapsed() - t_scan);
        }
        if !dirty {
            return Ok(RefreshStats { unchanged: scanned.len(), ..Default::default() });
        }

        let mut changes: Vec<Change> = known
            .iter()
            .filter(|f| !on_disk.contains(f.path.as_str()))
            .map(|f| Change::Remove(f.path.clone()))
            .collect();
        changes.extend(scanned.into_iter().map(Change::Upsert));
        in_tx(conn, |c| apply_changes(c, &changes))
    }

    /// Re-indexes exactly these paths (files or directories, absolute or workspace-relative):
    /// parsed again if they exist and changed, dropped if they are gone. Meant for a file watcher.
    pub fn refresh_paths(&self, root: &Path, paths: &[PathBuf]) -> Result<RefreshStats> {
        let mut seen: HashSet<String> = HashSet::new();
        let mut changes = Vec::new();
        for p in paths {
            let abs = if p.is_absolute() { p.clone() } else { root.join(p) };
            let Ok(rel) = abs.strip_prefix(root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            if !seen.insert(rel.clone()) {
                continue;
            }
            match scan::scan_one(root, &abs) {
                Some(file) => changes.push(Change::Upsert(file)),
                None => changes.push(Change::Remove(rel)),
            }
        }
        in_tx(&self.db.conn, |c| apply_changes(c, &changes))
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let conn = &self.db.conn;
        Ok(Snapshot {
            files: FileDao::new(conn).all()?,
            symbols: SymbolDao::new(conn).all()?,
            edges: EdgeDao::new(conn).all()?,
        })
    }

    // ── targeted reads ──────────────────────────────────────────────────────────────────────

    /// Symbols a user query could mean: exact name (any case), `Class.method`, `path:Name`, and
    /// only when none of those match, names containing the text.
    pub fn candidates(&self, query: &str) -> Result<Vec<Located>> {
        let q = query.trim().replace("::", ".").to_lowercase();
        let symbols = SymbolDao::new(&self.db.conn);

        let mut found = symbols.located_by_name(&q)?;
        if found.is_empty() {
            if let Some((owner, name)) = q.rsplit_once('.') {
                found = symbols.located_by_owner_name(owner, name)?;
            }
        }
        if found.is_empty() {
            if let Some((path, sym)) = q.rsplit_once(':') {
                found = match sym.rsplit_once('.') {
                    Some((owner, name)) => symbols
                        .located_by_path_name(path, name)?
                        .into_iter()
                        .filter(|l| l.symbol.owner.as_deref().map_or(false, |o| o.eq_ignore_ascii_case(owner)))
                        .collect(),
                    None => symbols.located_by_path_name(path, sym)?,
                };
            }
        }
        if found.is_empty() {
            found = symbols.located_fuzzy(&q, FUZZY_LIMIT)?;
        }
        Ok(found)
    }

    /// Indexed files named `name` (a bare file name or a path), case-insensitively.
    pub fn files_named(&self, name: &str) -> Result<Vec<File>> {
        FileDao::new(&self.db.conn).named(name.trim().replace('\\', "/").trim_matches('/'))
    }

    /// Every symbol of a file, in source order.
    pub fn symbols_in_file(&self, file_id: FileId) -> Result<Vec<Located>> {
        let dao = SymbolDao::new(&self.db.conn);
        let ids: Vec<SymbolId> = dao.find_by_file(file_id)?.iter().map(|s| s.id).collect();
        let mut located = dao.located_by_ids(&ids)?;
        located.sort_by_key(|l| l.symbol.start_line);
        Ok(located)
    }

    /// Methods of a class symbol, in source order.
    pub fn members(&self, class: &Located) -> Result<Vec<Located>> {
        SymbolDao::new(&self.db.conn).members(class.symbol.file_id, &class.symbol.name)
    }

    /// Opens the index WITHOUT walking the workspace: for callers that refresh just what they need
    /// (`refresh_paths`) and would otherwise pay for a whole-tree walk on every call.
    pub fn open_lazy(root: &Path) -> Index {
        if let Ok(dir) = crate::tools::build::log::ensure_dir(root) {
            if let Ok(db) = Db::open(&dir.join(DB_FILE)) {
                return Index { db };
            }
        }
        Index { db: Db::in_memory().expect("in-memory sqlite") }
    }

    /// Outlines (types and method signatures) of an indexed file, or of every indexed file under a directory.
    pub fn outlines(&self, target: &str) -> Result<Vec<Outline>> {
        SignatureDao::new(&self.db.conn).outlines(target.trim().replace('\\', "/").trim_matches('/'))
    }

    /// True when `prefix` names an indexed file or a directory containing indexed files.
    pub fn has_path(&self, prefix: &str) -> Result<bool> {
        FileDao::new(&self.db.conn).has_path(prefix.trim().replace('\\', "/").trim_matches('/'))
    }

    /// The symbols within `depth` calls of the seeds, in both directions, with the edges that
    /// connect them. Nodes at the last hop are leaves: their own edges are not loaded.
    pub fn neighborhood(&self, seeds: &[SymbolId], depth: usize) -> Result<Neighborhood> {
        let edge_dao = EdgeDao::new(&self.db.conn);
        let mut all: HashSet<SymbolId> = seeds.iter().copied().collect();
        let (mut seen_down, mut seen_up) = (all.clone(), all.clone());
        let (mut down, mut up) = (seeds.to_vec(), seeds.to_vec());
        let mut edge_set: HashSet<(SymbolId, SymbolId)> = HashSet::new();
        let mut edges: Vec<Edge> = Vec::new();

        for _ in 0..depth {
            let mut next_down = Vec::new();
            for e in edge_dao.callees_of(&down)? {
                if edge_set.insert((e.from, e.to)) {
                    edges.push(e);
                }
                if seen_down.insert(e.to) {
                    next_down.push(e.to);
                }
                all.insert(e.to);
            }
            let mut next_up = Vec::new();
            for e in edge_dao.callers_of(&up)? {
                if edge_set.insert((e.from, e.to)) {
                    edges.push(e);
                }
                if seen_up.insert(e.from) {
                    next_up.push(e.from);
                }
                all.insert(e.from);
            }
            if next_down.is_empty() && next_up.is_empty() {
                break;
            }
            down = next_down;
            up = next_up;
        }

        let ids: Vec<SymbolId> = all.into_iter().collect();
        let symbols = SymbolDao::new(&self.db.conn).located_by_ids(&ids)?;
        let edges = drop_class_edges_beside_member_calls(edges, &symbols);
        Ok(Neighborhood { symbols, edges })
    }

    #[cfg(test)]
    fn in_memory() -> Index {
        Index { db: Db::in_memory().unwrap() }
    }
}

/// `new Foo(...)` links a caller to the class `Foo`; when the same caller also calls methods of `Foo`, the class edge
/// only repeats what those edges say, so it is left out of a graph view.
fn drop_class_edges_beside_member_calls(edges: Vec<Edge>, symbols: &[Located]) -> Vec<Edge> {
    let by_id: HashMap<SymbolId, &Located> = symbols.iter().map(|l| (l.symbol.id, l)).collect();
    let mut owners_called: HashMap<SymbolId, HashSet<&str>> = HashMap::new();
    for e in &edges {
        if let Some(owner) = by_id.get(&e.to).and_then(|l| l.symbol.owner.as_deref()) {
            owners_called.entry(e.from).or_default().insert(owner);
        }
    }
    edges
        .into_iter()
        .filter(|e| match by_id.get(&e.to) {
            Some(l) if l.symbol.kind == SymbolKind::Class => !owners_called.get(&e.from).map_or(false, |o| o.contains(l.symbol.name.as_str())),
            _ => true,
        })
        .collect()
}

/// Applies file changes in the open transaction, then re-resolves the call edges they can affect.
fn apply_changes(c: &rusqlite::Connection, changes: &[Change]) -> Result<RefreshStats> {
    let files = FileDao::new(c);
    let symbols = SymbolDao::new(c);
    // Read inside the lock: another mysid process may have indexed in the meantime.
    let known: HashMap<String, File> = files.all()?.into_iter().map(|f| (f.path.clone(), f)).collect();
    let mut stats = RefreshStats::default();

    // Names whose meaning may change: everything the old versions defined and the new ones define.
    let mut affected: HashSet<String> = HashSet::new();
    let mut touched: Vec<SymbolId> = Vec::new();
    let mut removed: HashSet<String> = HashSet::new();

    for change in changes {
        match change {
            Change::Upsert(s) => {
                match known.get(&s.rel) {
                    Some(f) if f.mtime == s.mtime && f.size == s.size => {
                        stats.unchanged += 1;
                        continue;
                    }
                    Some(f) => {
                        affected.extend(symbols.names_in_file(f.id)?);
                        files.delete(f.id)?;
                        stats.updated += 1;
                    }
                    None => stats.added += 1,
                }

                let parsed = extract::parse_file(&s.path);
                let line_count = parsed.as_ref().map_or(0, |p| p.lines);
                let file_id = files.insert(&s.rel, &s.language, s.mtime, s.size, line_count)?;
                if let Some(p) = &parsed {
                    SignatureDao::new(c).insert_file(file_id, &p.types, &p.methods)?;
                }
                for sym in parsed.iter().flat_map(|p| p.symbols.iter()) {
                    let id = symbols.insert(file_id, sym)?;
                    touched.push(id);
                    affected.insert(sym.name.clone());
                    for (callee, dotted, recv_type) in &sym.calls {
                        symbols.insert_call(id, callee, *dotted, recv_type)?;
                    }
                }
            }
            Change::Remove(path) => {
                let dir_prefix = format!("{}/", path);
                for (p, f) in known.iter().filter(|(p, _)| *p == path || p.starts_with(&dir_prefix)) {
                    if removed.insert(p.clone()) {
                        affected.extend(symbols.names_in_file(f.id)?);
                        files.delete(f.id)?;
                        stats.removed += 1;
                    }
                }
            }
        }
    }

    let changed = stats.added + stats.updated + stats.removed;
    if changed > 0 {
        re_resolve(c, &touched, &affected, files.count()?, changed)?;
    }
    Ok(stats)
}

/// Brings the edges table back in line with the symbols after a change, then links native methods to the JNI
/// functions that implement them.
fn re_resolve(
    c: &rusqlite::Connection,
    touched: &[SymbolId],
    affected: &HashSet<String>,
    total_files: usize,
    changed_files: usize,
) -> Result<()> {
    re_resolve_calls(c, touched, affected, total_files, changed_files)?;
    link_jni(c)?;
    link_overrides(c)
}

/// Base class methods get a DISPATCH edge to the methods that override them (few overrides only).
fn link_overrides(c: &rusqlite::Connection) -> Result<()> {
    let symbols = SymbolDao::new(c);
    let edges = EdgeDao::new(c);
    edges.delete_kind(EdgeKind::Dispatch)?;
    let mut bases: HashMap<String, Vec<String>> = HashMap::new();
    for (class, list) in symbols.classes_with_bases()? {
        bases.entry(class).or_default().extend(list);
    }
    if bases.is_empty() {
        return Ok(());
    }
    for (from, to) in resolve::override_links(&bases, &symbols.methods()?, resolve::OVERRIDE_FANOUT) {
        edges.insert(&Edge { from, to, kind: EdgeKind::Dispatch })?;
    }
    Ok(())
}

/// `external` / `native` methods get an edge to the C/C++ function `Java_<package>_<Class>_<method>`.
fn link_jni(c: &rusqlite::Connection) -> Result<()> {
    let symbols = SymbolDao::new(c);
    let natives = symbols.natives()?;
    if natives.is_empty() {
        return Ok(());
    }
    let functions = symbols.jni_functions()?;
    let edges = EdgeDao::new(c);
    // Native methods have no body of their own: every edge leaving one is a JNI link, so redo them all.
    let ids: Vec<SymbolId> = natives.iter().map(|n| n.0).collect();
    edges.delete_from(&ids)?;
    for (from, to) in resolve::jni_links(&natives, &functions) {
        edges.insert(&Edge { from, to, kind: EdgeKind::Calls })?;
    }
    Ok(())
}

fn re_resolve_calls(
    c: &rusqlite::Connection,
    touched: &[SymbolId],
    affected: &HashSet<String>,
    total_files: usize,
    changed_files: usize,
) -> Result<()> {
    let symbols = SymbolDao::new(c);
    let edges = EdgeDao::new(c);

    // First index, or a change big enough that symbol-by-symbol queries would cost more than one pass.
    if changed_files * FULL_RESOLVE_RATIO >= total_files.max(1) || edges.count()? == 0 {
        let list = resolve::resolve(&symbols.all()?, &symbols.call_sites()?, &FileDao::new(c).languages()?, &SignatureDao::new(c).return_types()?);
        edges.clear()?;
        for e in &list {
            edges.insert(e)?;
        }
        return Ok(());
    }

    // Callers to redo: symbols in the changed files, plus any symbol anywhere that calls an
    // affected name (its candidate set - and so its uniqueness rule - may have changed).
    let names: Vec<String> = affected.iter().cloned().collect();
    let mut callers: HashSet<SymbolId> = touched.iter().copied().collect();
    callers.extend(symbols.callers_of_names(&names)?);
    let ids: Vec<SymbolId> = callers.into_iter().collect();

    edges.delete_from(&ids)?;
    let file_of: HashMap<SymbolId, FileId> = symbols.file_ids(&ids)?;
    let owner_of: HashMap<SymbolId, String> = symbols.owners_of(&ids)?;
    let languages = FileDao::new(c).languages()?;
    let rets = SignatureDao::new(c).return_types()?;
    let mut by_name: HashMap<String, Vec<model::Cand>> = HashMap::new();
    for call in symbols.call_sites_of(&ids)? {
        let Some(&caller_file) = file_of.get(&call.symbol_id) else { continue };
        if !by_name.contains_key(&call.callee) {
            by_name.insert(call.callee.clone(), symbols.candidates_by_name(&call.callee)?);
        }
        let candidates = &by_name[&call.callee];
        let recv = resolve::effective_recv(&call.recv_type, &rets);
        let caller_owner = owner_of.get(&call.symbol_id).map(String::as_str);
        if let Some(to) = resolve::pick_target_in((call.symbol_id, caller_file), caller_owner, &call.callee, call.dotted, &recv, candidates, &languages) {
            edges.insert(&Edge { from: call.symbol_id, to, kind: EdgeKind::Calls })?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mysid-index-test-{}-{}", name, std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn edge_names(index: &Index) -> Vec<String> {
        let snap = index.snapshot().unwrap();
        let name = |id: SymbolId| snap.symbols.iter().find(|s| s.id == id).unwrap().name.clone();
        let mut out: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", name(e.from), name(e.to))).collect();
        out.sort();
        out
    }

    /// The incremental edges must always equal what a from-scratch resolution would produce.
    fn assert_edges_match_full(index: &Index) {
        let c = &index.db.conn;
        let symbols = SymbolDao::new(c).all().unwrap();
        let calls = SymbolDao::new(c).call_sites().unwrap();
        let languages = FileDao::new(c).languages().unwrap();
        let rets = SignatureDao::new(c).return_types().unwrap();
        let mut expected: Vec<(i64, i64)> = resolve::resolve(&symbols, &calls, &languages, &rets).iter().map(|e| (e.from, e.to)).collect();
        let mut actual: Vec<(i64, i64)> = EdgeDao::new(c).all().unwrap().iter().filter(|e| e.kind == EdgeKind::Calls).map(|e| (e.from, e.to)).collect();
        expected.sort();
        actual.sort();
        assert_eq!(actual, expected, "incremental edges diverged from a full resolution");
    }

    /// `n` files where f{i} calls f{i-1}: a chain of unique, cross-file calls.
    fn chain(dir: &Path, n: usize) {
        for i in 0..n {
            let body = if i == 0 { "    return 1\n".to_string() } else { format!("    f{}()\n", i - 1) };
            fs::write(dir.join(format!("f{}.py", i)), format!("def f{}():\n{}", i, body)).unwrap();
        }
    }

    /// Kotlin calls through a typed field, JNI in both directions, and a C++ prototype giving way to its definition.
    #[test]
    fn resolves_typed_receivers_jni_and_cpp_definitions() {
        let dir = temp_project("jni");
        fs::write(
            dir.join("UsbDirectOutput.kt"),
            "class UsbDirectOutput(private val negotiator: UsbFormatNegotiator) {\n    private suspend fun setupUsbForFormat(rate: Int): Boolean {\n        val ok = negotiator.setupUsbForFormat(rate)\n        return ok\n    }\n}\n",
        )
        .unwrap();
        fs::write(dir.join("UsbFormatNegotiator.kt"), "class UsbFormatNegotiator {\n    suspend fun setupUsbForFormat(rate: Int): Boolean {\n        return true\n    }\n}\n").unwrap();
        fs::write(dir.join("UsbBridge.kt"), "class UsbBridge {\n    private external fun nativeOpen(fd: Int): Int\n    fun onDevice(id: Int) {\n    }\n}\n").unwrap();
        fs::write(dir.join("usb_jni.h"), "class Engine {\npublic:\n    void start();\n};\n").unwrap();
        fs::write(
            dir.join("usb_jni.cpp"),
            "#include <jni.h>\nvoid Engine::start() {\n    helper();\n}\nvoid helper() {\n}\nextern \"C\" JNIEXPORT jint JNICALL\nJava_com_acme_UsbBridge_nativeOpen(JNIEnv *env, jobject thiz, jint fd) {\n    Engine* engine = new Engine();\n    engine->start();\n    jmethodID m = env->GetMethodID(cls, \"onDevice\", \"(I)V\");\n    return 0;\n}\n",
        )
        .unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let snap = index.snapshot().unwrap();
        let file_of = |id: SymbolId| {
            let s = snap.symbols.iter().find(|s| s.id == id).unwrap();
            let owner = s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default();
            format!("{}{}", owner, s.name)
        };
        let edges: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", file_of(e.from), file_of(e.to))).collect();
        let has = |s: &str| assert!(edges.iter().any(|e| e == s), "missing {} in {:?}", s, edges);
        // A call through `negotiator: UsbFormatNegotiator`, although the caller's own class has a method of that name.
        has("UsbDirectOutput.setupUsbForFormat->UsbFormatNegotiator.setupUsbForFormat");
        // external fun -> the JNI function that implements it.
        has("UsbBridge.nativeOpen->Java_com_acme_UsbBridge_nativeOpen");
        // C++ through a typed pointer, to the definition in the .cpp (not the prototype in the header).
        has("Java_com_acme_UsbBridge_nativeOpen->Engine.start");
        let def = snap.symbols.iter().find(|s| s.name == "start" && s.has_body).unwrap().id;
        assert!(snap.edges.iter().any(|e| e.to == def), "the edge goes to the definition");
        // Native code calling back into Kotlin by method name.
        has("Java_com_acme_UsbBridge_nativeOpen->UsbBridge.onDevice");
        has("Engine.start->helper");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Members declared in a header, thread entry points, calls through a returned pointer, and libc names.
    #[test]
    fn resolves_header_members_thread_entries_chained_calls_and_ignores_libc() {
        let dir = temp_project("cppflow");
        fs::write(
            dir.join("Streamer.h"),
            "#pragma once\nclass Decoder {\npublic:\n    void setRate(int r);\n};\nclass Streamer {\npublic:\n    void start();\n    void loop();\n    Decoder* getDecoder();\nprivate:\n    Decoder* mDecoder;\n};\n",
        )
        .unwrap();
        fs::write(
            dir.join("Streamer.cpp"),
            "#include \"Streamer.h\"\nvoid Decoder::setRate(int r) {\n}\nvoid Streamer::start() {\n    mThread = std::thread(&Streamer::loop, this);\n}\nDecoder* Streamer::getDecoder() {\n    return mDecoder;\n}\nvoid Streamer::loop() {\n    mDecoder->setRate(48000);\n    getDecoder()->setRate(44100);\n    strerror(errno);\n}\n",
        )
        .unwrap();
        fs::write(dir.join("App.cpp"), "#include \"Streamer.h\"\nStreamer* gStreamer = nullptr;\nvoid run() {\n    gStreamer->start();\n}\n").unwrap();
        fs::write(dir.join("Other.cpp"), "int strerror(int e) {\n    return e;\n}\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let snap = index.snapshot().unwrap();
        let name = |id: SymbolId| {
            let s = snap.symbols.iter().find(|s| s.id == id).unwrap();
            format!("{}{}", s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default(), s.name)
        };
        let edges: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", name(e.from), name(e.to))).collect();
        let has = |s: &str| assert!(edges.iter().any(|e| e == s), "missing {} in {:?}", s, edges);
        // `&Streamer::loop` handed to std::thread is a use of loop.
        has("Streamer.start->Streamer.loop");
        // mDecoder is declared in the header, the call is in the .cpp.
        has("Streamer.loop->Decoder.setRate");
        // gStreamer is declared in this file; start is defined in the .cpp.
        has("run->Streamer.start");
        assert!(!edges.iter().any(|e| e.ends_with("->strerror")), "libc strerror must not link to a project symbol: {:?}", edges);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A base class calling its own pure virtual method, and that method reaching the override.
    #[test]
    fn a_base_class_call_to_its_own_virtual_method_reaches_the_override() {
        let dir = temp_project("virtual");
        fs::write(dir.join("BaseWorker.h"), "class BaseWorker {\npublic:\n    void loop();\n    virtual int decode(int n) = 0;\n};\n").unwrap();
        fs::write(dir.join("BaseWorker.cpp"), "#include \"BaseWorker.h\"\nvoid BaseWorker::loop() {\n    decode(5);\n}\n").unwrap();
        fs::write(dir.join("UsbWorker.h"), "#include \"BaseWorker.h\"\nclass UsbWorker : public BaseWorker {\npublic:\n    int decode(int n) override;\n};\n").unwrap();
        fs::write(dir.join("UsbWorker.cpp"), "#include \"UsbWorker.h\"\nint UsbWorker::decode(int n) {\n    return n;\n}\n").unwrap();
        fs::write(dir.join("Player.kt"), "interface Source\nopen class Base {\n    open fun play() {\n    }\n}\nclass Player(private val x: Int) : Base(), Source {\n    override fun play() {\n    }\n}\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let snap = index.snapshot().unwrap();
        let name = |id: SymbolId| {
            let s = snap.symbols.iter().find(|s| s.id == id).unwrap();
            format!("{}{}", s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default(), s.name)
        };
        let edges: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", name(e.from), name(e.to))).collect();
        let has = |s: &str| assert!(edges.iter().any(|e| e == s), "missing {} in {:?}", s, edges);
        // loop calls its own class's decode (the pure virtual declaration), and that dispatches to the override.
        has("BaseWorker.loop->BaseWorker.decode");
        has("BaseWorker.decode->UsbWorker.decode");
        // Kotlin: a base class method dispatches to the subclass's override; the interface name is not a base method.
        has("Base.play->Player.play");
        let _ = fs::remove_dir_all(&dir);
    }

    /// The receiver typing and override rules are not C++ features: Java, TypeScript, Python and Rust get them too, and a
    /// project function named like a libc one still links outside C/C++.
    #[test]
    fn typed_receivers_and_plain_names_work_in_java_typescript_python_and_rust() {
        let dir = temp_project("polyglot");
        // Java: field type, and an override through `extends`.
        fs::write(dir.join("Engine.java"), "public class Engine {\n    public void start() {\n    }\n}\n").unwrap();
        fs::write(dir.join("Car.java"), "public class Car {\n    private Engine engine;\n    public void drive() {\n        engine.start();\n    }\n}\n").unwrap();
        fs::write(dir.join("Base.java"), "public class Base {\n    public void run() {\n    }\n}\n").unwrap();
        fs::write(dir.join("Child.java"), "public class Child extends Base {\n    public void run() {\n    }\n}\n").unwrap();
        // TypeScript: `private svc: Service` and `this.svc.go()`.
        fs::write(dir.join("service.ts"), "export class Service {\n  go() {\n  }\n}\n").unwrap();
        fs::write(dir.join("app.ts"), "export class App {\n  private svc: Service;\n  run() {\n    this.svc.go();\n  }\n}\n").unwrap();
        // Python: an annotated parameter, and a plain call to a project function called `read` (a libc name in C).
        fs::write(dir.join("store.py"), "class Store:\n    def save(self):\n        return 1\n\ndef read():\n    return 2\n").unwrap();
        fs::write(dir.join("job.py"), "def work(store: Store):\n    store.save()\n    read()\n").unwrap();
        // Rust: `Self::helper()` inside an impl.
        fs::write(dir.join("lib.rs"), "struct Foo;\nimpl Foo {\n    fn helper() {\n    }\n    fn run() {\n        Self::helper();\n    }\n}\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let snap = index.snapshot().unwrap();
        let name = |id: SymbolId| {
            let s = snap.symbols.iter().find(|s| s.id == id).unwrap();
            format!("{}{}", s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default(), s.name)
        };
        let edges: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", name(e.from), name(e.to))).collect();
        let has = |s: &str| assert!(edges.iter().any(|e| e == s), "missing {} in {:?}", s, edges);
        has("Car.drive->Engine.start");
        has("Base.run->Child.run");
        has("App.run->Service.go");
        has("work->Store.save");
        has("work->read");
        has("Foo.run->Foo.helper");
        let _ = fs::remove_dir_all(&dir);
    }

    /// A constructor's initializer list is not a function: the body belongs to the constructor.
    #[test]
    fn a_cpp_constructor_with_an_initializer_list_owns_its_body() {
        let dir = temp_project("ctor");
        fs::write(
            dir.join("AudioEngine.h"),
            "class IWorker {\npublic:\n    virtual void start() = 0;\n};\nclass AudioEngine {\npublic:\n    AudioEngine();\n    ~AudioEngine();\n    void stop();\nprivate:\n    IWorker* mDecodeWorker;\n    int mRingBuffer;\n};\n",
        )
        .unwrap();
        fs::write(
            dir.join("AudioEngine.cpp"),
            "#include \"AudioEngine.h\"\nIWorker* createWorker(bool u) {\n  return nullptr;\n}\nAudioEngine::AudioEngine()\n    : mRingBuffer(kCap) {\n  mDecodeWorker = createWorker(false);\n  mDecodeWorker->start();\n}\n\nAudioEngine::~AudioEngine() {\n  stop();\n}\nvoid AudioEngine::stop() {\n}\n",
        )
        .unwrap();
        // Brace initializers must not end the constructor early, and a following method stays separate.
        fs::write(
            dir.join("Worker.cpp"),
            "void helper() {\n}\nvoid tick() {\n}\nWorker::Worker(int a)\n    : mA{a}, mB(a) {\n  helper();\n}\nvoid Worker::start() {\n  tick();\n}\nBaseDecode::BaseDecode(Auth &auth)\n    : mAuth(auth) {}\nvoid BaseDecode::run() {\n}\n",
        )
        .unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let snap = index.snapshot().unwrap();
        let name = |id: SymbolId| {
            let s = snap.symbols.iter().find(|s| s.id == id).unwrap();
            format!("{}{}", s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default(), s.name)
        };
        let symbols: Vec<String> = snap.symbols.iter().map(|s| format!("{}{}", s.owner.clone().map(|o| format!("{}.", o)).unwrap_or_default(), s.name)).collect();
        for fake in ["mRingBuffer", "mA", "mB", "mAuth"] {
            assert!(!symbols.iter().any(|s| s == fake || s.ends_with(&format!(".{}", fake))), "{} is an initializer, not a symbol: {:?}", fake, symbols);
        }
        for real in ["AudioEngine.AudioEngine", "AudioEngine.~AudioEngine", "Worker.Worker", "Worker.start", "BaseDecode.BaseDecode", "BaseDecode.run"] {
            assert!(symbols.iter().any(|s| s == real), "missing {} in {:?}", real, symbols);
        }
        let edges: Vec<String> = snap.edges.iter().map(|e| format!("{}->{}", name(e.from), name(e.to))).collect();
        let has = |s: &str| assert!(edges.iter().any(|e| e == s), "missing {} in {:?}", s, edges);
        has("AudioEngine.AudioEngine->IWorker.start");
        has("AudioEngine.AudioEngine->createWorker");
        has("AudioEngine.~AudioEngine->AudioEngine.stop");
        has("Worker.Worker->helper");
        has("Worker.start->tick");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn indexes_incrementally_and_resolves_edges() {
        let dir = temp_project("incremental");
        fs::write(dir.join("a.py"), "def helper():\n    return 1\n\ndef main():\n    helper()\n").unwrap();
        fs::write(dir.join("b.py"), "def other():\n    main()\n").unwrap();
        let index = Index::in_memory();

        let stats = index.refresh(&dir).unwrap();
        assert_eq!((stats.added, stats.updated, stats.removed), (2, 0, 0));
        assert_eq!(edge_names(&index), vec!["main->helper", "other->main"]);

        // Nothing changed: no work.
        let stats = index.refresh(&dir).unwrap();
        assert_eq!(stats, RefreshStats { unchanged: 2, ..Default::default() });

        // Edit one file: only it is re-parsed, edges follow.
        fs::write(dir.join("a.py"), "def helper():\n    return 1\n\ndef main():\n    pass\n\ndef extra():\n    helper()\n").unwrap();
        let stats = index.refresh(&dir).unwrap();
        assert_eq!((stats.added, stats.updated, stats.removed, stats.unchanged), (0, 1, 0, 1));
        assert_eq!(edge_names(&index), vec!["extra->helper", "other->main"]);

        // Delete a file: its symbols and edges go with it.
        fs::remove_file(dir.join("b.py")).unwrap();
        let stats = index.refresh(&dir).unwrap();
        assert_eq!((stats.added, stats.updated, stats.removed), (0, 0, 1));
        assert_eq!(edge_names(&index), vec!["extra->helper"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn small_changes_re_resolve_incrementally_and_match_a_full_resolution() {
        let dir = temp_project("affected");
        chain(&dir, 12);
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        assert_eq!(edge_names(&index).len(), 11);
        assert_edges_match_full(&index);

        // A second `f3` makes every call to the name ambiguous: f4's edge must disappear.
        fs::write(dir.join("g.py"), "def f3():\n    pass\n").unwrap();
        let stats = index.refresh(&dir).unwrap();
        assert_eq!((stats.added, stats.updated, stats.removed), (1, 0, 0));
        assert!(!edge_names(&index).contains(&"f4->f3".to_string()));
        assert_edges_match_full(&index);

        // Removing it makes the name unique again: the edge comes back.
        fs::remove_file(dir.join("g.py")).unwrap();
        index.refresh(&dir).unwrap();
        assert!(edge_names(&index).contains(&"f4->f3".to_string()));
        assert_edges_match_full(&index);

        // Rewriting a callee file re-links its callers to the new symbol ids.
        fs::write(dir.join("f5.py"), "def f5():\n    f0()\n\ndef extra():\n    pass\n").unwrap();
        index.refresh(&dir).unwrap();
        assert!(edge_names(&index).contains(&"f5->f0".to_string()));
        assert!(edge_names(&index).contains(&"f6->f5".to_string()));
        assert_edges_match_full(&index);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_paths_touches_only_the_given_files() {
        let dir = temp_project("paths");
        chain(&dir, 12);
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();

        fs::write(dir.join("f5.py"), "def f5():\n    f9()\n").unwrap(); // modified
        fs::remove_file(dir.join("f6.py")).unwrap(); // deleted
        fs::write(dir.join("f12.py"), "def f12():\n    f11()\n").unwrap(); // added
        let stats = index
            .refresh_paths(&dir, &[dir.join("f5.py"), PathBuf::from("f6.py"), dir.join("f12.py"), dir.join("nothing.py")])
            .unwrap();
        assert_eq!((stats.added, stats.updated, stats.removed), (1, 1, 1));
        let edges = edge_names(&index);
        assert!(edges.contains(&"f5->f9".to_string()) && edges.contains(&"f12->f11".to_string()));
        assert!(!edges.iter().any(|e| e.contains("f6")), "{:?}", edges);
        assert_edges_match_full(&index);

        // A directory removal event drops everything beneath it.
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("sub").join("s1.py"), "def s1():\n    pass\n").unwrap();
        index.refresh_paths(&dir, &[dir.join("sub").join("s1.py")]).unwrap();
        assert!(index.has_path("sub").unwrap());
        fs::remove_dir_all(dir.join("sub")).unwrap();
        let stats = index.refresh_paths(&dir, &[dir.join("sub")]).unwrap();
        assert_eq!(stats.removed, 1);
        assert!(!index.has_path("sub").unwrap());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn candidates_find_exact_qualified_and_fuzzy_matches() {
        let dir = temp_project("candidates");
        fs::write(dir.join("m.py"), "class Foo:\n    def bar(self):\n        pass\n\ndef helper_task():\n    pass\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();

        let names = |q: &str| -> Vec<String> {
            index.candidates(q).unwrap().iter().map(|l| format!("{}:{}", l.path, l.symbol.name)).collect()
        };
        assert_eq!(names("HELPER_TASK"), vec!["m.py:helper_task"], "exact name, any case");
        assert_eq!(names("Foo.bar"), vec!["m.py:bar"], "Class.method");
        assert_eq!(names("Foo::bar"), vec!["m.py:bar"], "Class::method");
        assert_eq!(names("m.py:bar"), vec!["m.py:bar"], "file:Name");
        assert_eq!(names("elper"), vec!["m.py:helper_task"], "fuzzy only when nothing matches exactly");
        assert!(names("zzz_missing").is_empty());
        // `_` and `%` in the query are literal, not LIKE wildcards.
        assert!(names("h_lper").is_empty());

        let foo = index.candidates("Foo").unwrap().remove(0);
        assert_eq!(index.members(&foo).unwrap().iter().map(|l| l.symbol.name.clone()).collect::<Vec<_>>(), vec!["bar"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn neighborhood_loads_only_what_the_depth_needs() {
        let dir = temp_project("hood");
        // a -> b -> c -> d, each in its own file.
        for (n, callee) in [("a", "b"), ("b", "c"), ("c", "d"), ("d", "")] {
            let body = if callee.is_empty() { "    pass\n".to_string() } else { format!("    {}()\n", callee) };
            fs::write(dir.join(format!("{}.py", n)), format!("def {}():\n{}", n, body)).unwrap();
        }
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();
        let id = |name: &str| index.candidates(name).unwrap()[0].symbol.id;
        let names = |hood: &Neighborhood| {
            let mut v: Vec<String> = hood.symbols.iter().map(|l| l.symbol.name.clone()).collect();
            v.sort();
            v
        };

        let one = index.neighborhood(&[id("b")], 1).unwrap();
        assert_eq!(names(&one), vec!["a", "b", "c"]);
        assert_eq!(one.edges.len(), 2);

        let two = index.neighborhood(&[id("b")], 2).unwrap();
        assert_eq!(names(&two), vec!["a", "b", "c", "d"]);
        assert_eq!(two.edges.len(), 3);

        let none = index.neighborhood(&[id("d")], 1).unwrap();
        assert_eq!(names(&none), vec!["c", "d"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn calls_link_only_within_a_language_family() {
        let dir = temp_project("langs");
        // `start` exists in Python; a JavaScript function calling `start()` must not link to it.
        fs::write(dir.join("engine.py"), "def start():\n    pass\n\ndef run():\n    start()\n").unwrap();
        fs::write(dir.join("ui.js"), "function launch() {\n  start();\n}\n").unwrap();
        // Kotlin and Java share a family: Kotlin may call a Java method by name.
        fs::write(dir.join("Helper.java"), "public class Helper {\n    public void javaThing() {\n    }\n}\n").unwrap();
        fs::write(dir.join("App.kt"), "fun callJava() {\n    javaThing()\n}\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();

        let edges = edge_names(&index);
        assert!(edges.contains(&"run->start".to_string()), "same language links: {:?}", edges);
        assert!(!edges.contains(&"launch->start".to_string()), "JS must not link to Python: {:?}", edges);
        assert!(edges.contains(&"callJava->javaThing".to_string()), "Kotlin -> Java links: {:?}", edges);
        assert_edges_match_full(&index);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn daos_answer_caller_and_callee_queries() {
        let dir = temp_project("dao");
        fs::write(dir.join("a.py"), "def leaf():\n    return 1\n\ndef mid():\n    leaf()\n\ndef top():\n    mid()\n").unwrap();
        let index = Index::in_memory();
        index.refresh(&dir).unwrap();

        let conn = &index.db.conn;
        let symbols = SymbolDao::new(conn);
        let edges = EdgeDao::new(conn);

        let mid = symbols.find_by_name("mid").unwrap().remove(0);
        let top = symbols.find_by_name("top").unwrap().remove(0);
        let leaf = symbols.find_by_name("leaf").unwrap().remove(0);
        assert_eq!(edges.find_callers(mid.id).unwrap(), vec![top.id]);
        assert_eq!(edges.find_callees(mid.id).unwrap(), vec![leaf.id]);

        let file_id = FileDao::new(conn).all().unwrap()[0].id;
        assert_eq!(symbols.find_by_file(file_id).unwrap().len(), 3);

        let _ = fs::remove_dir_all(&dir);
    }
}
