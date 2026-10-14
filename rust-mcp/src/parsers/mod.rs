pub mod c_family;
pub mod csharp;
pub mod dart;
pub mod go;
pub mod java;
pub mod kotlin;
pub mod markdown;
pub mod python;
pub mod rust_lang;
pub mod swift;
pub mod web;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TypeInfo {
    pub name: String,
    pub kind: String, // "class", "interface", "struct", "enum", "trait", "object", "impl", "h1", "h2", etc.
    pub line: usize,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MethodSignature {
    pub name: String,
    pub kind: String, // "method", "constructor", "function", "heading", etc.
    pub signature: String,
    pub params: String,
    pub return_type: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub parent: Option<String>,
}

/// Bump whenever a parser's output or the call-linking rules change (symbols, owners, kinds, which
/// calls link). The code index stores it, so indexes built by an older version are rebuilt instead of
/// serving stale results.
pub const PARSER_VERSION: i32 = 14;

pub trait LanguageParser: Send + Sync {
    fn language_name(&self) -> &'static str;
    fn extensions(&self) -> &[&'static str];
    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>);
}

/// Helper function to gather signatures spanning multiple lines across language parsers
pub fn gather_multiline_signature(
    lines: &[&str],
    start_idx: usize,
    stop_char: char,
) -> (String, String, usize) {
    let mut accumulated = String::new();
    let mut end_idx = start_idx;
    let mut paren_depth = 0;
    let mut found_open_paren = false;

    let max_idx = (start_idx + 25).min(lines.len());

    for idx in start_idx..max_idx {
        let line = lines[idx].trim();
        if accumulated.is_empty() {
            accumulated.push_str(line);
        } else {
            accumulated.push(' ');
            accumulated.push_str(line);
        }
        end_idx = idx;

        for ch in line.chars() {
            if ch == '(' {
                paren_depth += 1;
                found_open_paren = true;
            } else if ch == ')' {
                paren_depth -= 1;
            }
        }

        if found_open_paren && paren_depth <= 0 {
            // Check if stop char is reached on this line or trailing type is completed
            if line.contains(stop_char)
                || line.contains('{')
                || line.contains(';')
                || paren_depth == 0
            {
                break;
            }
        }
    }

    // Extract parameters inside first outermost ()
    let params = if let Some(open_pos) = accumulated.find('(') {
        if let Some(close_pos) = accumulated.rfind(')') {
            if close_pos > open_pos {
                accumulated[open_pos + 1..close_pos]
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                String::new()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };

    // Clean up signature up to opening brace { or semicolon
    let clean_sig = if let Some(brace_pos) = accumulated.find('{') {
        accumulated[..brace_pos].trim().to_string()
    } else if let Some(semi_pos) = accumulated.find(';') {
        accumulated[..semi_pos].trim().to_string()
    } else {
        accumulated.trim().to_string()
    };

    (clean_sig, params, end_idx + 1)
}

static PARSERS: &[&dyn LanguageParser] = &[
    &kotlin::KotlinParser,
    &java::JavaParser,
    &c_family::CppParser,
    &rust_lang::RustParser,
    &python::PythonParser,
    &web::WebParser,
    &go::GoParser,
    &swift::SwiftParser,
    &csharp::CSharpParser,
    &dart::DartParser,
    &markdown::MarkdownParser,
];

pub fn find_parser(ext: &str) -> Option<&'static dyn LanguageParser> {
    let lower = ext.to_ascii_lowercase();
    PARSERS
        .iter()
        .copied()
        .find(|p| p.extensions().contains(&lower.as_str()))
}

pub fn is_supported_extension(ext: &str) -> bool {
    find_parser(ext).is_some()
}

/// Which type a line belongs to, tracked by brace nesting.
///
/// Parsers used to remember only the *last* type declared, so after a nested class ended, every
/// method of the outer class was attributed to the nested one. A stack with brace depth fixes that:
/// `enter` when a declaration is seen, `end_line` after each line, `current` for the innermost
/// type still open. A declaration with no body (`data class P(val a: Int)`) closes immediately.
pub struct TypeScopes {
    stack: Vec<Scope>,
    depth: i32,
}

struct Scope {
    name: String,
    /// Brace depth outside the type's own block.
    depth: i32,
    /// Its `{` has been seen.
    opened: bool,
    /// Unbalanced `(` while the declaration header is still being read.
    parens: i32,
}

impl TypeScopes {
    pub fn new() -> Self {
        Self { stack: Vec::new(), depth: 0 }
    }

    /// The innermost type that is still open.
    pub fn current(&self) -> Option<String> {
        self.stack.last().map(|s| s.name.clone())
    }

    /// Whether the line about to be processed sits directly inside the innermost open type's body (not in a method body).
    pub fn at_member_level(&self) -> bool {
        self.stack.last().map_or(false, |t| t.opened && self.depth == t.depth + 1)
    }

    /// A type declaration starts on the current line.
    pub fn enter(&mut self, name: &str) {
        self.stack.push(Scope { name: name.to_string(), depth: self.depth, opened: false, parens: 0 });
    }

    /// Call after processing each code line. `next` is the following line, used to tell
    /// `class A` + `{` on the next line (Allman style) from a declaration with no body.
    pub fn end_line(&mut self, line: &str, next: Option<&str>) {
        let code = strip_literals(line);
        let opens = code.matches('{').count() as i32;
        let closes = code.matches('}').count() as i32;

        if let Some(top) = self.stack.last_mut() {
            if !top.opened {
                top.parens += code.matches('(').count() as i32 - code.matches(')').count() as i32;
                if opens > 0 {
                    top.opened = true;
                }
            }
        }
        self.depth += opens - closes;

        while let Some(top) = self.stack.last() {
            if top.opened {
                if self.depth <= top.depth {
                    self.stack.pop();
                    continue;
                }
            } else {
                let tail = code.trim_end();
                let continues = top.parens > 0
                    || [",", ":", "(", "=", "->", "&"].iter().any(|c| tail.ends_with(c));
                let body_follows = next.map_or(false, |n| n.trim_start().starts_with('{'));
                if !continues && !body_follows {
                    self.stack.pop(); // no body: nothing can belong to it
                    continue;
                }
            }
            break;
        }
    }
}

/// The line with string/char literals blanked and any `//` comment removed, so braces and
/// parentheses inside them are not counted.
fn strip_literals(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        if c == '\'' {
            // A char literal is `'x'` or `'\x'`; anything else is an apostrophe and is kept.
            let close = if chars.get(i + 1) == Some(&'\\') { i + 3 } else { i + 2 };
            if chars.get(close) == Some(&'\'') {
                i = close + 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod scope_tests {
    use super::*;

    fn parents(lines: &[&str]) -> Vec<(String, Option<String>)> {
        let kotlin = kotlin::KotlinParser;
        let (_, methods) = kotlin.parse(lines);
        methods.into_iter().map(|m| (m.name, m.parent)).collect()
    }

    #[test]
    fn methods_after_a_nested_type_belong_to_the_outer_type_again() {
        let src = [
            "class Outer {",
            "    private object Store {",
            "        fun inStore() {}",
            "    }",
            "    data class Pair(val a: Int)",
            "    fun afterNested() {}",
            "    class Inner(",
            "        val x: Int,",
            "    ) : Base() {",
            "        fun innerFun() {}",
            "    }",
            "    fun last() {}",
            "}",
            "fun topLevel() {}",
        ];
        let got = parents(&src);
        let of = |n: &str| got.iter().find(|(m, _)| m == n).unwrap().1.clone();
        assert_eq!(of("inStore").as_deref(), Some("Store"));
        assert_eq!(of("afterNested").as_deref(), Some("Outer"), "after the nested object closed");
        assert_eq!(of("innerFun").as_deref(), Some("Inner"), "multi-line header with a body");
        assert_eq!(of("last").as_deref(), Some("Outer"));
        assert_eq!(of("topLevel"), None, "top level after the class closed");
    }

    #[test]
    fn braces_in_strings_and_chars_do_not_break_nesting() {
        let src = [
            "class A {",
            "    fun open() { val s = \"{{\"; val c = '{' }",
            "    fun next() {}",
            "}",
            "fun free() {}",
        ];
        let got = parents(&src);
        let of = |n: &str| got.iter().find(|(m, _)| m == n).unwrap().1.clone();
        assert_eq!(of("next").as_deref(), Some("A"));
        assert_eq!(of("free"), None);
    }

    #[test]
    fn allman_style_declarations_keep_their_scope_open() {
        let mut s = TypeScopes::new();
        s.enter("Foo");
        s.end_line("class Foo", Some("{"));
        assert_eq!(s.current().as_deref(), Some("Foo"), "brace is on the next line");
        s.end_line("{", Some("    void m() {}"));
        s.end_line("    void m() {}", Some("}"));
        assert_eq!(s.current().as_deref(), Some("Foo"));
        s.end_line("}", None);
        assert_eq!(s.current(), None);
    }

    #[test]
    fn cpp_nested_classes_forward_declarations_and_free_functions() {
        let cpp = c_family::CppParser;
        let (_, m) = cpp.parse(&[
            "class Fwd;",
            "class Outer {",
            "public:",
            "    class Inner {",
            "    public:",
            "        void innerFn() {}",
            "    };",
            "    void outerAfter() {}",
            "};",
            "void freeFn() {}",
        ]);
        let parent = |n: &str| m.iter().find(|x| x.name == n).unwrap().parent.clone();
        assert_eq!(parent("innerFn").as_deref(), Some("Inner"));
        assert_eq!(parent("outerAfter").as_deref(), Some("Outer"));
        assert_eq!(parent("freeFn"), None, "after the forward declaration and both classes closed");
    }

    #[test]
    fn java_and_csharp_nested_types_are_scoped_too() {
        let java = java::JavaParser;
        let (_, m) = java.parse(&[
            "public class Outer {",
            "    static class Nested {",
            "        public void a() {}",
            "    }",
            "    public void b() {}",
            "}",
        ]);
        let parent = |n: &str| m.iter().find(|x| x.name == n).unwrap().parent.clone();
        assert_eq!(parent("a").as_deref(), Some("Nested"));
        assert_eq!(parent("b").as_deref(), Some("Outer"));

        let cs = csharp::CSharpParser;
        let (_, m) = cs.parse(&[
            "public class Outer {",
            "    public class Nested {",
            "        public void A() {}",
            "    }",
            "    public void B() {}",
            "}",
        ]);
        let parent = |n: &str| m.iter().find(|x| x.name == n).unwrap().parent.clone();
        assert_eq!(parent("A").as_deref(), Some("Nested"));
        assert_eq!(parent("B").as_deref(), Some("Outer"));
    }
}
