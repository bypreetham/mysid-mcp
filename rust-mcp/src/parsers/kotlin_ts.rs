//! Proof of concept (cargo feature `treesitter`): Kotlin symbols from a real syntax tree instead of line regexes.
//!
//! Same output as `kotlin.rs` (types, and methods with owner, signature, parameters, return type), so the index above it does not
//! change. What differs is how it is found: a declaration is a tree node, so nesting, the end of a body, multi-line signatures
//! and text inside strings or comments are exact, not inferred from braces and line starts.

use super::{LanguageParser, MethodSignature, TypeInfo};
use tree_sitter::{Node, Parser, Tree};

/// The syntax tree of Kotlin source (None when the grammar cannot be loaded).
pub fn kotlin_tree(src: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_kotlin_ng::LANGUAGE.into()).ok()?;
    parser.parse(src, None)
}

pub struct KotlinTsParser;

/// The line (1-based) of the first syntax error the grammar reported, if any.
pub fn first_error_line(src: &str) -> Option<usize> {
    first_error(src).map(|(line, _)| line)
}

/// (line, what) of the first syntax error: an `ERROR` node, or a token the grammar had to invent (`MISSING x`).
fn find_error(node: Node) -> Option<(usize, String)> {
    if node.is_error() {
        return Some((node.start_position().row + 1, "ERROR".to_string()));
    }
    if node.is_missing() {
        return Some((node.start_position().row + 1, format!("MISSING {}", node.kind())));
    }
    let mut c = node.walk();
    for ch in node.children(&mut c) {
        if let Some(found) = find_error(ch) {
            return Some(found);
        }
    }
    None
}

pub fn first_error(src: &str) -> Option<(usize, String)> {
    find_error(kotlin_tree(src)?.root_node())
}

impl KotlinTsParser {
    /// The symbols of a file, and whether the grammar hit a syntax error in it. After an error the tree can be missing whole
    /// declarations (error recovery), so such a file is read by the regex parser instead: the result is never worse than before.
    pub fn parse_checked(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>, bool) {
        let src = lines.join("\n");
        if let Some(tree) = kotlin_tree(&src) {
            // `has_error()` can stay true with no error node in the tree (repaired input); only a real ERROR / MISSING node counts.
            if !tree.root_node().has_error() || find_error(tree.root_node()).is_none() {
                let (mut types, mut methods) = (Vec::new(), Vec::new());
                walk(tree.root_node(), src.as_bytes(), &mut Vec::new(), &mut types, &mut methods);
                return (types, methods, false);
            }
        }
        let (types, methods) = super::kotlin::KotlinParser.parse(lines);
        (types, methods, true)
    }
}

fn text<'a>(node: Node, src: &'a [u8]) -> &'a str {
    node.utf8_text(src).unwrap_or("")
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn name_of(node: Node, src: &[u8]) -> Option<(String, usize)> {
    let n = node.child_by_field_name("name")?;
    Some((text(n, src).to_string(), n.start_position().row + 1))
}

fn has_child_kind(node: Node, kind: &str) -> bool {
    let mut c = node.walk();
    let found = node.children(&mut c).any(|ch| ch.kind() == kind);
    found
}

fn first_child_of_kind<'t>(node: Node<'t>, kind: &str) -> Option<Node<'t>> {
    let mut c = node.walk();
    let found = node.children(&mut c).find(|ch| ch.kind() == kind);
    found
}

/// `(a: Int, b: String)` -> `a: Int, b: String`
fn params_of(node: Node, src: &[u8]) -> String {
    match first_child_of_kind(node, "function_value_parameters") {
        Some(p) => {
            let t = text(p, src).trim();
            squash(t.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(t))
        }
        None => String::new(),
    }
}

/// The type written after the parameters (`: Boolean`), not the receiver of an extension function (`String.shout()`).
fn return_type_of(node: Node, src: &[u8]) -> Option<String> {
    let params_end = first_child_of_kind(node, "function_value_parameters")?.end_byte();
    let mut c = node.walk();
    let found = node
        .children(&mut c)
        .find(|ch| ch.start_byte() >= params_end && ch.is_named() && ch.kind().ends_with("type"))
        .map(|ch| squash(text(ch, src)));
    found
}

/// Everything from the start of the declaration to its body: modifiers, name, parameters, return type.
fn signature_of(node: Node, src: &[u8]) -> String {
    let end = first_child_of_kind(node, "function_body").or_else(|| first_child_of_kind(node, "block")).map_or(node.end_byte(), |b| b.start_byte());
    squash(std::str::from_utf8(&src[node.start_byte()..end]).unwrap_or(""))
}

fn walk(node: Node, src: &[u8], owners: &mut Vec<String>, types: &mut Vec<TypeInfo>, methods: &mut Vec<MethodSignature>) {
    let mut pushed = false;
    match node.kind() {
        "class_declaration" | "object_declaration" | "companion_object" => {
            // An unnamed companion object adds no type: its members belong to the class around it.
            if let Some((name, line)) = name_of(node, src) {
                let kind = match node.kind() {
                    "class_declaration" if has_child_kind(node, "interface") => "interface",
                    "class_declaration" => "class",
                    _ => "object",
                };
                types.push(TypeInfo { name: name.clone(), kind: kind.to_string(), line });
                owners.push(name);
                pushed = true;
            }
        }
        "function_declaration" => {
            if let Some((name, line)) = name_of(node, src) {
                methods.push(MethodSignature {
                    name,
                    kind: "method".to_string(),
                    signature: signature_of(node, src),
                    params: params_of(node, src),
                    return_type: return_type_of(node, src),
                    start_line: line,
                    end_line: node.end_position().row + 1,
                    parent: owners.last().cloned(),
                });
            }
        }
        // Grammar quirk (found on real code, reported by no error node): after annotations, `private fun name(...) { ... }` is
        // sometimes read as an infix expression of the identifiers `private`, `fun` and a call `name(...) { ... }`. The shape is
        // unambiguous, so the declaration is recovered from it.
        "infix_expression" => {
            let mut c = node.walk();
            let kids: Vec<Node> = node.children(&mut c).collect();
            if let Some(i) = kids.iter().position(|k| k.kind() == "identifier" && text(*k, src) == "fun") {
                if let Some(call) = kids.get(i + 1).filter(|k| k.kind() == "call_expression") {
                    // `name(args) { body }` is a call of a call: the callee and the arguments are in the innermost one.
                    let mut inner = *call;
                    while let Some(first) = inner.child(0).filter(|c| c.kind() == "call_expression") {
                        inner = first;
                    }
                    let mut cc = inner.walk();
                    let parts: Vec<Node> = inner.children(&mut cc).collect();
                    if let (Some(callee), Some(args)) = (parts.first().filter(|n| n.kind() == "identifier"), parts.iter().find(|n| n.kind() == "value_arguments")) {
                        let raw = text(*args, src).trim();
                        methods.push(MethodSignature {
                            name: text(*callee, src).to_string(),
                            kind: "method".to_string(),
                            signature: squash(std::str::from_utf8(&src[node.start_byte()..args.end_byte()]).unwrap_or("")),
                            params: squash(raw.strip_prefix('(').and_then(|s| s.strip_suffix(')')).unwrap_or(raw)),
                            return_type: None,
                            start_line: kids[i].start_position().row + 1,
                            end_line: call.end_position().row + 1,
                            parent: owners.last().cloned(),
                        });
                    }
                }
            }
        }
        "secondary_constructor" => {
            methods.push(MethodSignature {
                name: "constructor".to_string(),
                kind: "constructor".to_string(),
                signature: signature_of(node, src),
                params: params_of(node, src),
                return_type: None,
                start_line: node.start_position().row + 1,
                end_line: node.end_position().row + 1,
                parent: owners.last().cloned(),
            });
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, owners, types, methods);
    }
    if pushed {
        owners.pop();
    }
}

impl LanguageParser for KotlinTsParser {
    fn language_name(&self) -> &'static str {
        "Kotlin"
    }

    fn extensions(&self) -> &[&'static str] {
        &["kt", "kts"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let (types, methods, _) = self.parse_checked(lines);
        (types, methods)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let lines: Vec<&str> = src.lines().collect();
        KotlinTsParser.parse(&lines)
    }

    const SAMPLE: &str = r#"package a

abstract class Engine(private val name: String) : Base(), Runnable {
    companion object Factory {
        fun create(n: String): Engine = Impl(n)
    }
    constructor(n: String) : this(n) { init() }
    fun start(track: Track, flags: Int = 0): Boolean {
        fun localHelper(x: Int) = x + 1
        val s = "fun fake(a: Int) { class Fake }"
        // fun commented() {}
        return true
    }
    fun String.shout(): String = uppercase()
    interface Listener { fun onDone(code: Int) }
    enum class Mode { A, B }
    fun last() {}
}

object Registry {
    fun get(): Engine? = null
}

fun topLevel(a: Int, b: String): Unit {}
"#;

    #[test]
    fn types_are_found_with_their_kind_and_nested_members_belong_to_the_right_owner() {
        let (types, methods) = parse(SAMPLE);
        let t: Vec<(String, String)> = types.iter().map(|t| (t.kind.clone(), t.name.clone())).collect();
        for want in [("class", "Engine"), ("object", "Factory"), ("interface", "Listener"), ("class", "Mode"), ("object", "Registry")] {
            assert!(t.contains(&(want.0.to_string(), want.1.to_string())), "missing {:?} in {:?}", want, t);
        }
        let parent = |n: &str| methods.iter().find(|m| m.name == n).unwrap_or_else(|| panic!("no {}", n)).parent.clone();
        assert_eq!(parent("create").as_deref(), Some("Factory"));
        assert_eq!(parent("start").as_deref(), Some("Engine"));
        assert_eq!(parent("onDone").as_deref(), Some("Listener"));
        assert_eq!(parent("last").as_deref(), Some("Engine"), "after nested types");
        assert_eq!(parent("get").as_deref(), Some("Registry"));
        assert_eq!(parent("topLevel"), None);
    }

    #[test]
    fn a_file_the_grammar_cannot_read_falls_back_to_the_regex_parser() {
        // Kotlin grammar 1.1 loses the enclosing class on this input (error recovery); the regex parser still finds everything.
        let src = "class A {\n    interface L { fun f(c: Int) }\n}\n\nobject R { fun g(): E? = null }\n";
        let lines: Vec<&str> = src.lines().collect();
        let (types, methods, _had_error) = KotlinTsParser.parse_checked(&lines);
        let names: Vec<&str> = types.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"A") && names.contains(&"L") && names.contains(&"R"), "{:?}", names);
        // (The regex parser only sees a `fun` at the start of a line, so members written on one line with their type are missed:
        // `f` and `g` are not asserted. That is one of the things the syntax tree fixes whenever the grammar can read the file.)
        let _ = methods;
        assert_eq!(first_error_line("class A {\n    fun f() {}\n}\n"), None);
    }

    #[test]
    fn a_declaration_the_grammar_reads_as_an_infix_expression_is_recovered() {
        let (_, methods) = parse("val x = foo(@Composable private fun P() { T { S() } })\n");
        let p = methods.iter().find(|m| m.name == "P").expect("P recovered");
        assert_eq!((p.params.as_str(), p.start_line, p.signature.as_str()), ("", 1, "private fun P()"));
    }

    #[test]
    fn code_inside_strings_and_comments_is_not_a_declaration() {
        let (types, methods) = parse(SAMPLE);
        assert!(methods.iter().all(|m| m.name != "fake" && m.name != "commented"), "{:?}", methods.iter().map(|m| &m.name).collect::<Vec<_>>());
        assert!(types.iter().all(|t| t.name != "Fake"));
    }

    #[test]
    fn parameters_and_return_types_are_exact_even_for_expression_bodies_and_extensions() {
        let (_, methods) = parse(SAMPLE);
        let m = |n: &str| methods.iter().find(|m| m.name == n).unwrap();
        assert_eq!((m("start").params.as_str(), m("start").return_type.as_deref()), ("track: Track, flags: Int = 0", Some("Boolean")));
        assert_eq!(m("create").return_type.as_deref(), Some("Engine"), "not 'Engine = Impl(n)'");
        assert_eq!(m("shout").return_type.as_deref(), Some("String"), "the receiver String is not the return type");
        assert_eq!(m("get").return_type.as_deref(), Some("Engine?"));
        assert_eq!(m("topLevel").params, "a: Int, b: String");
        assert!(m("start").signature.starts_with("fun start(track: Track") && !m("start").signature.contains('{'), "{}", m("start").signature);
        let c = methods.iter().find(|m| m.kind == "constructor").unwrap();
        assert_eq!((c.name.as_str(), c.params.as_str(), c.parent.as_deref()), ("constructor", "n: String", Some("Engine")));
    }
}
