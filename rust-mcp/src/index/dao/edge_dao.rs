use rusqlite::{params, Connection};

use super::Result;
use crate::index::model::{Edge, EdgeKind, SymbolId};

pub struct EdgeDao<'a> {
    conn: &'a Connection,
}

impl<'a> EdgeDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute("DELETE FROM edges", [])?;
        Ok(())
    }

    pub fn insert(&self, e: &Edge) -> Result<()> {
        self.conn.execute(
            "INSERT OR IGNORE INTO edges (from_id, to_id, kind) VALUES (?1, ?2, ?3)",
            params![e.from, e.to, e.kind.as_str()],
        )?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Edge>> {
        let mut stmt = self.conn.prepare("SELECT from_id, to_id FROM edges WHERE kind = 'CALLS'")?;
        let rows = stmt.query_map([], |r| {
            Ok(Edge { from: r.get(0)?, to: r.get(1)?, kind: EdgeKind::Calls })
        })?;
        rows.collect()
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
