//! Persistent code index.
//!
//! ```text
//! parsers -> extract -> domain model -> DAOs (SQLite: <workspace>/.mysid/index.db)
//!                                          \-> resolve (in-memory HashMaps) -> edges
//! ```
//!
//! The index is a cache of what the parsers see. It refreshes itself incrementally: only files
//! whose mtime or size changed are re-parsed, and call edges are re-resolved when anything did.
//! Consumers (graph today; read/search later) work from a [`Snapshot`], never from SQL.

pub mod dao;
mod extract;
pub mod model;
mod resolve;
mod scan;

use std::collections::{HashMap, HashSet};
use std::path::Path;

use dao::{in_tx, Db, EdgeDao, FileDao, Result, SymbolDao};
pub use model::{Edge, File, Symbol, SymbolId, SymbolKind};

const DB_FILE: &str = "index.db";

/// Everything the index knows, loaded in three queries.
pub struct Snapshot {
    pub files: Vec<File>,
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RefreshStats {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
}

pub struct Index {
    db: Db,
}

impl Index {
    /// Opens (creating if needed) the workspace index and brings it up to date.
    /// Never fails: if `.mysid/index.db` is unusable it is rebuilt, and as a last resort the
    /// index lives in memory for this run.
    pub fn open(root: &Path) -> Index {
        if let Ok(dir) = crate::tools::build_log::ensure_dir(root) {
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
                    let _ = std::fs::remove_file(&path);
                }
            }
        }
        let index = Index { db: Db::in_memory().expect("in-memory sqlite") };
        let _ = index.refresh(root);
        index
    }

    /// Re-indexes new/changed files and drops deleted ones. A no-change refresh takes no write lock.
    pub fn refresh(&self, root: &Path) -> Result<RefreshStats> {
        let scanned = scan::scan(root);
        let conn = &self.db.conn;

        let known = FileDao::new(conn).all()?;
        let known_by_path: HashMap<&str, &File> = known.iter().map(|f| (f.path.as_str(), f)).collect();
        let on_disk: HashSet<&str> = scanned.iter().map(|s| s.rel.as_str()).collect();

        let dirty = scanned
            .iter()
            .any(|s| known_by_path.get(s.rel.as_str()).map_or(true, |f| f.mtime != s.mtime || f.size != s.size))
            || known.iter().any(|f| !on_disk.contains(f.path.as_str()));
        if !dirty {
            return Ok(RefreshStats { unchanged: scanned.len(), ..Default::default() });
        }

        in_tx(conn, |c| {
            let files = FileDao::new(c);
            let symbols = SymbolDao::new(c);
            // Re-read inside the lock: another mysid process may have indexed in the meantime.
            let known: HashMap<String, File> = files.all()?.into_iter().map(|f| (f.path.clone(), f)).collect();
            let mut stats = RefreshStats::default();

            for s in &scanned {
                match known.get(&s.rel) {
                    Some(f) if f.mtime == s.mtime && f.size == s.size => {
                        stats.unchanged += 1;
                        continue;
                    }
                    Some(f) => {
                        files.delete(f.id)?;
                        stats.updated += 1;
                    }
                    None => stats.added += 1,
                }

                let parsed = extract::parse_file(&s.path);
                let line_count = parsed.as_ref().map_or(0, |p| p.lines);
                let file_id = files.insert(&s.rel, &s.language, s.mtime, s.size, line_count)?;
                for sym in parsed.iter().flat_map(|p| p.symbols.iter()) {
                    let id = symbols.insert(file_id, sym)?;
                    for (callee, dotted) in &sym.calls {
                        symbols.insert_call(id, callee, *dotted)?;
                    }
                }
            }

            for (path, f) in &known {
                if !on_disk.contains(path.as_str()) {
                    files.delete(f.id)?;
                    stats.removed += 1;
                }
            }

            if stats.added + stats.updated + stats.removed > 0 {
                let edges = resolve::resolve(&symbols.all()?, &symbols.call_sites()?);
                let edge_dao = EdgeDao::new(c);
                edge_dao.clear()?;
                for e in &edges {
                    edge_dao.insert(e)?;
                }
            }
            Ok(stats)
        })
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let conn = &self.db.conn;
        Ok(Snapshot {
            files: FileDao::new(conn).all()?,
            symbols: SymbolDao::new(conn).all()?,
            edges: EdgeDao::new(conn).all()?,
        })
    }

    #[cfg(test)]
    fn in_memory() -> Index {
        Index { db: Db::in_memory().unwrap() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

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
