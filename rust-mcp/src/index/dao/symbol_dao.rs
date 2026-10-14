use rusqlite::{params, params_from_iter, Connection, Row};
use std::collections::HashMap;

use super::{chunked, like_escape, placeholders, Result};
use crate::index::extract::ParsedSymbol;
use crate::index::model::{CallSite, Cand, FileId, Located, Symbol, SymbolId, SymbolKind};

pub struct SymbolDao<'a> {
    conn: &'a Connection,
}

const COLUMNS: &str = "id, file_id, name, kind, detail, owner, start_line, end_line, body";

/// Symbols joined to their file path; column order matches `to_symbol` plus the path at the end.
const LOCATED: &str = "SELECT s.id, s.file_id, s.name, s.kind, s.detail, s.owner, s.start_line, s.end_line, s.body, f.path \
                       FROM symbols s JOIN files f ON f.id = s.file_id";

fn to_symbol(r: &Row) -> rusqlite::Result<Symbol> {
    let kind: String = r.get(3)?;
    let owner: String = r.get(5)?;
    Ok(Symbol {
        id: r.get(0)?,
        file_id: r.get(1)?,
        name: r.get(2)?,
        kind: SymbolKind::parse(&kind),
        detail: r.get(4)?,
        owner: if owner.is_empty() { None } else { Some(owner) },
        start_line: r.get(6)?,
        end_line: r.get(7)?,
        has_body: r.get::<_, i64>(8)? != 0,
    })
}

fn to_located(r: &Row) -> rusqlite::Result<Located> {
    Ok(Located { symbol: to_symbol(r)?, path: r.get(9)? })
}

impl<'a> SymbolDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Inserts a symbol, or returns the existing one with the same (file, owner, name):
    /// overloads and `struct Foo` + `impl Foo` collapse into a single symbol.
    pub fn insert(&self, file_id: FileId, s: &ParsedSymbol) -> Result<SymbolId> {
        let owner = s.owner.as_deref().unwrap_or("");
        self.conn
            .prepare_cached(
                "INSERT OR IGNORE INTO symbols (file_id, name, name_lower, kind, detail, owner, start_line, end_line, body, bases)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?
            .execute(params![file_id, s.name, s.name.to_lowercase(), s.kind.as_str(), s.detail, owner, s.start_line, s.end_line, s.has_body as i64, s.bases.join(",")])?;
        self.conn
            .prepare_cached("SELECT id FROM symbols WHERE file_id = ?1 AND owner = ?2 AND name = ?3")?
            .query_row(params![file_id, owner, s.name], |r| r.get(0))
    }

    pub fn insert_call(&self, symbol_id: SymbolId, callee: &str, dotted: bool, recv_type: &str) -> Result<()> {
        self.conn
            .prepare_cached("INSERT INTO call_sites (symbol_id, callee, dotted, recv_type) VALUES (?1, ?2, ?3, ?4)")?
            .execute(params![symbol_id, callee, dotted as i64, recv_type])?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {} FROM symbols ORDER BY id", COLUMNS))?;
        let rows = stmt.query_map([], to_symbol)?;
        rows.collect()
    }

    pub fn call_sites(&self) -> Result<Vec<CallSite>> {
        let mut stmt = self.conn.prepare("SELECT symbol_id, callee, dotted, recv_type FROM call_sites")?;
        let rows = stmt.query_map([], to_call_site)?;
        rows.collect()
    }

    // ── targeted reads ──────────────────────────────────────────────────────────────────────

    /// Case-insensitive exact name (`name_lower` is the lower-cased query).
    pub fn located_by_name(&self, name_lower: &str) -> Result<Vec<Located>> {
        let mut stmt = self
            .conn
            .prepare_cached(&format!("{} WHERE s.name_lower = ?1 ORDER BY f.path, s.start_line", LOCATED))?;
        let rows = stmt.query_map(params![name_lower], to_located)?;
        rows.collect()
    }

    /// `Owner.name`, case-insensitive.
    pub fn located_by_owner_name(&self, owner_lower: &str, name_lower: &str) -> Result<Vec<Located>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE s.name_lower = ?1 AND lower(s.owner) = ?2 ORDER BY f.path, s.start_line",
            LOCATED
        ))?;
        let rows = stmt.query_map(params![name_lower, owner_lower], to_located)?;
        rows.collect()
    }

    /// `path/suffix:name` - a symbol in a file whose path equals or ends with `path_part`.
    pub fn located_by_path_name(&self, path_lower: &str, name_lower: &str) -> Result<Vec<Located>> {
        let suffix = format!("%/{}", like_escape(path_lower));
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE s.name_lower = ?1 AND (lower(f.path) = ?2 OR lower(f.path) LIKE ?3 ESCAPE '\\') \
             ORDER BY f.path, s.start_line",
            LOCATED
        ))?;
        let rows = stmt.query_map(params![name_lower, path_lower, suffix], to_located)?;
        rows.collect()
    }

    /// Names containing the text (fallback when nothing matches exactly).
    pub fn located_fuzzy(&self, text_lower: &str, limit: usize) -> Result<Vec<Located>> {
        let pattern = format!("%{}%", like_escape(text_lower));
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE s.name_lower LIKE ?1 ESCAPE '\\' ORDER BY f.path, s.start_line LIMIT ?2",
            LOCATED
        ))?;
        let rows = stmt.query_map(params![pattern, limit as i64], to_located)?;
        rows.collect()
    }

    pub fn located_by_ids(&self, ids: &[SymbolId]) -> Result<Vec<Located>> {
        chunked(ids, |chunk| {
            let sql = format!("{} WHERE s.id IN ({})", LOCATED, placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), to_located)?;
            rows.collect()
        })
    }

    /// Methods of `owner` in one file, in source order.
    pub fn members(&self, file_id: FileId, owner: &str) -> Result<Vec<Located>> {
        let mut stmt = self.conn.prepare_cached(&format!(
            "{} WHERE s.file_id = ?1 AND s.owner = ?2 AND s.kind = 'method' ORDER BY s.start_line",
            LOCATED
        ))?;
        let rows = stmt.query_map(params![file_id, owner], to_located)?;
        rows.collect()
    }

    // ── incremental edge resolution ─────────────────────────────────────────────────────────

    pub fn names_in_file(&self, file_id: FileId) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached("SELECT DISTINCT name FROM symbols WHERE file_id = ?1")?;
        let rows = stmt.query_map(params![file_id], |r| r.get(0))?;
        rows.collect()
    }

    /// Symbols that call any of `names`.
    pub fn callers_of_names(&self, names: &[String]) -> Result<Vec<SymbolId>> {
        chunked(names, |chunk| {
            let sql = format!("SELECT DISTINCT symbol_id FROM call_sites WHERE callee IN ({})", placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| r.get(0))?;
            rows.collect()
        })
    }

    pub fn call_sites_of(&self, ids: &[SymbolId]) -> Result<Vec<CallSite>> {
        chunked(ids, |chunk| {
            let sql = format!(
                "SELECT symbol_id, callee, dotted, recv_type FROM call_sites WHERE symbol_id IN ({})",
                placeholders(chunk.len())
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), to_call_site)?;
            rows.collect()
        })
    }

    pub fn file_ids(&self, ids: &[SymbolId]) -> Result<HashMap<SymbolId, FileId>> {
        let pairs: Vec<(SymbolId, FileId)> = chunked(ids, |chunk| {
            let sql = format!("SELECT id, file_id FROM symbols WHERE id IN ({})", placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })?;
        Ok(pairs.into_iter().collect())
    }

    /// Every symbol with exactly this name, with what the resolver needs to choose between them, in id order.
    pub fn candidates_by_name(&self, name: &str) -> Result<Vec<Cand>> {
        let mut stmt = self.conn.prepare_cached("SELECT id, file_id, owner, body FROM symbols WHERE name = ?1 ORDER BY id")?;
        let rows = stmt.query_map(params![name], |r| {
            let owner: String = r.get(2)?;
            Ok(Cand { id: r.get(0)?, file: r.get(1)?, owner: if owner.is_empty() { None } else { Some(owner) }, has_body: r.get::<_, i64>(3)? != 0 })
        })?;
        rows.collect()
    }

    /// Methods declared `external` (Kotlin) or `native` (Java): `(id, owner, name)`. Their bodies are C/C++ JNI functions.
    pub fn natives(&self) -> Result<Vec<(SymbolId, String, String)>> {
        let mut stmt = self.conn.prepare("SELECT id, owner, name FROM symbols WHERE detail = 'native'")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// Classes that name a base class or interface: `(class, bases)`.
    pub fn classes_with_bases(&self) -> Result<Vec<(String, Vec<String>)>> {
        let mut stmt = self.conn.prepare("SELECT name, bases FROM symbols WHERE kind = 'class' AND bases != ''")?;
        let rows = stmt.query_map([], |r| {
            let bases: String = r.get(1)?;
            Ok((r.get::<_, String>(0)?, bases.split(',').filter(|b| !b.is_empty()).map(String::from).collect()))
        })?;
        rows.collect()
    }

    /// Every method that has an owner: `(id, owner, name, has_body)`.
    pub fn methods(&self) -> Result<Vec<(SymbolId, String, String, bool)>> {
        let mut stmt = self.conn.prepare("SELECT id, owner, name, body FROM symbols WHERE owner != '' AND kind = 'method'")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)))?;
        rows.collect()
    }

    /// The owning type of each of these symbols (those that have one).
    pub fn owners_of(&self, ids: &[SymbolId]) -> Result<HashMap<SymbolId, String>> {
        let pairs: Vec<(SymbolId, String)> = chunked(ids, |chunk| {
            let sql = format!("SELECT id, owner FROM symbols WHERE owner != '' AND id IN ({})", placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect()
        })?;
        Ok(pairs.into_iter().collect())
    }

    /// JNI entry points in native code (`Java_pkg_Class_method`): `(id, name, has_body)`.
    pub fn jni_functions(&self) -> Result<Vec<(SymbolId, String, bool)>> {
        let mut stmt = self.conn.prepare("SELECT id, name, body FROM symbols WHERE name LIKE 'Java\\_%' ESCAPE '\\'")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
        rows.collect()
    }

    #[allow(dead_code)] // part of the DAO contract; used by tests now, `read`/`search` next
    pub fn find_by_name(&self, name: &str) -> Result<Vec<Symbol>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {} FROM symbols WHERE name = ?1 ORDER BY id", COLUMNS))?;
        let rows = stmt.query_map(params![name], to_symbol)?;
        rows.collect()
    }

    #[allow(dead_code)]
    pub fn find_by_file(&self, file_id: FileId) -> Result<Vec<Symbol>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {} FROM symbols WHERE file_id = ?1 ORDER BY start_line", COLUMNS))?;
        let rows = stmt.query_map(params![file_id], to_symbol)?;
        rows.collect()
    }
}

fn to_call_site(r: &Row) -> rusqlite::Result<CallSite> {
    Ok(CallSite { symbol_id: r.get(0)?, callee: r.get(1)?, dotted: r.get::<_, i64>(2)? != 0, recv_type: r.get(3)? })
}
