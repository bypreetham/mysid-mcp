use rusqlite::{params, Connection, Row};
use std::collections::HashMap;

use super::{like_escape, Result};
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

    /// Language of every indexed file (by file id): which parser read it.
    pub fn languages(&self) -> Result<HashMap<FileId, String>> {
        let mut stmt = self.conn.prepare("SELECT id, language FROM files")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, FileId>(0)?, r.get::<_, String>(1)?)))?;
        rows.collect()
    }

    /// Files whose path is `name`, or ends with `/name` (a bare file name), case-insensitively.
    pub fn named(&self, name: &str) -> Result<Vec<File>> {
        let lower = name.to_lowercase();
        let tail = format!("%/{}", like_escape(&lower));
        let mut stmt = self.conn.prepare(
            "SELECT id, path, language, mtime, size, lines FROM files WHERE lower(path) = ?1 OR lower(path) LIKE ?2 ESCAPE '\\' ORDER BY path",
        )?;
        let rows = stmt.query_map(params![lower, tail], to_file)?;
        rows.collect()
    }

    pub fn count(&self) -> Result<usize> {
        self.conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0)).map(|n| n as usize)
    }

    /// True when `prefix` names an indexed file, or a directory containing indexed files.
    pub fn has_path(&self, prefix: &str) -> Result<bool> {
        let dir_pattern = format!("{}/%", like_escape(prefix));
        let mut stmt = self
            .conn
            .prepare_cached("SELECT 1 FROM files WHERE path = ?1 OR path LIKE ?2 ESCAPE '\\' LIMIT 1")?;
        stmt.exists(params![prefix, dir_pattern])
    }

    pub fn insert(&self, path: &str, language: &str, mtime: i64, size: i64, lines: u32) -> Result<FileId> {
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO files (path, language, mtime, size, lines) VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        stmt.execute(params![path, language, mtime, size, lines])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Cascades to the file's symbols, call sites and edges.
    pub fn delete(&self, id: FileId) -> Result<()> {
        self.conn.execute("DELETE FROM files WHERE id = ?1", params![id])?;
        Ok(())
    }
}
