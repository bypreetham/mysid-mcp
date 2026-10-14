use rusqlite::{params, params_from_iter, Connection};

use super::{chunked, placeholders, Result};
use crate::index::model::{Edge, EdgeKind, SymbolId};

pub struct EdgeDao<'a> {
    conn: &'a Connection,
}

/// What a graph view follows: calls, and the dispatch from a base class method to its overrides.
const FOLLOWED: &str = "kind IN ('CALLS', 'DISPATCH')";

fn kind_of(name: &str) -> EdgeKind {
    if name == "DISPATCH" { EdgeKind::Dispatch } else { EdgeKind::Calls }
}

impl<'a> EdgeDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM edges", [])?;
        Ok(())
    }

    pub fn count(&self) -> Result<usize> {
        self.conn.query_row("SELECT COUNT(*) FROM edges", [], |r| r.get::<_, i64>(0)).map(|n| n as usize)
    }

    pub fn insert(&self, e: &Edge) -> Result<()> {
        self.conn
            .prepare_cached("INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, ?3)")?
            .execute(params![e.from, e.to, e.kind.as_str()])?;
        Ok(())
    }

    /// Drops the outgoing call edges of these symbols (they are about to be re-resolved). Dispatch edges are
    /// recomputed as a whole (`delete_kind`), so they are not touched here.
    pub fn delete_from(&self, ids: &[SymbolId]) -> Result<()> {
        chunked(ids, |chunk| {
            let sql = format!("DELETE FROM edges WHERE kind = 'CALLS' AND from_id IN ({})", placeholders(chunk.len()));
            self.conn.execute(&sql, params_from_iter(chunk.iter()))?;
            Ok(Vec::<()>::new())
        })?;
        Ok(())
    }

    /// Drops every edge of one kind.
    pub fn delete_kind(&self, kind: EdgeKind) -> Result<()> {
        self.conn.execute("DELETE FROM edges WHERE kind = ?1", params![kind.as_str()])?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare(&format!("SELECT from_id, to_id, kind FROM edges WHERE {}", FOLLOWED))?;
        let rows = stmt.query_map([], |r| Ok(Edge { from: r.get(0)?, to: r.get(1)?, kind: kind_of(&r.get::<_, String>(2)?) }))?;
        rows.collect()
    }

    /// Edges leaving any of `ids` (what they call).
    pub fn callees_of(&self, ids: &[SymbolId]) -> Result<Vec<Edge>> {
        chunked(ids, |chunk| {
            let sql = format!("SELECT from_id, to_id, kind FROM edges WHERE {} AND from_id IN ({})", FOLLOWED, placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
                Ok(Edge { from: r.get(0)?, to: r.get(1)?, kind: kind_of(&r.get::<_, String>(2)?) })
            })?;
            rows.collect()
        })
    }

    /// Edges arriving at any of `ids` (who calls them).
    pub fn callers_of(&self, ids: &[SymbolId]) -> Result<Vec<Edge>> {
        chunked(ids, |chunk| {
            let sql = format!("SELECT from_id, to_id, kind FROM edges WHERE {} AND to_id IN ({})", FOLLOWED, placeholders(chunk.len()));
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params_from_iter(chunk.iter()), |r| {
                Ok(Edge { from: r.get(0)?, to: r.get(1)?, kind: kind_of(&r.get::<_, String>(2)?) })
            })?;
            rows.collect()
        })
    }

    #[allow(dead_code)] // part of the DAO contract; used by tests now, `read`/`search` next
    pub fn find_callers(&self, target: SymbolId) -> Result<Vec<SymbolId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT from_id FROM edges WHERE to_id = ?1 AND kind = 'CALLS' ORDER BY from_id")?;
        let rows = stmt.query_map(params![target], |r| r.get(0))?;
        rows.collect()
    }

    #[allow(dead_code)]
    pub fn find_callees(&self, source: SymbolId) -> Result<Vec<SymbolId>> {
        let mut stmt = self
            .conn
            .prepare("SELECT to_id FROM edges WHERE from_id = ?1 AND kind = 'CALLS' ORDER BY to_id")?;
        let rows = stmt.query_map(params![source], |r| r.get(0))?;
        rows.collect()
    }
}
