use super::{gather_multiline_signature, LanguageParser, MethodSignature, TypeInfo, TypeScopes};
use regex::Regex;

pub struct CppParser;

impl LanguageParser for CppParser {
    fn language_name(&self) -> &'static str {
        "C/C++"
    }

    fn extensions(&self) -> &[&'static str] {
        &["cpp", "cc", "cxx", "c", "h", "hpp", "hxx"]
    }

    fn parse(&self, lines: &[&str]) -> (Vec<TypeInfo>, Vec<MethodSignature>) {
        let mut types = Vec::new();
        let mut methods = Vec::new();

        let type_re = Regex::new(
            r"^\s*(?:template\s*<[^>]*>\s*)?(class|struct|namespace|enum(?:\s+class)?)\s+([A-Za-z0-9_]+)",
        )
        .unwrap();
        let func_re = Regex::new(
            r"^\s*(?:template\s*<[^>]*>\s*)?(?:(?:virtual|static|inline|explicit|friend|constexpr)\s+)*(?:(?:const|unsigned|signed|struct|class|enum)\s+)*([A-Za-z0-9_:<>,*&~]+(?:\s*[*&]+)?)\s+([A-Za-z0-9_:]+)\s*\(",
        )
        .unwrap();
        // JNI entry points: `Java_com_x_Foo_bar(JNIEnv *env, ...)`, often on the line after `extern "C" JNIEXPORT jint JNICALL`.
        let jni_re = Regex::new(r#"^\s*(?:extern\s+"C"\s+)?(?:JNIEXPORT\s+\w+\s+JNICALL\s+)?(Java_[A-Za-z0-9_]+)\s*\("#).unwrap();
        // Constructors and destructors have no return type: `Foo::Foo(...)`, `Foo::~Foo()`, or `Foo(...)` / `~Foo()` inside `class Foo`.
        let ctor_re = Regex::new(r"^\s*(?:(?:explicit|inline|constexpr|virtual)\s+)*(?:([A-Za-z_][A-Za-z0-9_]*)::)?(~?)([A-Za-z_][A-Za-z0-9_]*)\s*\(").unwrap();
        let dtor_re = Regex::new(
            r"^\s*(?:virtual\s+)?~([A-Za-z0-9_]+)\s*\(",
        )
        .unwrap();

        let mut scopes = TypeScopes::new();

        for (idx, &line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("//")
                || trimmed.starts_with('*')
                || trimmed.starts_with("/*")
                || trimmed.starts_with('#')
            {
                continue;
            }

            if let Some(caps) = type_re.captures(trimmed) {
                let kind = caps.get(1).map_or("class", |m| m.as_str());
                let name = caps.get(2).map_or("", |m| m.as_str());
                if !name.is_empty() {
                    types.push(TypeInfo {
                        name: name.to_string(),
                        kind: kind.to_string(),
                        line: idx + 1,
                    });
                    scopes.enter(name);
                }
            }

            let current_type = scopes.current();

            // `: mRing(cap) {`, `, mFoo(x)`: a constructor's member initializers, not functions.
            let initializer = (trimmed.starts_with(':') && !trimmed.starts_with("::")) || trimmed.starts_with(',');
            let ctor = if initializer {
                None
            } else {
                ctor_re.captures(trimmed).filter(|c| {
                    let name = c.get(3).map_or("", |m| m.as_str());
                    match (c.get(1).map(|m| m.as_str()), current_type.as_deref()) {
                        (Some(q), _) => q == name,
                        (None, Some(cur)) => cur == name,
                        _ => false,
                    }
                })
            };

            if initializer {
                // nothing: the initializer list belongs to the constructor above
            } else if let Some(caps) = ctor {
                let tilde = caps.get(2).map_or("", |m| m.as_str());
                let name = caps.get(3).map_or("", |m| m.as_str());
                let full = match caps.get(1) {
                    Some(q) => format!("{}::{}{}", q.as_str(), tilde, name),
                    None => format!("{}{}", tilde, name),
                };
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: full,
                    kind: if tilde.is_empty() { "constructor" } else { "destructor" }.to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            } else if let Some(caps) = jni_re.captures(trimmed) {
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: caps[1].to_string(),
                    kind: "method".to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: None,
                });
            } else if let Some(caps) = func_re.captures(trimmed) {
                let ret = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let name = caps.get(2).map_or("", |m| m.as_str()).to_string();

                // `errno, strerror(errno));` is the continuation of a call's arguments, not a declaration.
                let is_continuation = ret.ends_with(',');
                // `return foo(x);`, `else bar();`, `delete p;`: statements, not return types.
                let is_statement = ret.starts_with(':')
                    || matches!(ret.as_str(), "return" | "else" | "delete" | "throw" | "case" | "goto" | "co_return" | "co_await" | "new" | "using" | "typedef" | "namespace");
                let is_keyword = is_continuation || is_statement || matches!(
                    name.as_str(),
                    "if" | "while" | "for" | "switch" | "catch" | "return"
                );
                let is_raii_lock = ret.contains("lock_guard")
                    || ret.contains("unique_lock")
                    || ret.contains("shared_lock")
                    || ret.contains("scoped_lock");

                if !is_keyword && !is_raii_lock {
                    let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                    methods.push(MethodSignature {
                        name,
                        kind: "method".to_string(),
                        signature: sig,
                        params,
                        return_type: if !ret.is_empty() { Some(ret) } else { None },
                        start_line: idx + 1,
                        end_line,
                        parent: current_type.clone(),
                    });
                }
            } else if let Some(caps) = dtor_re.captures(trimmed) {
                let name = caps.get(1).map_or("", |m| m.as_str()).to_string();
                let (sig, params, end_line) = gather_multiline_signature(lines, idx, '{');
                methods.push(MethodSignature {
                    name: format!("~{}", name),
                    kind: "destructor".to_string(),
                    signature: sig,
                    params,
                    return_type: None,
                    start_line: idx + 1,
                    end_line,
                    parent: current_type.clone(),
                });
            }
            scopes.end_line(line, lines.get(idx + 1).copied());
        }

        (types, methods)
    }
}
