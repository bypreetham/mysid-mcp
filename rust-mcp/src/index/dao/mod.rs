//! Data access layer: the only code that knows about SQLite.
mod db;
mod edge_dao;
mod file_dao;
mod signature_dao;
mod symbol_dao;

pub use db::{in_tx, Db};
pub use edge_dao::EdgeDao;
pub use file_dao::FileDao;
pub use signature_dao::{Outline, SignatureDao};
pub use symbol_dao::SymbolDao;

pub type Result<T> = rusqlite::Result<T>;

/// `IN (...)` lists are queried in slices of this size, well under SQLite's variable limit.
const IN_CHUNK: usize = 500;

fn placeholders(n: usize) -> String {
    vec!["?"; n].join(",")
}

/// Escapes `%`, `_` and `\` for use in `LIKE ... ESCAPE '\'`.
pub(crate) fn like_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

/// Runs `run` for each slice of `items` and concatenates the results (batched `IN` queries).
fn chunked<T, R>(items: &[T], mut run: impl FnMut(&[T]) -> Result<Vec<R>>) -> Result<Vec<R>> {
    let mut out = Vec::new();
    for chunk in items.chunks(IN_CHUNK) {
        out.extend(run(chunk)?);
    }
    Ok(out)
}
