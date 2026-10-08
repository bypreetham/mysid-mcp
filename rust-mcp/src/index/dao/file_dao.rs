use rusqlite::{params, Connection, Row};

use super::Result;
use crate::index::model::{File, FileId};

pub struct FileDao<'a> {
    conn: &'a Connection,
}

fn to_file(r: &Row) -> rusqlite::Result<File> {
    Ok(File {
        id: r.get(0)?,
        path: r.get(1)?,
        language: r.get(2)?,
        mtime: r.get(3)?,
        size: r.get(4)?,
        lines: r.get(5)?,
    })
}

impl<'a> FileDao<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        Self { conn }
    }

    pub fn all(&self) -> Result<Vec<File>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, language, mtime, size, lines FROM files ORDER BY path")?;
        let rows = stmt.query_map([], to_file)?;
        rows.collect()
    }

    pub fn insert(&self, path: &str, language: &str, mtime: i64, size: i64, lines: u32) -> Result<FileId> {
        self.conn.execute(
            "INSERT INTO files (path, language, mtime, size, lines) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![path, language, mtime, size, lines],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Cascades to the file's symbols, call sites and edges.
    pub fn delete(&self, id: FileId) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
        Ok(())
    }
}
