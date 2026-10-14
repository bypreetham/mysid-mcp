use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo};
use regex::Regex;

pub struct DartParser;

/// A type whose `{` has not been seen yet (the header runs over several lines).
struct Pending {
    name: String,
}

/// An open type: its name and the brace depth inside its body.
struct Open {
    name: String,
    body_depth: i32,
}

/// Blanks string literals and comments, so braces and parentheses inside them are not counted.
/// `in_triple` carries a multi-line `'''` / `"""` string (or a `/* */` comment) from line to line.
fn code_of(line: &str, in_triple: &mut Option<String>) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if let Some(close) = in_triple.clone() {
            let cl: Vec<char> = close.chars().collect();
            if chars[i..].starts_with(&cl) {
                i += cl.len();
                *in_triple = None;
            } else {
                i += 1;
            }
            continue;
        }
        let c = chars[i];
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            break;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            *in_triple = Some("*/".to_string());
            i += 2;
            continue;
        }
        if c == '\'' || c == '"' {
            let raw = i > 0 && chars[i - 1] == 'r';
            let triple = chars.get(i + 1) == Some(&c) && chars.get(i + 2) == Some(&c);
            if triple {
                *in_triple = Some(std::iter::repeat(c).take(3).collect());
                i += 3;
                continue;
            }
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && !raw {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

const NOT_DECLARATIONS: &[&str] = &[
    "if", "for", "while", "switch", "catch", "return", "await", "assert", "super", "this", "throw", "new", "else", "yield", "Function", "do", "try",
];

impl LanguageParser for DartParser {
    fn language_name(&self) -> &'static str {
        "Dart"
    }

    fn extensions(&self) -> &[&'static str] {
        &["dart"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^(?:(?:abstract|base|final|interface|sealed)\s+)*(class|mixin|enum)\s+(?:class\s+)?([A-Za-z_]\w*)",
        )
        .unwrap();
        let extension_re = Regex::new(r"^extension\s+(?:type\s+)?(?:([A-Za-z_]\w*)(?:<[^>]*>)?\s+)?(?:on\s+([A-Za-z_]\w*))?").unwrap();
        let typedef_re = Regex::new(r"^typedef\s+([A-Za-z_]\w*)").unwrap();
        let modifiers = r"(?:@\w+(?:\.\w+)?(?:\([^)]*\))?\s+)*(?:(?:external|static|abstract|const|covariant|final|late)\s+)*";
        // `Type name(`, `Future<void> name<T>(`, `Map<String, int>? name(`.
        let func_re = Regex::new(&format!(
            r"^{}([A-Za-z_][\w<>,?.\[\] ]*?)\s+([A-Za-z_]\w*)\s*(?:<[^>(]*>)?\s*\(",
            modifiers
        ))
        .unwrap();
        let accessor_re = Regex::new(&format!(
            r"^{}(?:([A-Za-z_][\w<>,?.\[\] ]*?)\s+)?(get|set)\s+([A-Za-z_]\w*)",
            modifiers
        ))
        .unwrap();
        // `Foo(`, `const Foo(`, `Foo.named(`, `factory Foo.named(`: checked against the open class's name.
        let ctor_re = Regex::new(&format!(r"^{}(?:factory\s+)?([A-Za-z_]\w*)(?:\.([A-Za-z_]\w*))?\s*\(", modifiers)).unwrap();

        let mut stack: Vec<Open> = Vec::new();
        let mut pending: Option<Pending> = None;
        let mut depth: i32 = 0;
        let mut parens: i32 = 0;
        let mut multi: Option<String> = None;

        for (idx, &line) in lines.iter().enumerate() {
            let at_start_multi = multi.is_some();
            let code = code_of(line, &mut multi);
            let trimmed = code.trim();
            let in_decl_position = !at_start_multi && parens == 0 && !trimmed.is_empty();
            let line_start_depth = depth;
            let owner_depth = stack.last().map(|o| o.body_depth).unwrap_or(0);

            if in_decl_position && pending.is_none() && depth == 0 {
                let mut declared: Option<(String, String)> = None;
                if let Some(c) = type_re.captures(trimmed) {
                    declared = Some((c[1].to_string(), c[2].to_string()));
                } else if trimmed.starts_with("extension") {
                    if let Some(c) = extension_re.captures(trimmed) {
                        if let Some(n) = c.get(1).or_else(|| c.get(2)) {
                            declared = Some(("extension".to_string(), n.as_str().to_string()));
                        }
                    }
                } else if let Some(c) = typedef_re.captures(trimmed) {
                    declared = Some(("typedef".to_string(), c[1].to_string()));
                }
                if let Some((kind, name)) = declared {
                    types.push(TypeInfo { name: name.clone(), kind, line: idx + 1 });
                    pending = Some(Pending { name });
                }
            }

            // Members and top-level functions: only where a declaration can stand (the type's own body, or the file top).
            if in_decl_position && pending.is_none() && line_start_depth == owner_depth {
                let parent = stack.last().map(|o| o.name.clone());
                let mut found: Option<(String, &str, Option<String>)> = None;
                if let Some(c) = accessor_re.captures(trimmed) {
                    let kind = if &c[2] == "get" { "getter" } else { "setter" };
                    found = Some((c[3].to_string(), kind, c.get(1).map(|m| m.as_str().trim().to_string())));
                } else if let Some(c) = func_re.captures(trimmed) {
                    let ret = c[1].trim().to_string();
                    let name = c[2].to_string();
                    let first_word = ret.split_whitespace().next().unwrap_or("");
                    if !NOT_DECLARATIONS.contains(&name.as_str()) && !NOT_DECLARATIONS.contains(&first_word) && ret != "factory" {
                        found = Some((name, "method", Some(ret)));
                    }
                } else if let (Some(owner), Some(c)) = (parent.as_deref(), ctor_re.captures(trimmed)) {
                    if &c[1] == owner {
                        let name = match c.get(2) {
                            Some(n) => format!("{}.{}", owner, n.as_str()),
                            None => owner.to_string(),
                        };
                        found = Some((name, "constructor", None));
                    }
                }
                if let Some((name, kind, return_type)) = found {
                    let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                    let sig = match sig.find("=>") {
                        Some(p) => sig[..p].trim().to_string(),
                        None => sig,
                    };
                    methods.push(MethodSignature {
                        name,
                        kind: kind.to_string(),
                        signature: sig,
                        params,
                        return_type,
                        start_line: idx + 1,
                        end_line,
                        parent,
                    });
                }
            }

            // Bookkeeping after the line: braces open and close types, parentheses hold multi-line headers together.
            let opens = code.matches('{').count() as i32;
            let closes = code.matches('}').count() as i32;
            parens += code.matches('(').count() as i32 - code.matches(')').count() as i32;
            if parens < 0 {
                parens = 0;
            }
            if let Some(p) = pending.take() {
                if opens > 0 {
                    stack.push(Open { name: p.name, body_depth: line_start_depth + 1 });
                } else if trimmed.ends_with(';') {
                    // `typedef X = Y;` and `class A = B with C;` have no body.
                } else {
                    pending = Some(p);
                }
            }
            depth += opens - closes;
            while stack.last().map_or(false, |o| depth < o.body_depth) {
                stack.pop();
            }
        }

        (types, methods)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"import 'package:flutter/material.dart';

typedef Callback = void Function(int);

void main() {
  runApp(const MyApp());
}

abstract class Shape extends Base with Mixin implements Drawable {
  final String label;
  Shape(this.label);
  Shape.named({required this.label}) : super();
  factory Shape.make() => Circle();

  double get area;
  set name(String v) { _name = v; }

  Future<void> draw(
    Canvas canvas, {
    Offset? at,
  }) async {
    if (at != null) {
      canvas.drawCircle(at, 1);
    }
    for (final x in items) {
      print('{ $x');
    }
  }

  @override
  Widget build(BuildContext context) => Text('}');
  static Map<String, List<int>> table() { return {}; }
}

mixin Walker on Animal {
  void walk() {}
}

extension StringX on String {
  String shout() => toUpperCase();
}

enum Color { red, green;
  String label() => name;
}

class Other {
  void m() {}
}
"#;

    fn parse() -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let lines: Vec<&str> = SRC.lines().collect();
        DartParser.parse(&lines)
    }

    #[test]
    fn types_include_classes_mixins_extensions_enums_and_typedefs() {
        let (types, _) = parse();
        let got: Vec<(String, String)> = types.iter().map(|t| (t.kind.clone(), t.name.clone())).collect();
        for want in [("typedef", "Callback"), ("class", "Shape"), ("mixin", "Walker"), ("extension", "StringX"), ("enum", "Color"), ("class", "Other")] {
            assert!(got.contains(&(want.0.to_string(), want.1.to_string())), "missing {:?} in {:?}", want, got);
        }
    }

    #[test]
    fn members_belong_to_their_type_and_statements_are_not_declarations() {
        let (_, m) = parse();
        let find = |n: &str| m.iter().find(|x| x.name == n).unwrap_or_else(|| panic!("no {} in {:?}", n, m.iter().map(|x| &x.name).collect::<Vec<_>>()));
        assert_eq!(find("main").parent, None);
        assert_eq!(find("Shape").kind, "constructor");
        assert_eq!(find("Shape.named").kind, "constructor");
        assert_eq!(find("Shape.make").kind, "constructor");
        assert_eq!(find("area").kind, "getter");
        assert_eq!(find("area").return_type.as_deref(), Some("double"));
        assert_eq!(find("name").kind, "setter");
        let draw = find("draw");
        assert_eq!((draw.parent.as_deref(), draw.return_type.as_deref()), (Some("Shape"), Some("Future<void>")));
        assert_eq!(find("build").parent.as_deref(), Some("Shape"), "after a method whose body holds strings with braces");
        assert_eq!(find("table").return_type.as_deref(), Some("Map<String, List<int>>"));
        assert_eq!(find("walk").parent.as_deref(), Some("Walker"));
        assert_eq!(find("shout").parent.as_deref(), Some("StringX"));
        assert_eq!(find("label").parent.as_deref(), Some("Color"));
        assert_eq!(find("m").parent.as_deref(), Some("Other"));
        // Calls inside bodies (`canvas.drawCircle(`, `print(`, `runApp(`) are never declarations.
        for stmt in ["drawCircle", "print", "runApp", "if", "for"] {
            assert!(m.iter().all(|x| x.name != stmt), "{} must not be a declaration", stmt);
        }
    }
}
