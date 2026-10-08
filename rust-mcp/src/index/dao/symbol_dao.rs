use rusqlite::{params, Connection, Row};

use super::Result;
use crate::index::extract::ParsedSymbol;
use crate::index::model::{CallSite, FileId, Symbol, SymbolId, SymbolKind};

pub struct SymbolDao<'a> {
    conn: &'a Connection,
}

const COLUMNS: &str = "id, file_id, name, kind, detail, owner, start_line, end_line";

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
    })
}

impl<'a> SymbolDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    /// Inserts a symbol, or returns the existing one with the same (file, owner, name):
    /// overloads and `struct Foo` + `impl Foo` collapse into a single symbol.
    pub fn insert(&self, file_id: FileId, s: &ParsedSymbol) -> Result<SymbolId> {
        let owner = s.owner.as_deref().unwrap_or("");
        self.conn.execute(
            "INSERT OR IGNORE INTO symbols (file_id, name, kind, detail, owner, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![file_id, s.name, s.kind.as_str(), s.detail, owner, s.start_line, s.end_line],
        )?;
        self.conn.query_row(
            "SELECT id FROM symbols WHERE file_id = ?1 AND owner = ?2 AND name = ?3",
            params![file_id, owner, s.name],
            |r| r.get(0),
        )
    }

    pub fn insert_call(&self, symbol_id: SymbolId, callee: &str, dotted: bool) -> Result<()> {
        self.conn.execute(
            "INSERT INTO call_sites (symbol_id, callee, dotted) VALUES (?1, ?2, ?3)",
            params![symbol_id, callee, dotted as i64],
        )?;
        Ok(())
    }

    pub fn all(&self) -> Result<Vec<Symbol>> {
        let mut stmt = self.conn.prepare(&format!("SELECT {} FROM symbols ORDER BY id", COLUMNS))?;
        let rows = stmt.query_map([], to_symbol)?;
        rows.collect()
    }

    pub fn call_sites(&self) -> Result<Vec<CallSite>> {
        let mut stmt = self.conn.prepare("SELECT symbol_id, callee, dotted FROM call_sites")?;
        let rows = stmt.query_map([], |r| {
            Ok(CallSite { symbol_id: r.get(0)?, callee: r.get(1)?, dotted: r.get::<_, i64>(2)? != 0 })
        })?;
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
