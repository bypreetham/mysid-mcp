use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::model::SymbolKind;
use crate::parsers::{find_parser, MethodSignature, TypeInfo};

/// Identifiers followed by `(` that are language constructs or logging macros, never call targets.
const NOT_CALLS: &[&str] = &[
    "if", "for", "while", "switch", "match", "return", "catch", "fn", "def", "function", "class",
    "sizeof", "typeof", "new", "await", "async", "else", "loop", "when", "with", "assert", "super",
    "this", "self", "LOGD", "LOGE", "LOGI", "LOGW",
];

/// libc / POSIX functions: a plain call to one of these is never the project's own function of that name.
const LIBC: &[&str] = &[
    "strerror", "printf", "fprintf", "snprintf", "sprintf", "vsnprintf", "puts", "memcpy", "memset", "memmove", "memcmp", "malloc", "calloc",
    "realloc", "free", "strlen", "strcmp", "strncmp", "strcpy", "strncpy", "strcat", "strchr", "strstr", "fopen", "fclose", "fread", "fwrite",
    "fseek", "ftell", "rewind", "fflush", "open", "close", "read", "write", "ioctl", "poll", "select", "usleep", "sleep", "nanosleep",
    "clock_gettime", "gettimeofday", "getenv", "atoi", "atol", "abs", "exit", "abort", "perror", "errno", "pthread_create", "pthread_join",
    "mmap", "munmap", "dup", "pipe", "fcntl", "stat", "lseek", "unlink", "qsort", "bsearch",
];

/// A symbol as found in a file, before it has a database id.
pub struct ParsedSymbol {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: String,
    pub owner: Option<String>,
    pub start_line: u32,
    pub end_line: u32,
    /// Distinct `(callee, written as x.callee(), declared type of x)` found in the body; the type is empty when unknown,
    /// and `@jvm` for a call from native code to a Java/Kotlin method by name (`GetMethodID`).
    pub calls: Vec<(String, bool, String)>,
    /// Base classes and interfaces a class names (`class A : public B, C` -> B, C); empty for methods.
    pub bases: Vec<String>,
    /// False for a declaration with no block of its own (a C++ prototype, an `external` method).
    pub has_body: bool,
}

pub struct ParsedFile {
    pub lines: u32,
    pub symbols: Vec<ParsedSymbol>,
    /// The parser output as is (every type and method, overloads included), kept for `overview`.
    pub types: Vec<TypeInfo>,
    pub methods: Vec<MethodSignature>,
}

fn call_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(\.|->|::)?\b([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap())
}

/// The names after the colon (`class A : public B, C`), without access keywords, templates or namespaces.
fn names_in_list(list: &str) -> Vec<String> {
    list.split(',')
        .filter_map(|item| {
            let item = item.trim();
            let item = item.strip_prefix("where").unwrap_or(item).trim();
            let item = item.split('(').next().unwrap_or(item).split('<').next().unwrap_or(item);
            let word = item
                .split_whitespace()
                .filter(|w| !matches!(*w, "public" | "protected" | "private" | "virtual" | "final" | "internal" | "open" | "abstract"))
                .last()?;
            let name = word.rsplit(|c| c == ':' || c == '.').next()?.trim_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
            if name.is_empty() || !name.chars().next().map_or(false, |c| c.is_uppercase() || c == '_' || c.is_alphabetic()) {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect()
}

/// Base classes and interfaces named in a type's declaration (the line of the type and up to three after it, to the
/// first `{`): C++ `class A : public B`, Kotlin / C# / Swift `class A(x) : B(), I`, Java / TypeScript `extends B implements I`.
fn bases_of(lines: &[&str], t: &TypeInfo, language: &str) -> Vec<String> {
    if !matches!(t.kind.as_str(), "class" | "struct" | "interface" | "object" | "record" | "mixin") {
        return Vec::new();
    }
    let start = t.line.saturating_sub(1);
    let mut decl = String::new();
    for line in lines.iter().skip(start).take(4) {
        decl.push_str(line);
        decl.push(' ');
        if line.contains('{') || line.contains(';') {
            break;
        }
    }
    let Some(at) = decl.find(&t.name) else { return Vec::new() };
    let mut rest = decl[at + t.name.len()..].to_string();
    if let Some(i) = rest.find(|c| c == '{' || c == ';') {
        rest.truncate(i);
    }
    if language == "Dart" {
        // `class A extends B with M implements I`, `mixin M on B`: the mixins join the base list, `on` is the superclass.
        rest = rest.replace(" with ", ", ").replace(" on ", " extends ");
    }
    if rest.contains(" extends ") || rest.contains(" implements ") {
        let mut out = Vec::new();
        for kw in [" extends ", " implements "] {
            if let Some(i) = rest.find(kw) {
                let tail = &rest[i + kw.len()..];
                let end = tail.find(" implements ").unwrap_or(tail.len());
                out.extend(names_in_list(&tail[..end]));
            }
        }
        return out;
    }
    if language != "C/C++" {
        // Kotlin, C#, Swift: drop the constructor's parentheses (balanced) before looking for the colon.
        let mut flat = String::new();
        let mut depth = 0;
        for ch in rest.chars() {
            match ch {
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                _ if depth == 0 => flat.push(ch),
                _ => {}
            }
        }
        rest = flat;
    }
    // The first single colon (not `::`) starts the base list.
    let bytes: Vec<char> = rest.chars().collect();
    for i in 0..bytes.len() {
        if bytes[i] == ':' && bytes.get(i + 1) != Some(&':') && (i == 0 || bytes[i - 1] != ':') {
            let list: String = bytes[i + 1..].iter().collect();
            return names_in_list(&list);
        }
    }
    Vec::new()
}

fn include_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?m)^\s*#\s*include\s+"([^"]+)""#).unwrap())
}

/// `&Class::method` handed to `std::thread`, `std::bind`, a callback registry: the method is used (called later).
fn member_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"&\s*(?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Z][A-Za-z0-9_]*)::([A-Za-z_][A-Za-z0-9_]*)\s*(\()?").unwrap())
}

/// Kotlin / Java method references: `this::loop`, `Player::play`.
fn jvm_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\b(this|[A-Z][A-Za-z0-9_]*)::([a-z_][A-Za-z0-9_]*)\b").unwrap())
}

/// The headers a C/C++ source includes (quoted includes found next to it or in a parent folder) plus its same-named
/// header: member declarations (`Engine* mEngine;`) live there, and calls through them are in the .cpp.
fn included_text(path: &Path, content: &str) -> String {
    let mut out = String::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    seen.insert(path.to_path_buf());
    // (file, its text, how many includes deep): a derived class's header includes its base's, which declares the members.
    let mut queue: Vec<(PathBuf, String, usize)> = vec![(path.to_path_buf(), content.to_string(), 0)];
    while let Some((current, text, depth)) = queue.pop() {
        let Some(dir) = current.parent() else { continue };
        let mut wanted: Vec<PathBuf> = include_re().captures_iter(&text).map(|c| PathBuf::from(&c[1])).collect();
        if depth == 0 {
            if let Some(stem) = current.file_stem() {
                for ext in ["h", "hpp", "hh"] {
                    wanted.push(PathBuf::from(format!("{}.{}", stem.to_string_lossy(), ext)));
                }
            }
        }
        for inc in wanted {
            for ancestor in dir.ancestors().take(8) {
                let p = ancestor.join(&inc);
                if p.is_file() {
                    if seen.insert(p.clone()) {
                        if let Ok(t) = fs::read_to_string(&p) {
                            if t.len() < 300_000 && out.len() < 900_000 {
                                out.push_str(&t);
                                out.push('\n');
                                if depth < 2 {
                                    queue.push((p.clone(), t, depth + 1));
                                }
                            }
                        }
                    }
                    break;
                }
            }
        }
    }
    out
}

fn jni_callback_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"Get(?:Static)?MethodID\s*\([^"]*"([A-Za-z_][A-Za-z0-9_]*)""#).unwrap())
}

/// Declarations that give a name its type: `(pattern, type comes first)`. Used to find the type of a call's receiver.
fn type_env_patterns() -> &'static Vec<(Regex, bool)> {
    static P: OnceLock<Vec<(Regex, bool)>> = OnceLock::new();
    P.get_or_init(|| {
        vec![
            // std::shared_ptr<Engine> engine_
            (Regex::new(r"\b(?:shared_ptr|unique_ptr|weak_ptr|sp|RefPtr)\s*<\s*(?:[A-Za-z0-9_]+::)*([A-Z][A-Za-z0-9_]*)\s*>\s*[*&]*\s*([A-Za-z_][A-Za-z0-9_]*)").unwrap(), true),
            // Kotlin, TypeScript, Rust: name: Type
            (Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*:\s*(?:&\s*)?(?:mut\s+)?(?:[A-Za-z0-9_]+\.)*([A-Z][A-Za-z0-9_]*)\b").unwrap(), false),
            // Java, C++: Type name
            (Regex::new(r"\b([A-Z][A-Za-z0-9_]*)(?:<[^<>;(){}]*>)?(?:\s+[*&]*\s*|\s*[*&]+\s*)([a-z_][A-Za-z0-9_]*)\s*(?:=|;|,|\)|\{|\()").unwrap(), true),
            // name = Type(...)  /  name = new Type(...)
            (Regex::new(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(?:new\s+)?([A-Z][A-Za-z0-9_]*)\s*[(<{]").unwrap(), false),
        ]
    })
}

/// name -> declared type, for the whole file (fields, parameters, locals): `negotiator` -> `UsbFormatNegotiator`.
fn type_env(content: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for (re, type_first) in type_env_patterns() {
        for c in re.captures_iter(content) {
            let (ty, name) = if *type_first { (&c[1], &c[2]) } else { (&c[2], &c[1]) };
            if !matches!(name, "if" | "else" | "return" | "new" | "in" | "is" | "as" | "for" | "throw") {
                env.entry(name.to_string()).or_insert_with(|| ty.to_string());
            }
        }
    }
    env
}

/// The identifier right before the `.`, `->` or `::` that starts at byte `dot` (`negotiator` in `negotiator?.setup(`).
fn receiver_before(line: &str, dot: usize) -> Option<&str> {
    let head = line[..dot].trim_end_matches(|c| c == '?' || c == '!' || c == ' ');
    let mut start = head.len();
    for (i, ch) in head.char_indices().rev() {
        if ch.is_alphanumeric() || ch == '_' {
            start = i;
        } else {
            break;
        }
    }
    let ident = &head[start..];
    if ident.is_empty() || ident.chars().next().map_or(true, |c| c.is_ascii_digit()) { None } else { Some(ident) }
}

/// The receiver of a call: an identifier, or `ret:name` when it is the result of a call (`a->getDecoder()->setRate()`:
/// the type is whatever `getDecoder` returns, looked up when the calls are linked).
fn receiver_expr(line: &str, dot: usize) -> Option<String> {
    let head = line[..dot].trim_end_matches(|c| c == '?' || c == '!' || c == ' ');
    if head.ends_with(')') {
        let mut depth = 0i32;
        let mut open = None;
        for (i, ch) in head.char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth -= 1;
                    if depth == 0 {
                        open = Some(i);
                        break;
                    }
                }
                _ => {}
            }
        }
        return receiver_before(head, open?).map(|n| format!("ret:{}", n));
    }
    receiver_before(line, dot).map(String::from)
}

/// The declared type of a call's receiver: `this` is the enclosing type, a capitalised name is a type itself
/// (`Engine.create()`, `Engine::create()`), anything else is looked up in the file's declarations.
fn receiver_type(recv: Option<String>, owner: &Option<String>, env: &HashMap<String, String>) -> String {
    match recv.as_deref() {
        None | Some("super") => String::new(),
        Some("this") | Some("self") | Some("Self") => owner.clone().unwrap_or_default(),
        Some(r) if r.starts_with("ret:") => r.to_string(),
        Some(r) => env
            .get(r)
            .cloned()
            .unwrap_or_else(|| if r.chars().next().map_or(false, |c| c.is_uppercase()) { r.to_string() } else { String::new() }),
    }
}

fn jsx_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"<([A-Z][A-Za-z0-9_]*)").unwrap())
}

/// The Rust parser reports `impl Foo` / `impl<T> Trait for Foo<T>`; the owning type is `Foo`.
fn canonical_type(name: &str) -> String {
    match name.strip_prefix("impl ") {
        Some(rest) => {
            let target = rest.rsplit(" for ").next().unwrap_or(rest);
            target.split('<').next().unwrap_or(target).trim().to_string()
        }
        None => name.to_string(),
    }
}

/// Parses one file into symbols plus the calls found in each symbol's body.
/// None when the file has no parser or cannot be read as text.
pub fn parse_file(path: &Path) -> Option<ParsedFile> {
    let ext = path.extension()?.to_str()?.to_lowercase();
    let parser = find_parser(&ext)?;
    if parser.language_name() == "Markdown" {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let (types, methods) = parser.parse(&lines);

    let braces = parser.language_name() != "Python";
    let is_web = matches!(ext.as_str(), "js" | "jsx" | "ts" | "tsx");
    let cpp_lang = parser.language_name() == "C/C++";
    // Where the block search starts: after a C++ constructor's initializer list.
    let block_from = |lines: &[&str], start: usize| if cpp_lang { cpp_body_start(lines, start).unwrap_or(start) } else { start };

    // A symbol with no block of its own is bounded by the next symbol's start.
    let mut boundaries: Vec<usize> = types
        .iter()
        .map(|t| t.line)
        .chain(methods.iter().map(|m| m.start_line))
        .collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    let body_end = |start_line: usize| -> usize {
        let start = start_line.saturating_sub(1);
        let next_start = boundaries
            .iter()
            .find(|&&b| b > start_line)
            .map(|b| b - 1)
            .unwrap_or(lines.len());
        // The real block, nested symbols included: a component's JSX often sits after its inner
        // handlers, so cutting at the next symbol would credit calls to the wrong function.
        let block = if braces {
            brace_block_end(&lines, block_from(&lines, start), next_start)
        } else {
            indent_block_end(&lines, start)
        };
        block.unwrap_or(next_start).min(lines.len()).max(start_line)
    };
    // Whether the symbol has a block of its own (a prototype or an `external` method has none).
    let has_block = |start_line: usize| -> bool {
        if !braces {
            return true;
        }
        let start = start_line.saturating_sub(1);
        let next_start = boundaries.iter().find(|&&b| b > start_line).map(|b| b - 1).unwrap_or(lines.len());
        brace_block_end(&lines, block_from(&lines, start), next_start).is_some()
    };

    let mut symbols = Vec::new();

    for t in &types {
        symbols.push(ParsedSymbol {
            name: canonical_type(&t.name),
            kind: SymbolKind::Class,
            detail: t.kind.clone(),
            owner: None,
            start_line: t.line as u32,
            end_line: body_end(t.line) as u32,
            calls: Vec::new(),
            bases: bases_of(&lines, t, parser.language_name()),
            has_body: true,
        });
    }

    let is_cpp = parser.language_name() == "C/C++";
    let is_jvm = matches!(parser.language_name(), "Kotlin" | "Java");
    let env = if is_cpp { type_env(&format!("{}\n{}", content, included_text(path, &content))) } else { type_env(&content) };
    for m in &methods {
        let mut owner = m.parent.as_deref().map(canonical_type);
        let mut name = m.name.clone();
        // `void Engine::start() {}` in a .cpp file is the method `start` of `Engine`, like its header declaration.
        if is_cpp {
            if let Some((qualifier, short)) = m.name.rsplit_once("::") {
                if !short.is_empty() {
                    name = short.to_string();
                    owner = qualifier.rsplit("::").next().filter(|s| !s.is_empty()).map(String::from).or(owner);
                }
            }
        }
        let end = body_end(m.start_line);
        let start = m.start_line.saturating_sub(1);

        let mut seen: HashSet<(String, bool, String)> = HashSet::new();
        let mut calls: Vec<(String, bool, String)> = Vec::new();
        let mut add = |callee: &str, dotted: bool, recv_type: String| {
            // A method does not call itself by its own name, unless the call goes through a receiver of another type.
            let own_name = callee == name && (recv_type.is_empty() || Some(&recv_type) == owner.as_ref());
            if !own_name && seen.insert((callee.to_string(), dotted, recv_type.clone())) {
                calls.push((callee.to_string(), dotted, recv_type));
            }
        };

        for line in &lines[start..end] {
            let trimmed = line.trim();
            if trimmed.starts_with("//") || trimmed.starts_with("/*") || trimmed.starts_with('*') || trimmed.starts_with('#') {
                continue;
            }
            for cap in call_re().captures_iter(line) {
                let callee = &cap[2];
                if !NOT_CALLS.contains(&callee) {
                    // `std::move(x)`, `std::string(s)`: the standard library, never the project's own function.
                    if cap.get(1).map_or(false, |d| d.as_str() == "::") && receiver_before(line, cap.get(1).unwrap().start()) == Some("std") {
                        continue;
                    }
                    // libc names are skipped for C/C++ only: `read()` in Python or `open()` in JS may be the project's own.
                    if is_cpp && cap.get(1).is_none() && LIBC.contains(&callee) {
                        continue;
                    }
                    let recv_type = match cap.get(1) {
                        Some(dot) => receiver_type(receiver_expr(line, dot.start()), &owner, &env),
                        None => String::new(),
                    };
                    add(callee, cap.get(1).is_some(), recv_type);
                }
            }
            if is_cpp {
                for cap in member_ref_re().captures_iter(line) {
                    if cap.get(3).is_none() {
                        add(&cap[2], true, cap[1].to_string());
                    }
                }
            }
            if is_jvm {
                for cap in jvm_ref_re().captures_iter(line) {
                    let recv = if &cap[1] == "this" { owner.clone().unwrap_or_default() } else { cap[1].to_string() };
                    if &cap[2] != "class" {
                        add(&cap[2], true, recv);
                    }
                }
            }
            if is_web {
                for cap in jsx_re().captures_iter(line) {
                    add(&cap[1], false, String::new());
                }
            }
            if is_cpp {
                for cap in jni_callback_re().captures_iter(line) {
                    add(&cap[1], false, "@jvm".to_string());
                }
            }
        }

        let native = m.signature.split_whitespace().any(|w| w == "external" || w == "native");
        symbols.push(ParsedSymbol {
            name,
            kind: if owner.is_some() { SymbolKind::Method } else { SymbolKind::Function },
            detail: if native { "native".to_string() } else { m.kind.clone() },
            owner,
            start_line: m.start_line as u32,
            end_line: end as u32,
            calls,
            bases: Vec::new(),
            has_body: has_block(m.start_line),
        });
    }

    Some(ParsedFile { lines: lines.len() as u32, symbols, types, methods })
}

/// A C++ constructor with an initializer list (`Foo::Foo(int a) : mA{a}, mB(a) {`): the line where the body's `{`
/// is, skipping the `{...}` of brace initializers (`mA{a}`) that would otherwise end the block early. None when the
/// symbol has no initializer list (the ordinary block search applies).
fn cpp_body_start(lines: &[&str], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let (mut seen_params, mut init) = (false, false);
    let mut init_brace = 0i32;
    let mut prev = ' ';
    for (i, line) in lines.iter().enumerate().skip(start).take(30) {
        let chars: Vec<char> = line.chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let ch = chars[k];
            if init_brace > 0 {
                if ch == '{' {
                    init_brace += 1;
                } else if ch == '}' {
                    init_brace -= 1;
                }
                prev = ch;
                k += 1;
                continue;
            }
            if ch == '/' && chars.get(k + 1) == Some(&'/') {
                break;
            }
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        seen_params = true;
                    }
                }
                ':' if seen_params && depth == 0 && !init => {
                    if chars.get(k + 1) != Some(&':') && prev != ':' {
                        init = true;
                    }
                }
                '{' if depth == 0 && seen_params => {
                    if !init {
                        return None;
                    }
                    if prev.is_alphanumeric() || prev == '_' || prev == '>' {
                        init_brace = 1;
                    } else {
                        return Some(i);
                    }
                }
                ';' if depth == 0 && seen_params => return None,
                _ => {}
            }
            if !ch.is_whitespace() {
                prev = ch;
            }
            k += 1;
        }
    }
    None
}

/// Last line (exclusive) of the `{ ... }` block that opens before `opening_limit`
/// (the next symbol's line). None when the symbol has no block (e.g. a one-line arrow function).
fn brace_block_end(lines: &[&str], start: usize, opening_limit: usize) -> Option<usize> {
    let mut depth: i32 = 0;
    let mut opened = false;
    for (i, line) in lines.iter().enumerate().skip(start) {
        if !opened && i >= opening_limit {
            return None;
        }
        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    opened = true;
                }
                '}' => depth -= 1,
                _ => {}
            }
        }
        if opened && depth <= 0 {
            return Some(i + 1);
        }
    }
    None
}

/// Python: the body ends at the next non-blank line indented no deeper than the `def` line.
fn indent_block_end(lines: &[&str], start: usize) -> Option<usize> {
    let indent_of = |l: &str| l.len() - l.trim_start().len();
    let base = indent_of(lines.get(start)?);
    for (i, line) in lines.iter().enumerate().skip(start + 1) {
        if !line.trim().is_empty() && indent_of(line) <= base {
            return Some(i);
        }
    }
    Some(lines.len())
}
