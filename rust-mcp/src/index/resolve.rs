use std::collections::{HashMap, HashSet};

use super::model::{CallSite, Edge, EdgeKind, Symbol, SymbolId};

/// Method names so common in std/collection APIs that a `.name(` call is almost never the
/// user's own method of that name; such calls only link within the same file.
const COMMON_METHODS: &[&str] = &[
    "push", "pop", "get", "set", "insert", "remove", "len", "is_empty", "new", "map", "iter", "clone",
    "unwrap", "contains", "join", "next", "take", "skip", "find", "filter", "collect", "from", "into",
    "to_string", "as_str", "trim", "split", "replace", "write", "read", "push_str", "extend", "clear",
    "first", "last", "keys", "values", "entry", "min", "max", "sort", "dedup", "flush", "lock", "open",
    "close", "run", "add", "delete", "update", "parse", "format", "print", "log", "then", "catch",
    "forEach", "includes", "append", "count", "start", "stop", "emit", "send", "call", "apply", "bind",
    "ok", "err", "default", "build", "init", "exists", "starts_with", "ends_with", "chars", "lines",
];

/// Turns raw call sites into CALLS edges. Prefers a same-file target; otherwise links only when
/// the name is unique workspace-wide, so common names (`new`, `get`) don't create noise.
pub fn resolve(symbols: &[Symbol], calls: &[CallSite]) -> Vec<Edge> {
    // In-memory accelerators; the persistent source of truth stays in SQLite.
    let by_id: HashMap<SymbolId, &Symbol> = symbols.iter().map(|s| (s.id, s)).collect();
    let mut by_name: HashMap<&str, Vec<&Symbol>> = HashMap::new();
    for s in symbols {
        by_name.entry(s.name.as_str()).or_default().push(s);
    }

    let mut seen: HashSet<(SymbolId, SymbolId)> = HashSet::new();
    let mut edges = Vec::new();

    for call in calls {
        let (Some(caller), Some(candidates)) = (by_id.get(&call.symbol_id), by_name.get(call.callee.as_str())) else {
            continue;
        };

        let same_file = candidates.iter().find(|c| c.file_id == caller.file_id);
        let target = same_file.or_else(|| {
            let common = call.dotted && COMMON_METHODS.contains(&call.callee.as_str());
            if candidates.len() == 1 && !common { candidates.first() } else { None }
        });

        if let Some(target) = target {
            if target.id != caller.id && seen.insert((caller.id, target.id)) {
                edges.push(Edge { from: caller.id, to: target.id, kind: EdgeKind::Calls });
            }
        }
    }
    edges
}
