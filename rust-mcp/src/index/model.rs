//! Domain model of the code index. Storage-agnostic: DAOs map these to and from SQLite rows.

pub type FileId = i64;
pub type SymbolId = i64;

#[derive(Clone, Debug)]
pub struct File {
    pub id: FileId,
    /// Workspace-relative path with `/` separators.
    pub path: String,
    #[allow(dead_code)] // persisted for upcoming per-language views
    pub language: String,
    /// Modification time (ms since epoch) and size at indexing time; used for change detection.
    pub mtime: i64,
    pub size: i64,
    #[allow(dead_code)] // persisted for `read`/`overview` totals
    pub lines: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolKind {
    Class,
    Function,
    Method,
}

impl SymbolKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SymbolKind::Class => "class",
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "class" => SymbolKind::Class,
            "method" => SymbolKind::Method,
            _ => SymbolKind::Function,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Symbol {
    pub id: SymbolId,
    pub file_id: FileId,
    pub name: String,
    pub kind: SymbolKind,
    /// The parser's own kind (`interface`, `struct`, `arrow_function`, ...).
    #[allow(dead_code)] // persisted for upcoming views (e.g. `[interface]` labels)
    pub detail: String,
    /// Owning type for methods; None for classes and free functions.
    pub owner: Option<String>,
    pub start_line: u32,
    #[allow(dead_code)] // persisted for `read` (exact body extents)
    pub end_line: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Calls,
}

impl EdgeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeKind::Calls => "CALLS",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Edge {
    pub from: SymbolId,
    pub to: SymbolId,
    pub kind: EdgeKind,
}

/// A call found in a symbol's body, before it is resolved to a target symbol.
#[derive(Clone, Debug)]
pub struct CallSite {
    pub symbol_id: SymbolId,
    pub callee: String,
    /// Written as `x.callee(...)`.
    pub dotted: bool,
}
