use std::collections::{HashMap, HashSet};

use super::model::{CallSite, Cand, Edge, EdgeKind, FileId, Symbol, SymbolId};

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

/// `recv_type` of a call made from native code back into the JVM by name (`GetMethodID(cls, "onDevice", ...)`).
pub const JVM_CALLBACK: &str = "@jvm";

/// Languages whose code calls each other by plain name share a family. Everything else is its own:
/// a C++ function and a Kotlin method with the same name are unrelated. The bindings between them go through
/// strings, never through a call the name could resolve; JNI is linked separately (`jni_links`, `JVM_CALLBACK`).
fn language_family(language: &str) -> &str {
    match language {
        "Kotlin" | "Java" => "JVM",
        other => other,
    }
}

/// A header declaration and its definition (same owner, same name) are one function: keep the definition.
fn collapse_declarations(c: Vec<&Cand>) -> Vec<&Cand> {
    let defined: HashSet<Option<&str>> = c.iter().filter(|x| x.has_body).map(|x| x.owner.as_deref()).collect();
    c.into_iter().filter(|x| x.has_body || !defined.contains(&x.owner.as_deref())).collect()
}

/// The one linking rule, shared by full and incremental resolution.
///
/// - A call from native code by Java method name (`recv_type == "@jvm"`) links to the one JVM method of that name.
/// - Only symbols in the caller's language family are candidates, and a declaration gives way to its definition.
/// - A receiver whose declared type is known (`negotiator.setup()` with `negotiator: UsbFormatNegotiator`) links to the
///   method of that type, even when the caller's own file has a method of the same name.
/// - Otherwise prefer a same-file target, or link when the name is unique workspace-wide, so common names
///   (`new`, `get`) don't create noise.
///
/// `candidates` are all symbols with the callee's name, in id order.
#[cfg(test)]
pub fn pick_target(
    caller: (SymbolId, FileId),
    callee: &str,
    dotted: bool,
    recv_type: &str,
    candidates: &[Cand],
    languages: &HashMap<FileId, String>,
) -> Option<SymbolId> {
    pick_target_in(caller, None, callee, dotted, recv_type, candidates, languages)
}

/// `pick_target` for a caller that is a method of `caller_owner`: a plain call (`decode(n)`, no receiver) inside a method
/// means a method of the same class first.
pub fn pick_target_in(
    caller: (SymbolId, FileId),
    caller_owner: Option<&str>,
    callee: &str,
    dotted: bool,
    recv_type: &str,
    candidates: &[Cand],
    languages: &HashMap<FileId, String>,
) -> Option<SymbolId> {
    let family = |file: FileId| language_family(languages.get(&file).map_or("", |l| l.as_str()));
    if recv_type == JVM_CALLBACK {
        let jvm: Vec<&Cand> = candidates.iter().filter(|c| family(c.file) == "JVM").collect();
        return if jvm.len() == 1 { Some(jvm[0].id) } else { None };
    }
    let wanted = family(caller.1);
    let mut usable = collapse_declarations(candidates.iter().filter(|c| family(c.file) == wanted).collect());
    // A class and its constructor share a name: a call by that name (`new Foo(...)`) means the constructor.
    let ctors: Vec<&Cand> = usable.iter().filter(|c| c.owner.as_deref() == Some(callee)).copied().collect();
    if ctors.len() == 1 && usable.len() > 1 {
        usable = ctors;
    }

    if !recv_type.is_empty() {
        let typed: Vec<&&Cand> = usable.iter().filter(|c| c.owner.as_deref() == Some(recv_type)).collect();
        if let Some(t) = typed.iter().find(|c| c.file == caller.1).or_else(|| typed.first()) {
            return if t.id == caller.0 { None } else { Some(t.id) };
        }
    }

    if !dotted {
        if let Some(own) = caller_owner {
            let mine: Vec<&&Cand> = usable.iter().filter(|c| c.owner.as_deref() == Some(own)).collect();
            if let Some(t) = mine.iter().find(|c| c.file == caller.1).or_else(|| mine.first()) {
                return if t.id == caller.0 { None } else { Some(t.id) };
            }
        }
    }

    // A call on a receiver of a known type that matched nothing is a call on someone else's method: the same-file
    // method of that name is not what it means.
    let same_file = if dotted && !recv_type.is_empty() { None } else { usable.iter().find(|c| c.file == caller.1) };
    let target = same_file.or_else(|| {
        let common = dotted && COMMON_METHODS.contains(&callee);
        if usable.len() == 1 && !common { usable.first() } else { None }
    })?;
    if target.id == caller.0 { None } else { Some(target.id) }
}

/// JNI mangling of one name part: `_` becomes `_1`.
fn jni_mangle(s: &str) -> String {
    s.replace('_', "_1")
}

/// Edges from `external` / `native` methods to the JNI functions that implement them. A native method `open` of class
/// `UsbBridge` is `Java_<package>_UsbBridge_open` (plus `__<signature>` when overloaded); the package is not known,
/// so the name is matched by its ending. A definition is preferred over a declaration.
pub fn jni_links(natives: &[(SymbolId, String, String)], functions: &[(SymbolId, String, bool)]) -> Vec<(SymbolId, SymbolId)> {
    let mut out = Vec::new();
    for (id, owner, name) in natives {
        let suffix = if owner.is_empty() { format!("_{}", jni_mangle(name)) } else { format!("_{}_{}", jni_mangle(owner), jni_mangle(name)) };
        let overloaded = format!("{}__", suffix);
        let mut found: Vec<&(SymbolId, String, bool)> = functions.iter().filter(|f| f.1.ends_with(&suffix) || f.1.contains(&overloaded)).collect();
        if found.iter().any(|f| f.2) {
            found.retain(|f| f.2);
        }
        if let Some(f) = found.first() {
            out.push((*id, f.0));
        }
    }
    out
}

/// `AudioDecoder*`, `const IDecoder&`, `std::shared_ptr<IDecoder>`, `Player?` -> `IDecoder` / `Player`: the type a
/// method returns, for the receiver of a chained call.
pub fn simplify_type(t: &str) -> Option<String> {
    let mut s = t.trim();
    for wrapper in ["shared_ptr<", "unique_ptr<", "weak_ptr<", "sp<", "RefPtr<", "Optional<", "optional<"] {
        if let Some(i) = s.find(wrapper) {
            s = &s[i + wrapper.len()..];
            break;
        }
    }
    s.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == ':'))
        .filter_map(|tok| tok.rsplit("::").next())
        .find(|seg| seg.chars().next().map_or(false, |c| c.is_uppercase()))
        .map(String::from)
}

/// A receiver written `ret:getDecoder` (the result of a call) becomes the type that method returns, when all methods
/// of that name agree on one.
pub fn effective_recv(recv: &str, rets: &HashMap<String, String>) -> String {
    match recv.strip_prefix("ret:") {
        Some(name) => rets.get(name).cloned().unwrap_or_default(),
        None => recv.to_string(),
    }
}

/// At most this many overrides: a method with more (an interface with 29 implementations) is not fanned out.
pub const OVERRIDE_FANOUT: usize = 4;

/// Edges from a base class method to the methods of the same name in the classes derived from it (the nearest ancestor
/// that has the method is the one that links), for methods with few overrides. `bases` maps a class to the classes and
/// interfaces it names; `methods` is `(id, owner, name, has_body)`.
pub fn override_links(bases: &HashMap<String, Vec<String>>, methods: &[(SymbolId, String, String, bool)], max: usize) -> Vec<(SymbolId, SymbolId)> {
    // (owner, name) -> the method symbol: a definition rather than a header declaration.
    let mut by_class: HashMap<&str, HashMap<&str, (SymbolId, bool)>> = HashMap::new();
    for (id, owner, name, body) in methods {
        let entry = by_class.entry(owner.as_str()).or_default().entry(name.as_str()).or_insert((*id, *body));
        if *body && !entry.1 {
            *entry = (*id, true);
        }
    }
    let mut overrides: HashMap<SymbolId, HashSet<SymbolId>> = HashMap::new();
    for (class, _) in bases {
        // Ancestors, nearest first.
        let mut order: Vec<&str> = Vec::new();
        let mut queue: Vec<&str> = bases.get(class).map(|b| b.iter().map(String::as_str).collect()).unwrap_or_default();
        while !queue.is_empty() && order.len() < 12 {
            let next: Vec<&str> = queue.drain(..).collect();
            for a in next {
                if a != class && !order.contains(&a) {
                    order.push(a);
                    if let Some(up) = bases.get(a) {
                        queue.extend(up.iter().map(String::as_str));
                    }
                }
            }
        }
        let Some(own) = by_class.get(class.as_str()) else { continue };
        for (name, (id, _)) in own {
            if name.starts_with('~') || *name == "constructor" || *name == class.as_str() {
                continue;
            }
            for anc in &order {
                if let Some((aid, _)) = by_class.get(anc).and_then(|m| m.get(name)) {
                    if aid != id {
                        overrides.entry(*aid).or_default().insert(*id);
                    }
                    break;
                }
            }
        }
    }
    let mut out: Vec<(SymbolId, SymbolId)> = overrides.into_iter().filter(|(_, o)| o.len() <= max).flat_map(|(from, o)| o.into_iter().map(move |to| (from, to))).collect();
    out.sort();
    out
}

/// Resolves every call site against every symbol (used for the first index and big changes).
/// `rets` maps a method name to the one type all methods of that name return.
pub fn resolve(symbols: &[Symbol], calls: &[CallSite], languages: &HashMap<FileId, String>, rets: &HashMap<String, String>) -> Vec<Edge> {
    // In-memory accelerators; the persistent source of truth stays in SQLite.
    let file_of: HashMap<SymbolId, FileId> = symbols.iter().map(|s| (s.id, s.file_id)).collect();
    let owner_of: HashMap<SymbolId, &str> = symbols.iter().filter_map(|s| s.owner.as_deref().map(|o| (s.id, o))).collect();
    let mut by_name: HashMap<&str, Vec<Cand>> = HashMap::new();
    for s in symbols {
        by_name.entry(s.name.as_str()).or_default().push(Cand::from(s));
    }

    let mut seen: HashSet<(SymbolId, SymbolId)> = HashSet::new();
    let mut edges = Vec::new();
    for call in calls {
        let (Some(&caller_file), Some(candidates)) = (file_of.get(&call.symbol_id), by_name.get(call.callee.as_str())) else {
            continue;
        };
        let recv = effective_recv(&call.recv_type, rets);
        let caller_owner = owner_of.get(&call.symbol_id).copied();
        if let Some(to) = pick_target_in((call.symbol_id, caller_file), caller_owner, &call.callee, call.dotted, &recv, candidates, languages) {
            if seen.insert((call.symbol_id, to)) {
                edges.push(Edge { from: call.symbol_id, to, kind: EdgeKind::Calls });
            }
        }
    }
    edges
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand(id: SymbolId, file: FileId, owner: &str, body: bool) -> Cand {
        Cand { id, file, owner: if owner.is_empty() { None } else { Some(owner.to_string()) }, has_body: body }
    }

    fn langs() -> HashMap<FileId, String> {
        [(1, "Kotlin"), (2, "Kotlin"), (3, "C/C++"), (4, "C/C++")].iter().map(|(f, l)| (*f, l.to_string())).collect()
    }

    #[test]
    fn a_typed_receiver_links_to_that_types_method_not_the_same_file_one() {
        // UsbDirectOutput.setup (id 10, file 1) calls negotiator.setup; both classes define `setup`.
        let c = vec![cand(10, 1, "UsbDirectOutput", true), cand(20, 2, "UsbFormatNegotiator", true)];
        assert_eq!(pick_target((10, 1), "setup", true, "UsbFormatNegotiator", &c, &langs()), Some(20));
        // Unknown receiver type: the old rule (same-file wins, here a self call, so no edge).
        assert_eq!(pick_target((10, 1), "setup", true, "", &c, &langs()), None);
        // A known foreign type with no such method in the workspace: not linked to the same-file method.
        assert_eq!(pick_target((11, 1), "setup", true, "String", &c, &langs()), None);
    }

    #[test]
    fn a_header_declaration_gives_way_to_its_definition() {
        // Header prototype (id 1, no body) and the .cpp definition (id 2) of Engine::start; a Kotlin caller is another family.
        let c = vec![cand(1, 3, "Engine", false), cand(2, 4, "Engine", true)];
        assert_eq!(pick_target((9, 3), "start", true, "Engine", &c, &langs()), Some(2));
        assert_eq!(pick_target((9, 3), "start", false, "", &c, &langs()), Some(2));
    }

    #[test]
    fn native_code_calling_back_into_the_jvm_links_to_the_unique_method() {
        let c = vec![cand(5, 1, "UsbBridge", true)];
        assert_eq!(pick_target((9, 3), "onDevice", false, JVM_CALLBACK, &c, &langs()), Some(5));
        let two = vec![cand(5, 1, "A", true), cand(6, 2, "B", true)];
        assert_eq!(pick_target((9, 3), "onDevice", false, JVM_CALLBACK, &two, &langs()), None);
        // The same name in C++ is not a JVM method.
        let cpp = vec![cand(7, 4, "X", true)];
        assert_eq!(pick_target((9, 3), "onDevice", false, JVM_CALLBACK, &cpp, &langs()), None);
    }

    #[test]
    fn returned_types_are_simplified_for_chained_calls() {
        assert_eq!(simplify_type("AudioDecoder*").as_deref(), Some("AudioDecoder"));
        assert_eq!(simplify_type("const IDecoder&").as_deref(), Some("IDecoder"));
        assert_eq!(simplify_type("std::shared_ptr<hyperplay::IDecoder>").as_deref(), Some("IDecoder"));
        assert_eq!(simplify_type("Player?").as_deref(), Some("Player"));
        assert_eq!(simplify_type("int"), None);
        let rets: HashMap<String, String> = [("getDecoder".to_string(), "IDecoder".to_string())].into_iter().collect();
        assert_eq!(effective_recv("ret:getDecoder", &rets), "IDecoder");
        assert_eq!(effective_recv("ret:unknown", &rets), "");
        assert_eq!(effective_recv("Engine", &rets), "Engine");
    }

    #[test]
    fn a_plain_call_inside_a_method_prefers_the_same_class() {
        // BaseWorker.loop (id 1, file 3) calls decode(): the base declaration (id 2) and two overrides exist.
        let c = vec![cand(2, 4, "BaseWorker", false), cand(5, 4, "UsbWorker", true), cand(6, 4, "OboeWorker", true)];
        assert_eq!(pick_target_in((1, 3), Some("BaseWorker"), "decode", false, "", &c, &langs()), Some(2));
        // Without the owner the name is ambiguous.
        assert_eq!(pick_target_in((1, 3), None, "decode", false, "", &c, &langs()), None);
    }

    #[test]
    fn overrides_link_from_the_nearest_base_method_when_there_are_few() {
        let bases: HashMap<String, Vec<String>> = [
            ("Usb".to_string(), vec!["Base".to_string()]),
            ("Oboe".to_string(), vec!["Base".to_string()]),
            ("Base".to_string(), vec![]),
        ]
        .into_iter()
        .collect();
        let methods = vec![
            (1, "Base".to_string(), "decode".to_string(), false),
            (2, "Usb".to_string(), "decode".to_string(), true),
            (3, "Oboe".to_string(), "decode".to_string(), true),
            (4, "Usb".to_string(), "onlyUsb".to_string(), true),
        ];
        assert_eq!(override_links(&bases, &methods, 4), vec![(1, 2), (1, 3)]);
        // Too many overrides: not fanned out.
        assert!(override_links(&bases, &methods, 1).is_empty());
    }

    #[test]
    fn jni_names_are_matched_by_class_and_method() {
        let natives = vec![(1, "UsbBridge".to_string(), "open_device".to_string()), (2, "UsbBridge".to_string(), "close".to_string())];
        let funcs = vec![
            (10, "Java_com_acme_UsbBridge_open_1device".to_string(), true),
            (11, "Java_com_acme_UsbBridge_close".to_string(), false),
            (12, "Java_com_acme_UsbBridge_close".to_string(), true),
            (13, "Java_com_acme_Other_close".to_string(), true),
        ];
        let links = jni_links(&natives, &funcs);
        assert_eq!(links, vec![(1, 10), (2, 12)]);
    }
}
