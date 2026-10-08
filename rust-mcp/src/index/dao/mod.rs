//! Data access layer: the only code that knows about SQLite.
mod db;
mod edge_dao;
mod file_dao;
mod symbol_dao;

pub use db::{in_tx, Db};
pub use edge_dao::EdgeDao;
pub use file_dao::FileDao;
pub use symbol_dao::SymbolDao;

pub type Result<T> = rusqlite::Result<T>;
