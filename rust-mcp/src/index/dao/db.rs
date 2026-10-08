use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

use super::Result;

/// Bump when the schema changes. The index is a cache, so a mismatch rebuilds it from scratch.
const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE files (
    id       INTEGER PRIMARY KEY,
    path     TEXT NOT NULL UNIQUE,
    language TEXT NOT NULL,
    mtime    INTEGER NOT NULL,
    size     INTEGER NOT NULL,
    lines    INTEGER NOT NULL
);
CREATE TABLE symbols (
    id         INTEGER PRIMARY KEY,
    file_id    INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    detail     TEXT NOT NULL,
    owner      TEXT NOT NULL DEFAULT '',
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    UNIQUE (file_id, owner, name)
);
CREATE INDEX idx_symbols_name ON symbols(name);
CREATE TABLE call_sites (
    symbol_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    callee    TEXT NOT NULL,
    dotted    INTEGER NOT NULL
);
CREATE INDEX idx_call_sites_symbol ON call_sites(symbol_id);
CREATE TABLE edges (
    from_id INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    to_id   INTEGER NOT NULL REFERENCES symbols(id) ON DELETE CASCADE,
    kind    TEXT NOT NULL,
    PRIMARY KEY (from_id, to_id, kind)
);
CREATE INDEX idx_edges_to ON edges(to_id);
";

const DROP_ALL: &str = "
DROP TABLE IF EXISTS edges;
DROP TABLE IF EXISTS call_sites;
DROP TABLE IF EXISTS symbols;
DROP TABLE IF EXISTS files;
";

pub struct Db {
    pub conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;

        let version: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            conn.execute_batch(DROP_ALL)?;
            conn.execute_batch(SCHEMA)?;
            conn.execute_batch(&format!("PRAGMA user_version = {};", SCHEMA_VERSION))?;
        }
        Ok(Self { conn })
    }
}

/// Runs `f` inside a write transaction (BEGIN IMMEDIATE avoids reader-to-writer lock upgrades
/// failing when two mysid processes index the same workspace).
pub fn in_tx<T>(conn: &Connection, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    conn.execute_batch("BEGIN IMMEDIATE")?;
    match f(conn) {
        Ok(v) => {
            conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
