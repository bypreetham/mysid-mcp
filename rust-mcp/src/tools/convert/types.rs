//! TypeScript types -> Dart types, and interfaces -> Dart model classes.

use super::ast::*;
use std::collections::HashMap;

/// A field of an interface after `extends` has been folded in.
#[derive(Clone, Debug)]
pub struct DField {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
}

pub type Models = HashMap<String, Vec<DField>>;

/// Names that are types in the web world and have no Dart class here: they read as `dynamic`.
const DYNAMIC_TYPES: &[&str] = &[
    "any", "unknown", "object", "Object", "Function", "ErrorInfo", "Timestamp", "User", "UserCredential", "IDBDatabase", "IDBRequest", "IDBObjectStore", "IDBKeyRange",
    "HTMLElement", "HTMLDivElement", "HTMLInputElement", "HTMLButtonElement", "HTMLCanvasElement", "HTMLVideoElement", "FormEvent", "ChangeEvent", "MouseEvent", "KeyboardEvent",
    "React.FormEvent", "React.ChangeEvent", "React.MouseEvent", "React.KeyboardEvent", "React.SyntheticEvent", "SyntheticEvent", "File", "Blob", "FileReader", "Html5Qrcode",
    "ConfirmationResult", "RecaptchaVerifier", "Auth", "Firestore", "DocumentData", "QuerySnapshot", "DocumentSnapshot", "Unsubscribe", "ComponentType", "Element",
    "JSX.Element", "React.ReactElement", "ReactElement", "CryptoKey", "Uint8Array", "ArrayBuffer", "MediaStream", "CSSProperties", "React.CSSProperties", "Event", "T",
];

/// Names for a UI value that Flutter has as `Widget`.
const WIDGET_TYPES: &[&str] = &["ReactNode", "React.ReactNode", "ReactElement", "JSX.Element", "React.ReactElement", "Element"];

pub fn is_widget_type(t: &Ty) -> bool {
    match t {
        Ty::Name(n, _) => WIDGET_TYPES.contains(&n.as_str()),
        Ty::Union(v) => v.iter().any(is_widget_type),
        _ => false,
    }
}

fn is_nullish(t: &Ty) -> bool {
    matches!(t, Ty::Name(n, _) if n == "null" || n == "undefined" || n == "void")
}

pub fn is_string_literal_union(t: &Ty) -> bool {
    match t {
        Ty::Union(v) => v.iter().all(|x| matches!(x, Ty::Lit(l) if l.starts_with('\'')) || is_nullish(x)),
        Ty::Lit(l) => l.starts_with('\''),
        _ => false,
    }
}

/// `T | null | undefined` -> (T, true)
pub fn strip_null(t: &Ty) -> (Ty, bool) {
    match t {
        Ty::Union(v) => {
            let rest: Vec<Ty> = v.iter().filter(|x| !is_nullish(x)).cloned().collect();
            let had = rest.len() != v.len();
            match rest.len() {
                0 => (Ty::Any, true),
                1 => (rest.into_iter().next().unwrap(), had),
                _ => (Ty::Union(rest), had),
            }
        }
        other => (other.clone(), false),
    }
}

/// The Dart spelling of a type. `models` says which names are classes the converter writes.
pub fn dart_type(t: &Ty, models: &Models) -> String {
    match t {
        Ty::Any => "dynamic".to_string(),
        Ty::Lit(l) => {
            if l.starts_with('\'') {
                "String".to_string()
            } else if l == "true" || l == "false" {
                "bool".to_string()
            } else {
                "num".to_string()
            }
        }
        Ty::Array(inner) => format!("List<{}>", dart_type(inner, models)),
        Ty::Tuple(items) => {
            let all_same = items.windows(2).all(|w| dart_type(&w[0], models) == dart_type(&w[1], models));
            if all_same && !items.is_empty() { format!("List<{}>", dart_type(&items[0], models)) } else { "List<dynamic>".to_string() }
        }
        Ty::Union(_) => {
            let (inner, nullable) = strip_null(t);
            if is_string_literal_union(&inner) {
                return if nullable { "String?".to_string() } else { "String".to_string() };
            }
            if let Ty::Union(parts) = &inner {
                // `string | number` and the like: no common Dart type except dynamic
                let kinds: Vec<String> = parts.iter().map(|p| dart_type(p, models)).collect();
                if kinds.windows(2).all(|w| w[0] == w[1]) {
                    return format!("{}{}", kinds[0].trim_end_matches('?'), if nullable { "?" } else { "" });
                }
                return "dynamic".to_string();
            }
            let base = dart_type(&inner, models);
            if nullable && base != "dynamic" && base != "void" && !base.ends_with('?') { format!("{}?", base) } else { base }
        }
        Ty::Inter(parts) => {
            // a named model alone reads as the model; with extra fields of an object literal it is `dynamic` (read by name)
            if parts.iter().any(|p| matches!(p, Ty::Obj(_))) {
                return "dynamic".to_string();
            }
            parts.iter().find_map(|p| match p {
                Ty::Name(n, _) if models.contains_key(n) => Some(model_dart_name(n)),
                _ => None,
            }).unwrap_or_else(|| "dynamic".to_string())
        }
        Ty::Obj(fields) => {
            if fields.len() == 1 && fields[0].name == "[index]" {
                return dart_type(&fields[0].ty, models);
            }
            "Map<String, dynamic>".to_string()
        }
        Ty::Fn(params, ret) => {
            if params.is_empty() && matches!(**ret, Ty::Name(ref n, _) if n == "void") {
                return "VoidCallback".to_string();
            }
            let ps: Vec<String> = params.iter().enumerate().map(|(i, p)| {
                let ty = p.ty.as_ref().map_or("dynamic".to_string(), |t| dart_type(t, models));
                format!("{} {}", ty, match &p.pat {
                    Pat::Ident(n) => n.clone(),
                    _ => format!("p{}", i),
                })
            }).collect();
            let ret_t = dart_type(ret, models);
            format!("{} Function({})", if ret_t == "void" { "void".to_string() } else { ret_t }, ps.join(", "))
        }
        Ty::Name(n, args) => name_type(n, args, models),
    }
}

fn name_type(n: &str, args: &[Ty], models: &Models) -> String {
    let n = n.strip_prefix("React.").unwrap_or(n);
    match n {
        "string" | "String" => "String".to_string(),
        "number" | "Number" | "bigint" => "num".to_string(),
        "boolean" | "Boolean" => "bool".to_string(),
        "void" | "never" => "void".to_string(),
        "null" | "undefined" => "Null".to_string(),
        "Date" => "DateTime".to_string(),
        "Error" => "Exception".to_string(),
        "Promise" | "PromiseLike" => format!("Future<{}>", args.first().map_or("void".to_string(), |a| dart_type(a, models))),
        "Array" | "ReadonlyArray" => format!("List<{}>", args.first().map_or("dynamic".to_string(), |a| dart_type(a, models))),
        "Set" => format!("Set<{}>", args.first().map_or("dynamic".to_string(), |a| dart_type(a, models))),
        "Map" => format!("Map<{}, {}>", args.first().map_or("dynamic".to_string(), |a| dart_type(a, models)), args.get(1).map_or("dynamic".to_string(), |a| dart_type(a, models))),
        "Record" => format!("Map<{}, {}>", args.first().map_or("String".to_string(), |a| dart_type(a, models)), args.get(1).map_or("dynamic".to_string(), |a| dart_type(a, models))),
        "Partial" => match args.first() {
            Some(Ty::Name(m, _)) if models.contains_key(&format!("{}Partial", m)) => model_dart_name(&format!("{}Partial", m)),
            Some(a) => dart_type(a, models),
            None => "dynamic".to_string(),
        },
        "Required" | "Readonly" | "Pick" | "Omit" => args.first().map_or("dynamic".to_string(), |a| dart_type(a, models)),
        "ReactNode" | "ReactElement" | "Element" | "JSX.Element" => "Widget?".to_string(),
        "FC" | "FunctionComponent" | "Component" => "dynamic".to_string(),
        "LucideIcon" | "LucideProps" => "IconData".to_string(),
        _ if DYNAMIC_TYPES.contains(&n) => "dynamic".to_string(),
        _ if models.contains_key(n) => model_dart_name(n),
        // a single capital letter is a generic parameter
        _ if n.len() == 1 && n.chars().all(|c| c.is_ascii_uppercase()) => "dynamic".to_string(),
        _ => "dynamic".to_string(),
    }
}

/// The type of a parameter that may be missing (`T?`), without doubling an existing `?`.
pub fn nullable(t: &str) -> String {
    if t == "dynamic" || t.ends_with('?') || t == "void" { t.to_string() } else { format!("{}?", t) }
}

/// A TypeScript interface name that would clash with a Flutter class (`State`, `Key`, ...) gets `Model` appended.
pub fn model_dart_name(n: &str) -> String {
    const RESERVED: &[&str] = &["State", "Key", "Widget", "Color", "Size", "Offset", "Border", "Theme", "Icon", "Image", "Text", "Locale", "Duration", "Action", "Intent", "Route", "Page", "Notification", "Card", "Form"];
    if RESERVED.contains(&n) { format!("{}Model", n) } else { n.to_string() }
}

pub fn lower_camel(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(c) => c.to_lowercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

/// Dart reserved words cannot be field names.
pub fn safe_ident(n: &str) -> String {
    // only the true reserved words: `get`, `set`, `on`, `required`, ... are ordinary names in Dart
    const RESERVED: &[&str] = &[
        "assert", "break", "case", "catch", "class", "const", "continue", "default", "do", "else", "enum", "extends", "false", "final", "finally", "for", "if", "in", "is", "new", "null",
        "rethrow", "return", "super", "switch", "this", "throw", "true", "try", "var", "void", "while", "with",
    ];
    // Flutter widget classes the output uses: a local of the same name would hide the class
    const WIDGETS: &[&str] = &["Icon", "Text", "Row", "Column", "Container", "Padding", "Center", "Align", "Stack", "Expanded", "Opacity", "Material", "InkWell", "Scaffold", "SizedBox", "Image", "Card", "Divider", "Wrap", "Form"];
    if WIDGETS.contains(&n) {
        return format!("{}_", n);
    }
    let cleaned: String = n.chars().map(|c| if c.is_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let cleaned = if cleaned.chars().next().map_or(true, |c| c.is_ascii_digit()) { format!("_{}", cleaned) } else { cleaned };
    if RESERVED.contains(&cleaned.as_str()) { format!("{}_", cleaned) } else { cleaned }
}

/// The attributes an HTML element's React type brings with it (`extends React.ButtonHTMLAttributes<..>`), as Dart fields.
pub fn html_attr_fields(base: &str) -> Vec<DField> {
    let f = |n: &str, t: &str, opt: bool| DField { name: n.to_string(), ty: Ty::Name(t.to_string(), Vec::new()), optional: opt };
    let cb = DField { name: "onClick".to_string(), ty: Ty::Fn(Vec::new(), Box::new(Ty::Name("void".to_string(), Vec::new()))), optional: true };
    let onchange = DField { name: "onChange".to_string(), ty: Ty::Fn(vec![Param { pat: Pat::Ident("e".to_string()), ty: Some(Ty::Any), default: None, optional: false, rest: false }], Box::new(Ty::Name("void".to_string(), Vec::new()))), optional: true };
    let base = base.strip_prefix("React.").unwrap_or(base);
    let mut out = vec![f("className", "string", true), f("id", "string", true)];
    if base.starts_with("Button") {
        out.extend([cb.clone(), f("disabled", "boolean", true), f("type", "string", true)]);
    } else if base.starts_with("Input") || base.starts_with("Textarea") || base.starts_with("Select") {
        out.extend([f("value", "any", true), onchange, f("placeholder", "string", true), f("disabled", "boolean", true), f("type", "string", true), f("name", "string", true), f("required", "boolean", true), f("maxLength", "number", true)]);
    } else {
        out.extend([cb]);
    }
    out
}

/// Folds `extends` into the interface's own fields, recursively, and reads intersections of a name with an object literal.
pub fn collect_models(items: &[&Item]) -> Models {
    let mut raw: HashMap<String, (Vec<Ty>, Vec<Field>)> = HashMap::new();
    for it in items {
        match it {
            Item::Interface { name, extends, fields, .. } => {
                raw.insert(name.clone(), (extends.clone(), fields.clone()));
            }
            Item::TypeAlias { name, ty, .. } => match ty {
                Ty::Obj(fields) => {
                    raw.insert(name.clone(), (Vec::new(), fields.clone()));
                }
                Ty::Inter(parts) => {
                    let mut ext = Vec::new();
                    let mut fields = Vec::new();
                    for p in parts {
                        match p {
                            Ty::Obj(f) => fields.extend(f.clone()),
                            other => ext.push(other.clone()),
                        }
                    }
                    raw.insert(name.clone(), (ext, fields));
                }
                _ => {}
            },
            _ => {}
        }
    }
    fn fold(name: &str, raw: &HashMap<String, (Vec<Ty>, Vec<Field>)>, depth: usize) -> Vec<DField> {
        let Some((ext, fields)) = raw.get(name) else { return Vec::new() };
        let mut out: Vec<DField> = Vec::new();
        if depth < 8 {
            for e in ext {
                if let Ty::Name(base, _) = e {
                    if raw.contains_key(base.as_str()) {
                        out.extend(fold(base, raw, depth + 1));
                    } else if base.contains("HTMLAttributes") {
                        out.extend(html_attr_fields(base));
                    }
                }
            }
        }
        for f in fields {
            if f.name == "[index]" {
                continue;
            }
            out.retain(|x| x.name != f.name);
            out.push(DField { name: f.name.clone(), ty: f.ty.clone(), optional: f.optional });
        }
        out
    }
    raw.keys().map(|k| (k.clone(), fold(k, &raw, 0))).collect()
}

/// The element type of `T[]` / `Array<T>`.
pub fn elem_type(t: &Ty) -> Option<Ty> {
    match strip_null(t).0 {
        Ty::Array(inner) => Some(*inner),
        Ty::Name(n, a) if (n == "Array" || n == "ReadonlyArray") && !a.is_empty() => a.into_iter().next(),
        _ => None,
    }
}

/// A Dart class for an interface: final fields, a const constructor with named parameters, `copyWith`, `fromMap`, `toMap`.
pub fn model_class(name: &str, fields: &[DField], models: &Models) -> String {
    let mut out = String::new();
    out.push_str(&format!("class {} {{\n", name));
    for f in fields {
        let t = field_type(f, models);
        out.push_str(&format!("  {} {};\n", t, safe_ident(&f.name)));
    }
    if fields.is_empty() {
        out.push_str(&format!("\n  {}();\n", name));
    } else {
        out.push_str(&format!("\n  {}({{\n", name));
        for f in fields {
            let t = field_type(f, models);
            let req = !f.optional && !t.ends_with('?') && t != "dynamic";
            out.push_str(&format!("    {}this.{},\n", if req { "required " } else { "" }, safe_ident(&f.name)));
        }
        out.push_str("  });\n");
        // copyWith keeps a field unless a value (null included) is passed
        out.push_str(&format!("\n  {} copyWith({{\n", name));
        for f in fields {
            out.push_str(&format!("    Object? {} = _keep,\n", safe_ident(&f.name)));
        }
        out.push_str("  }) {\n");
        out.push_str(&format!("    return {}(\n", name));
        for f in fields {
            let n = safe_ident(&f.name);
            let t = field_type(f, models);
            let cast = if t == "dynamic" { String::new() } else { format!(" as {}", t) };
            out.push_str(&format!("      {}: identical({}, _keep) ? this.{} : {}{},\n", n, n, n, n, cast));
        }
        out.push_str("    );\n  }\n");
        out.push_str(&format!("\n  factory {}.fromMap(Map<String, dynamic> m) {{\n    return {}(\n", name, name));
        for f in fields {
            let t = field_type(f, models);
            let n = safe_ident(&f.name);
            let key = &f.name;
            let value = if t == "dynamic" || t == "dynamic?" {
                format!("m['{}']", key)
            } else if let Some(model) = model_of(&f.ty, models) {
                let md = model_dart_name(&model);
                if t.ends_with('?') {
                    format!("m['{}'] is Map ? {}.fromMap(Map<String, dynamic>.from(m['{}'])) : null", key, md, key)
                } else {
                    format!("{}.fromMap(Map<String, dynamic>.from(m['{}'] as Map))", md, key)
                }
            } else if t.starts_with("List<") || t.starts_with("Map<") || t == "DateTime" || t == "DateTime?" || t.starts_with("Future") || t.contains("Function") {
                format!("m['{}']", key)
            } else {
                format!("m['{}'] as {}", key, if f.optional { nullable(&t) } else { t.clone() })
            };
            out.push_str(&format!("      {}: {},\n", n, value));
        }
        out.push_str("    );\n  }\n");
        out.push_str("\n  Map<String, dynamic> toMap() => {\n");
        for f in fields {
            let n = safe_ident(&f.name);
            let value = if model_of(&f.ty, models).is_some() { format!("{}{}.toMap()", n, if field_type(f, models).ends_with('?') { "?" } else { "" }) } else { n.clone() };
            // only a field that can be null is left out when it is
            if field_type(f, models).ends_with('?') {
                out.push_str(&format!("        if ({} != null) '{}': {},\n", n, f.name, value));
            } else {
                out.push_str(&format!("        '{}': {},\n", f.name, value));
            }
        }
        out.push_str("      };\n");
        // `obj[key]` / `obj[key] = value` by field name
        out.push_str("\n  dynamic operator [](String key) => toMap()[key];\n");
        out.push_str("\n  void operator []=(String key, dynamic value) {\n    switch (key) {\n");
        for f in fields {
            let t = field_type(f, models);
            let n = safe_ident(&f.name);
            let cast = if t == "dynamic" || t == "dynamic?" { String::new() } else { format!(" as {}", t) };
            out.push_str(&format!("      case '{}':\n        {} = value{};\n        break;\n", f.name, n, cast));
        }
        out.push_str("    }\n  }\n");
    }
    out.push_str("}\n");
    out
}

/// The model class a type names (looking through `| null`), if any.
pub fn model_of(t: &Ty, models: &Models) -> Option<String> {
    let (inner, _) = strip_null(t);
    // Omit / Pick / Required / Readonly of a model read as that model
    if let Ty::Name(n, a) = &inner {
        if matches!(n.as_str(), "Omit" | "Pick" | "Required" | "Readonly") {
            if let Some(first) = a.first() {
                return model_of(first, models);
            }
        }
    }
    match inner {
        Ty::Name(n, _) if models.contains_key(&n) => Some(n),
        Ty::Name(n, a) if n == "Partial" => match a.first() {
            Some(Ty::Name(m, _)) if models.contains_key(&format!("{}Partial", m)) => Some(format!("{}Partial", m)),
            Some(Ty::Name(m, _)) if models.contains_key(m) => Some(m.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// A field is nullable in Dart when it is optional or its type allows null.
pub fn field_type(f: &DField, models: &Models) -> String {
    let t = dart_type(&f.ty, models);
    if f.optional { nullable(&t) } else { t }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::convert::parse::parse_module;

    fn models_of(src: &str) -> Models {
        let m = parse_module(src);
        let items: Vec<&Item> = m.items.iter().collect();
        collect_models(&items)
    }

    #[test]
    fn types_map_to_dart() {
        let ms = models_of("interface A { x: number }");
        let t = |s: &str| {
            let m = parse_module(&format!("type T = {};", s));
            let Item::TypeAlias { ty, .. } = &m.items[0] else { panic!() };
            dart_type(ty, &ms)
        };
        assert_eq!(t("string | null"), "String?");
        assert_eq!(t("'admin' | 'guard' | null"), "String?");
        assert_eq!(t("string[]"), "List<String>");
        assert_eq!(t("Promise<void>"), "Future<void>");
        assert_eq!(t("A | null"), "A?");
        assert_eq!(t("() => void"), "VoidCallback");
        assert_eq!(t("(id: string) => void"), "void Function(String id)");
        assert_eq!(t("Record<string, number>"), "Map<String, num>");
        assert_eq!(t("React.ReactNode"), "Widget?");
        assert_eq!(t("string | number"), "dynamic");
    }

    #[test]
    fn interfaces_fold_their_bases_and_become_classes() {
        let ms = models_of("interface S { a: string; b?: number } interface C extends S { f: () => void } interface B extends React.ButtonHTMLAttributes<HTMLButtonElement> { variant?: 'x' | 'y' }");
        assert_eq!(ms["C"].iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), vec!["a", "b", "f"]);
        assert!(ms["B"].iter().any(|f| f.name == "onClick") && ms["B"].iter().any(|f| f.name == "variant"));
        let code = model_class("S", &ms["S"], &ms);
        assert!(code.contains("required this.a,") && code.contains("this.b,") && !code.contains("required this.b"), "{}", code);
        assert!(code.contains("S copyWith({") && code.contains("factory S.fromMap"));
    }
}
