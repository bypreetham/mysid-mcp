//! Expressions and statements: TypeScript -> Dart text.
//!
//! What has no Dart twin is written as a call into the compatibility file that ships with the output (`js_compat.dart`:
//! `jsTruthy`, `jsOr`, `errorMessage`, `LocalStorage`, `webOnly`, ...), so the converted code compiles and says at run time
//! which web-only call still needs a Flutter implementation.

use super::ast::*;
use super::cx::*;
use super::types::*;

/// A Dart single-quoted string literal for `s`.
pub fn str_lit(s: &str) -> String {
    let mut out = String::from("'");
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '$' => out.push_str("\\$"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The body of a Dart string (no quotes) for raw JS template text: escapes stay, `$` and `'` are escaped for Dart.
fn template_text(raw: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < cs.len() {
        match cs[i] {
            '\\' if i + 1 < cs.len() => {
                match cs[i + 1] {
                    '`' => out.push('`'),
                    '$' => out.push_str("\\$"),
                    '\n' => {}
                    c => {
                        out.push('\\');
                        out.push(c);
                    }
                }
                i += 2;
                continue;
            }
            '$' => out.push_str("\\$"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
        i += 1;
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Truth {
    Bool,
    BoolOpt,
    Str,
    StrOpt,
    Num,
    NumOpt,
    Presence,
    /// type unknown, name gives no hint: could be a flag or an object
    Maybe,
}

/// `user`, `snapshot`, `result`: names that hold an object, tested for presence.
pub fn object_name(n: &str) -> bool {
    const NAMES: &[&str] = &[
        "user", "data", "result", "response", "snap", "snapshot", "doc", "ref", "entry", "item", "error", "err", "info", "config", "ctx", "context", "record", "row", "profile", "apartment", "admin",
        "guard", "session", "token", "scanner", "stream", "video", "element", "node", "file", "blob", "db", "auth", "provider", "instance", "current", "target", "event", "e", "cred", "credential",
        "text", "name", "email", "phone", "id", "uid", "message", "label", "title", "value", "query", "state", "role", "key", "url", "path", "code",
    ];
    let l = n.to_lowercase();
    NAMES.contains(&l.as_str()) || ["user", "data", "snap", "ref", "result", "response", "entry", "item", "error", "info", "doc", "record", "scanner", "stream", "credential", "name", "id", "text", "url", "key", "role"].iter().any(|s| l.ends_with(s))
}

/// `isOpen`, `hasError`, `canEdit`: names that hold a flag.
pub fn bool_name(n: &str) -> bool {
    const PREFIXES: &[&str] = &["is", "has", "can", "should", "show", "enable", "enabled", "allow", "exists", "loading", "visible", "open", "valid", "active", "checked", "selected", "disabled", "ready", "done", "verified", "required", "scanned", "hidden", "locked", "present", "granted"];
    const SUFFIXES: &[&str] = &["Verified", "Enabled", "Disabled", "Active", "Visible", "Loading", "Valid", "Checked", "Selected", "Ready", "Done", "Open", "Closed", "Present", "Allowed", "Required", "Exists", "Scanned", "Granted", "Hidden", "Locked", "Expanded", "Collapsed", "Submitting", "Pending"];
    PREFIXES.iter().any(|p| n == *p || (n.starts_with(p) && n[p.len()..].chars().next().map_or(false, |c| c.is_uppercase() || c == '_'))) || SUFFIXES.iter().any(|s| n.ends_with(s))
}

/// Strips one pair of parentheses that wraps the whole text: `(a ?? b)` -> `a ?? b`.
pub fn unparen(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() < 2 || b[0] != b'(' || b[b.len() - 1] != b')' {
        return s.to_string();
    }
    let mut depth = 0i32;
    let mut in_str: Option<u8> = None;
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match in_str {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    in_str = None;
                }
            }
            None => match c {
                b'\'' | b'"' => in_str = Some(c),
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 && i != b.len() - 1 {
                        return s.to_string();
                    }
                }
                _ => {}
            },
        }
        i += 1;
    }
    s[1..s.len() - 1].to_string()
}

fn wrap_atom(code: &str) -> String {
    if code.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '.' | '!' | '(' | ')')) && !code.contains(' ') { code.to_string() } else { format!("({})", code) }
}

/// `$name` when `code` is a plain identifier that is not followed by an identifier character, else `${code}`.
pub fn interp(code: &str, next: Option<char>) -> String {
    let plain = code.chars().next().map_or(false, |c| c.is_alphabetic() || c == '_') && code.chars().all(|c| c.is_alphanumeric() || c == '_');
    let clash = next.map_or(false, |c| c.is_alphanumeric() || c == '_');
    if plain && !clash { format!("${}", code) } else { format!("${{{}}}", code) }
}

/// `Promise<T>` -> `T`: what an async function's `return` has to be.
pub fn unwrap_promise(t: &Ty) -> Ty {
    match t {
        Ty::Name(n, a) if n == "Promise" => a.first().cloned().unwrap_or(Ty::Any),
        other => other.clone(),
    }
}

fn is_event_call(name: &str) -> bool {
    matches!(name, "preventDefault" | "stopPropagation" | "stopImmediatePropagation")
}

impl<'a> Cx<'a> {
    // ---------------------------------------------------------------- small helpers

    pub fn ident(&mut self, n: &str) -> String {
        if let Some(r) = self.scope.renames.get(n) {
            return r.clone();
        }
        if let Some(orig) = self.import_alias.get(n) {
            return orig.clone();
        }
        if self.firebase.get(n).map(|o| o.as_str()) == Some("browserLocalPersistence") || self.firebase.get(n).map(|o| o.as_str()) == Some("browserSessionPersistence") {
            return "null".to_string();
        }
        match n {
            "NaN" => "double.nan".to_string(),
            "Infinity" => "double.infinity".to_string(),
            "localStorage" => "LocalStorage".to_string(),
            "sessionStorage" => "SessionStorage".to_string(),
            _ if self.lucide_imports.contains(n) && !self.names.contains(n) => {
                let real = self.lucide_alias.get(n).map(|s| s.as_str()).unwrap_or(n);
                super::widgets::lucide(real).map(String::from).unwrap_or_else(|| "Icons.circle_outlined".to_string())
            }
            "URL" | "Blob" | "FileReader" | "XMLHttpRequest" | "Notification" | "crypto" | "Intl" | "indexedDB" | "IDBKeyRange" | "TextEncoder" | "TextDecoder" | "Uint8Array" | "ArrayBuffer" | "DOMException" | "Audio" | "MediaRecorder" if !self.names.contains(n) => {
                self.note(format!("`{}` is a browser API: written as webOnly('{}')", n, n));
                format!("webOnly('{}')", n)
            }
            "window" | "document" | "navigator" | "globalThis" | "self" | "location" | "history" | "screen" | "performance" if !self.names.contains(n) => {
                self.note(format!("`{}` is a browser object: written as webOnly('{}') (needs a Flutter implementation)", n, n));
                format!("webOnly('{}')", n)
            }
            _ => safe_ident(n),
        }
    }

    /// The Dart type of an expression when it is cheap to know (a typed local, a field of a model).
    pub fn expr_type(&self, e: &Expr) -> Option<Ty> {
        match e {
            Expr::Ident(n) => self.type_of_name(n).cloned(),
            Expr::Member(obj, name, _) => {
                // this.x of a class: the field's declared type
                if matches!(**obj, Expr::This) {
                    return self.type_of_name(name).cloned();
                }
                // this.state.x / this.props.x of a class component: the field's declared type
                if let Expr::Member(this, which, _) = &**obj {
                    if matches!(**this, Expr::This) && (which == "state" || which == "props") {
                        return self.type_of_name(name).cloned();
                    }
                }
                let t = self.expr_type(obj)?;
                if matches!(&t, Ty::Name(n, _) if n == "__json") {
                    return Some(t);
                }
                let m = self.model_name(&t)?;
                self.info.models.get(&m)?.iter().find(|f| &f.name == name).map(|f| {
                    // an optional field may be missing
                    if f.optional && !matches!(f.ty, Ty::Union(_)) { Ty::Union(vec![f.ty.clone(), Ty::Name("undefined".to_string(), Vec::new())]) } else { f.ty.clone() }
                })
            }
            Expr::Index(obj, _, _) => match self.expr_type(obj) {
                Some(t @ Ty::Name(_, _)) if matches!(&t, Ty::Name(n, _) if n == "__json") => Some(t),
                Some(t) => elem_type(&t),
                None => None,
            },
            Expr::Str(_) | Expr::Template(..) => Some(Ty::Name("string".to_string(), Vec::new())),
            Expr::Num(_) => Some(Ty::Name("number".to_string(), Vec::new())),
            Expr::Bool(_) => Some(Ty::Name("boolean".to_string(), Vec::new())),
            Expr::Call(callee, _, _) => {
                if let Expr::Ident(n) = &**callee {
                    return self.info.funcs.get(n).and_then(|f| f.ret.clone());
                }
                // localStorage.getItem(..) is a string or null
                if let Expr::Member(obj, m, _) = &**callee {
                    if m == "getItem" && matches!(&**obj, Expr::Ident(s) if s == "localStorage" || s == "sessionStorage") {
                        return Some(Ty::Union(vec![Ty::Name("string".to_string(), Vec::new()), Ty::Name("null".to_string(), Vec::new())]));
                    }
                }
                // JSON.parse(...) and response.json(): untyped JSON
                if let Expr::Member(obj, m, _) = &**callee {
                    if (m == "parse" && matches!(&**obj, Expr::Ident(j) if j == "JSON")) || m == "json" {
                        return Some(Ty::Name("__json".to_string(), Vec::new()));
                    }
                }
                None
            }
            Expr::As(_, t) if matches!(t, Ty::Name(n, _) if n == "__targs") => None,
            Expr::As(_, t) => Some(t.clone()),
            Expr::Await(inner) => {
                let t = self.expr_type(inner)?;
                match t {
                    Ty::Name(n, a) if n == "Promise" => a.into_iter().next(),
                    other => Some(other),
                }
            }
            _ => None,
        }
    }

    fn is_boolish(&self, e: &Expr) -> bool {
        match e {
            Expr::Bool(_) => true,
            Expr::Unary(op, _) => op == "!",
            Expr::Binary(op, l, r) => match op.as_str() {
                "===" | "!==" | "==" | "!=" | "<" | ">" | "<=" | ">=" | "in" | "instanceof" => true,
                "&&" | "||" => self.is_boolish(l) && self.is_boolish(r),
                _ => false,
            },
            Expr::Call(callee, _, _) => matches!(&**callee, Expr::Member(_, m, _) if matches!(m.as_str(), "includes" | "startsWith" | "endsWith" | "some" | "every" | "has" | "test" | "isArray" | "isNaN" | "isInteger" | "contains" | "exists") || bool_name(m)),
            _ => matches!(self.expr_type(e), Some(Ty::Name(ref n, _)) if n == "boolean"),
        }
    }

    fn is_numeric(&self, e: &Expr) -> bool {
        match e {
            Expr::Num(_) | Expr::Update(..) => true,
            Expr::Unary(op, x) => op == "-" && self.is_numeric(x),
            Expr::Binary(op, l, r) => matches!(op.as_str(), "+" | "-" | "*" | "/" | "%") && !self.is_stringish(l) && !self.is_stringish(r),
            _ => matches!(self.expr_type(e), Some(Ty::Name(ref n, _)) if n == "number"),
        }
    }

    /// An identifier or member whose declared type allows null (read with `!` after TypeScript's own null check).
    fn is_nullable_ref(&self, e: &Expr) -> bool {
        matches!(e, Expr::Ident(_) | Expr::Member(..)) && self.expr_type(e).map_or(false, |t| strip_null(&t).1 && !matches!(strip_null(&t).0, Ty::Any))
    }

    /// A widget that may be null (`children`) returned or shown: an empty box when it is.
    pub fn nn_widget(&self, code: String) -> String {
        let plain = code.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
        if plain && (code.ends_with(".children") || self.scope.nullable_widgets.contains(&code)) { format!("({} ?? const SizedBox.shrink())", code) } else { code }
    }

    fn is_stringish(&self, e: &Expr) -> bool {
        match e {
            Expr::Str(_) | Expr::Template(..) => true,
            Expr::Binary(op, l, r) if op == "+" => self.is_stringish(l) || self.is_stringish(r),
            _ => matches!(self.expr_type(e), Some(Ty::Name(ref n, _)) if n == "string"),
        }
    }

    // ---------------------------------------------------------------- conditions

    /// An expression used as a condition: JavaScript truthiness written in Dart.
    /// What kind of value a condition tests: JavaScript's truthiness written as the explicit Dart test for that kind.
    fn truth_kind(&self, e: &Expr) -> Truth {
        if self.is_boolish(e) {
            return Truth::Bool;
        }
        if let Some(t) = self.expr_type(e) {
            let (inner, nullable) = strip_null(&t);
            return match &inner {
                Ty::Name(n, _) if n == "string" => if nullable { Truth::StrOpt } else { Truth::Str },
                Ty::Name(n, _) if n == "boolean" => if nullable { Truth::BoolOpt } else { Truth::Bool },
                Ty::Name(n, _) if n == "number" => if nullable { Truth::NumOpt } else { Truth::Num },
                _ => Truth::Presence,
            };
        }
        // no type known: a name like `isOpen` / `hasError` is a flag, anything else is tested for presence
        let name = match e {
            Expr::Ident(n) => Some(n.as_str()),
            Expr::Member(_, n, _) => Some(n.as_str()),
            _ => None,
        };
        match name {
            Some(n) if bool_name(n) => Truth::BoolOpt,
            Some(n) if object_name(n) => Truth::Presence,
            // an unknown thing that could be a flag or an object: `null` and `false` are both "no"
            Some(_) if matches!(e, Expr::Ident(_) | Expr::Member(..)) => Truth::Maybe,
            _ => Truth::Presence,
        }
    }

    /// An expression used as a condition.
    pub fn cond(&mut self, e: &Expr) -> String {
        match e {
            Expr::Binary(op, l, r) if op == "&&" || op == "||" => {
                // a nested `&&` / `||` keeps its parentheses; a single test does not need them
                let side = |cx: &mut Self, e: &Expr| {
                    let c = cx.cond(e);
                    if matches!(e, Expr::Binary(o, ..) if o == "&&" || o == "||") { c } else { unparen(&c) }
                };
                let (a, b) = (side(self, l), side(self, r));
                format!("({} {} {})", a, op, b)
            }
            Expr::Unary(op, x) if op == "!" => self.negate(x),
            _ => {
                let kind = self.truth_kind(e);
                let code = self.expr(e, None);
                match kind {
                    Truth::Bool => {
                        // an optional chain may give null: only `true` counts
                        if code.contains("?.") && !code.starts_with('!') { format!("({} == true)", code) } else { code }
                    }
                    Truth::BoolOpt => format!("({} == true)", code),
                    Truth::Str => format!("{}.isNotEmpty", code),
                    Truth::StrOpt => format!("({} ?? '').isNotEmpty", code),
                    Truth::Num => format!("({} != 0)", code),
                    Truth::NumOpt => format!("(({} ?? 0) != 0)", code),
                    Truth::Presence => format!("({} != null)", code),
                    Truth::Maybe => format!("({} != null && {} != false)", code, code),
                }
            }
        }
    }

    /// `!x` as the explicit test: `x == null`, `x.isEmpty`, `!flag`.
    fn negate(&mut self, x: &Expr) -> String {
        if let Expr::Binary(op, _, _) = x {
            if op == "&&" || op == "||" {
                return format!("!{}", self.cond(x));
            }
        }
        let kind = self.truth_kind(x);
        let code = self.expr(x, None);
        match kind {
            Truth::Bool => {
                if code.contains("?.") { format!("({} != true)", code) } else { format!("!{}", wrap_atom(&code)) }
            }
            Truth::BoolOpt => format!("({} != true)", code),
            Truth::Str => format!("{}.isEmpty", code),
            Truth::StrOpt => format!("({} ?? '').isEmpty", code),
            Truth::Num => format!("({} == 0)", code),
            Truth::NumOpt => format!("(({} ?? 0) == 0)", code),
            Truth::Presence => format!("({} == null)", code),
            Truth::Maybe => format!("({} == null || {} == false)", code, code),
        }
    }

    fn cond_atom(&mut self, e: &Expr) -> String {
        let c = self.cond(e);
        if c.starts_with('(') || c.chars().all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '.') || c.starts_with("jsTruthy(") { c } else { format!("({})", c) }
    }

    // ---------------------------------------------------------------- expressions

    pub fn expr(&mut self, e: &Expr, exp: Option<&Ty>) -> String {
        match e {
            // a map made by `...rest` where a model is expected
            Expr::Ident(n) if self.map_vars.contains(n) && exp.and_then(|t| self.model_name(t)).is_some() => {
                let m = exp.and_then(|t| self.model_name(t)).unwrap_or_default();
                format!("{}.fromMap({})", model_dart_name(&m), self.ident(n))
            }
            Expr::Ident(n) => self.ident(n),
            Expr::Num(n) => {
                if n.starts_with("0x") || n.starts_with("0X") || n.starts_with("0b") || n.starts_with("0B") || n.starts_with("0o") {
                    return if n.starts_with("0b") || n.starts_with("0B") { i64::from_str_radix(&n[2..], 2).map_or(n.clone(), |v| v.to_string()) } else { n.clone() };
                }
                if n.starts_with('.') { format!("0{}", n) } else if n.ends_with('.') { format!("{}0", n) } else { n.clone() }
            }
            Expr::Str(s) => str_lit(s),
            Expr::Bool(b) => b.to_string(),
            Expr::Null | Expr::Undefined => "null".to_string(),
            Expr::This => "this".to_string(),
            Expr::Template(q, ex) => {
                let mut out = String::from("'");
                for (i, quasi) in q.iter().enumerate() {
                    out.push_str(&template_text(quasi));
                    if let Some(x) = ex.get(i) {
                        let code = self.expr(x, None);
                        let next = q.get(i + 1).and_then(|n| template_text(n).chars().next());
                        out.push_str(&interp(&code, next));
                    }
                }
                out.push('\'');
                out
            }
            Expr::Array(items) => {
                let elem = exp.and_then(|t| match strip_null(t).0 {
                    Ty::Array(inner) => Some(*inner),
                    Ty::Name(n, a) if (n == "Array" || n == "ReadonlyArray") && !a.is_empty() => a.into_iter().next(),
                    _ => None,
                });
                let parts: Vec<String> = items.iter().map(|i| self.expr(i, elem.as_ref())).collect();
                format!("[{}]", parts.join(", "))
            }
            Expr::Object(props) => self.object(props, exp),
            Expr::Member(obj, name, opt) => self.member(obj, name, *opt),
            Expr::Index(obj, idx, opt) => {
                let o = self.expr(obj, None);
                let mut i = self.expr(idx, None);
                // a list index must be an int: arithmetic on numbers gives num
                if self.is_numeric(idx) && !matches!(**idx, Expr::Num(_)) {
                    i = format!("({}).toInt()", i);
                }
                format!("{}{}[{}]", o, if *opt { "?" } else { "" }, i)
            }
            Expr::Call(callee, args, opt) => self.call(callee, args, *opt, exp),
            Expr::New(callee, args) => self.new_expr(callee, args, exp),
            Expr::Unary(op, x) => match op.as_str() {
                "!" => format!("!{}", self.cond_atom(x)),
                "typeof" => format!("jsTypeof({})", self.expr(x, None)),
                "void" => "null".to_string(),
                "delete" => "null".to_string(),
                "-" | "+" | "~" => {
                    let inner = self.expr(x, None);
                    if matches!(**x, Expr::Num(_) | Expr::Ident(_) | Expr::Member(..)) { format!("{}{}", op, inner) } else { format!("{}({})", op, inner) }
                }
                _ => self.expr(x, None),
            },
            Expr::Update(op, x, prefix) => {
                let inner = self.expr(x, None);
                if *prefix { format!("{}{}", op, inner) } else { format!("{}{}", inner, op) }
            }
            Expr::Binary(op, l, r) => self.binary(op, l, r),
            Expr::Assign(op, l, r) => self.assign(op, l, r),
            Expr::Cond(c, t, f) => {
                let test = unparen(&self.cond(c));
                format!("({} ? {} : {})", test, self.expr(t, exp), self.expr(f, exp))
            }
            Expr::Arrow(f) => self.func(f, exp),
            Expr::Await(x) => format!("(await {})", self.expr(x, None)),
            Expr::Spread(x) => format!("...{}", self.expr(x, None)),
            Expr::NonNull(x) => {
                let inner = self.expr(x, exp);
                // `!` on a call result or a member keeps Dart's null check; on `dynamic` it is harmless
                format!("{}!", inner)
            }
            Expr::As(x, t) => {
                // casts are erased; an object literal cast to a model becomes that model
                if matches!(t, Ty::Name(n, _) if n == "__targs") {
                    return self.expr(x, exp);
                }
                // `snap.data() as Model`: a Map from the database becomes the model
                if let Some(m) = self.model_name(t) {
                    let from_call = matches!(&**x, Expr::Call(..) | Expr::Member(..) | Expr::Await(_) | Expr::Index(..));
                    let same = self.expr_type(x).and_then(|xt| self.model_name(&xt)).map_or(false, |xm| xm == m);
                    if from_call && !same {
                        let inner = unparen(&self.expr(x, None));
                        return format!("{}.fromMap({} as Map<String, dynamic>)", model_dart_name(&m), inner);
                    }
                }
                // `x as any`: the static type is dropped
                if matches!(t, Ty::Any) || matches!(t, Ty::Name(n, _) if n == "any" || n == "unknown") {
                    let inner = self.expr(x, None);
                    return if matches!(**x, Expr::Ident(_) | Expr::Member(..)) { format!("({} as dynamic)", inner) } else { inner };
                }
                self.expr(x, Some(t))
            }
            Expr::Jsx(j) => self.jsx_widget(j),
            Expr::Regex(r) => {
                let (body, flags) = r[1..].rsplit_once('/').unwrap_or((&r[1..], ""));
                format!("RegExp(r'{}'{})", body.replace('\'', "\\'"), if flags.contains('i') { ", caseSensitive: false" } else { "" })
            }
        }
    }

    fn binary(&mut self, op: &str, l: &Expr, r: &Expr) -> String {
        match op {
            // `a && b`: boolean logic when both are flags, else `b` when `a` holds
            "&&" => {
                if self.is_boolish(l) && self.is_boolish(r) {
                    return format!("({} && {})", self.expr(l, None), self.expr(r, None));
                }
                let test = self.cond(l);
                let rc = self.expr(r, None);
                if self.is_boolish(r) {
                    return format!("({} && {})", test, rc);
                }
                format!("({} ? {} : null)", unparen(&test), rc)
            }
            // `a || b`: a default for a missing value is `a ?? b`
            "||" => {
                if self.is_boolish(l) && self.is_boolish(r) {
                    return format!("({} || {})", self.expr(l, None), self.expr(r, None));
                }
                // a flag on the left makes it boolean logic, not a default value
                if matches!(self.truth_kind(l), Truth::Bool | Truth::BoolOpt) {
                    let (a, b) = (self.cond(l), self.cond(r));
                    return format!("({} || {})", unparen(&a), unparen(&b));
                }
                let lc = self.expr(l, None);
                let lc = if matches!(l, Expr::Binary(o, ..) if o == "||" || o == "??") { unparen(&lc) } else { lc };
                format!("({} ?? {})", lc, self.expr(r, None))
            }
            "??" => {
                let lc = self.expr(l, None);
                let lc = if matches!(l, Expr::Binary(o, ..) if o == "||" || o == "??") { unparen(&lc) } else { lc };
                format!("({} ?? {})", lc, self.expr(r, None))
            }
            "===" | "==" | "!==" | "!=" | ">" | "<" | ">=" | "<=" if matches!(l, Expr::Member(_, n, _) if n == "length") && matches!(r, Expr::Num(n) if n == "0" || n == "1") => {
                // `xs.length > 0` is `xs.isNotEmpty`
                let Expr::Member(obj, _, _) = l else { unreachable!() };
                let Expr::Num(n) = r else { unreachable!() };
                let recv = self.expr(obj, None);
                let recv = if self.is_nullable_ref(obj) { format!("{}!", recv) } else { recv };
                let empty = match (op, n.as_str()) {
                    ("===" | "==" | "<=", "0") | ("<", "1") => Some(true),
                    ("!==" | "!=" | ">", "0") | (">=", "1") => Some(false),
                    _ => None,
                };
                match empty {
                    Some(true) => format!("{}.isEmpty", recv),
                    Some(false) => format!("{}.isNotEmpty", recv),
                    None => format!("({}.length {} {})", recv, op.replace("===", "==").replace("!==", "!="), n),
                }
            }
            "===" | "==" | "!==" | "!=" => {
                let dart_op = if op.starts_with('!') { "!=" } else { "==" };
                // typeof x === 'string'
                if let (Expr::Unary(u, x), Expr::Str(s)) = (l, r) {
                    if u == "typeof" {
                        let test = match s.as_str() {
                            "string" => Some("String"),
                            "number" => Some("num"),
                            "boolean" => Some("bool"),
                            "function" => Some("Function"),
                            "object" => Some("Object"),
                            _ => None,
                        };
                        let inner = self.expr(x, None);
                        return match (s.as_str(), test) {
                            ("undefined", _) => format!("({} {} null)", inner, dart_op),
                            ("object", _) => format!("({} {} null && {} is! String && {} is! num && {} is! bool)", inner, if dart_op == "==" { "!=" } else { "==" }, inner, inner, inner).replace("&&", if dart_op == "==" { "&&" } else { "||" }),
                            (_, Some(t)) => format!("({} {} {})", inner, if dart_op == "==" { "is" } else { "is!" }, t),
                            _ => format!("(jsTypeof({}) {} {})", inner, dart_op, str_lit(s)),
                        };
                    }
                }
                format!("({} {} {})", self.expr(l, None), dart_op, self.expr(r, None))
            }
            "+" if self.is_stringish(l) || self.is_stringish(r) => {
                let (a, b) = (self.concat_part(l), self.concat_part(r));
                format!("'{}{}'", a, b)
            }
            "**" => format!("pow({}, {})", self.expr(l, None), self.expr(r, None)),
            "in" => format!("{}.containsKey({})", self.expr(r, None), self.expr(l, None)),
            "instanceof" => {
                let t = self.expr(r, None);
                format!("({} is {})", self.expr(l, None), if t == "Error" { "Exception".to_string() } else { t })
            }
            ">>>" => format!("({} >>> {})", self.expr(l, None), self.expr(r, None)),
            _ => {
                let (mut lc, mut rc) = (self.expr(l, None), self.expr(r, None));
                let arithmetic = matches!(op, "+" | "-" | "*" | "/" | "%");
                let relational = matches!(op, "<" | ">" | "<=" | ">=");
                if arithmetic {
                    if self.is_nullable_ref(l) {
                        lc = format!("{}!", lc);
                    }
                    if self.is_nullable_ref(r) {
                        rc = format!("{}!", rc);
                    }
                }
                // `undefined > 0` is false in JavaScript: NaN compares false in Dart too
                if relational {
                    let optional = |c: &str| c.contains("?.") || c.contains("?[");
                    if optional(&lc) {
                        lc = format!("({} ?? double.nan)", lc);
                    }
                    if optional(&rc) {
                        rc = format!("({} ?? double.nan)", rc);
                    }
                }
                format!("({} {} {})", lc, op, rc)
            }
        }
    }

    /// One piece of a string concatenation, as text that goes inside a Dart string literal.
    fn concat_part(&mut self, e: &Expr) -> String {
        match e {
            Expr::Str(s) => str_lit(s)[1..str_lit(s).len() - 1].to_string(),
            Expr::Template(q, ex) => {
                let lit = self.expr(&Expr::Template(q.clone(), ex.clone()), None);
                lit[1..lit.len() - 1].to_string()
            }
            Expr::Binary(op, l, r) if op == "+" && self.is_stringish(e) => format!("{}{}", self.concat_part(l), self.concat_part(r)),
            other => {
                let code = self.expr(other, None);
                format!("${{{}}}", code)
            }
        }
    }

    fn assign(&mut self, op: &str, l: &Expr, r: &Expr) -> String {
        // an assignment to a state field of a class component / a ref's current
        let target_ty = self.expr_type(l);
        let rhs = self.expr(r, target_ty.as_ref());
        let lhs = self.expr(l, None);
        // `x = x || []` is `x ??= []`
        if op == "=" {
            if let Expr::Binary(bop, bl, br) = r {
                if (bop == "||" || bop == "??") && self.expr(bl, None) == lhs {
                    let default = unparen(&self.expr(br, target_ty.as_ref()));
                    return format!("{} ??= {}", lhs, default);
                }
            }
        }
        match op {
            "||=" => format!("{} ??= {}", lhs, unparen(&rhs)),
            "&&=" => format!("{} = ({} != null ? {} : {})", lhs, lhs, rhs, lhs),
            "**=" => format!("{} = pow({}, {})", lhs, lhs, rhs),
            _ => format!("{} {} {}", lhs, op, rhs),
        }
    }

    fn member(&mut self, obj: &Expr, name: &str, opt: bool) -> String {
        // import.meta.env.X / process.env.X
        if let Expr::Member(inner, m, _) = obj {
            if let Expr::Member(root, meta, _) = &**inner {
                if matches!(&**root, Expr::Ident(r) if r == "import") && meta == "meta" && m == "env" {
                    return format!("const String.fromEnvironment({})", str_lit(name));
                }
            }
            if matches!(&**inner, Expr::Ident(r) if r == "process") && m == "env" {
                return format!("const String.fromEnvironment({})", str_lit(name));
            }
            // this.state.x / this.props.x
            if matches!(&**inner, Expr::This) && self.scope.class_component {
                if m == "state" {
                    return self.ident(name);
                }
                if m == "props" {
                    return format!("widget.{}", safe_ident(name));
                }
            }
            // e.target.value
            if let Expr::Ident(e) = &**inner {
                if m == "target" && self.scope.events.contains(e) && matches!(name, "value" | "checked" | "files") {
                    return self.ident(e);
                }
            }
        }
        if matches!(obj, Expr::This) {
            return safe_ident(name);
        }
        if let Expr::Ident(n) = obj {
            if self.catch_params.contains(n) {
                match name {
                    "message" => return format!("jsErrorMessage({})", n),
                    "code" => return format!("jsErrorCode({})", n),
                    _ => {}
                }
            }
            match (n.as_str(), name) {
                ("Number", "MAX_SAFE_INTEGER") => return "9007199254740991".to_string(),
                ("Number", "MIN_SAFE_INTEGER") => return "-9007199254740991".to_string(),
                ("Number", "EPSILON") => return "2.220446049250313e-16".to_string(),
                ("Math", "PI") => return "pi".to_string(),
                ("Math", "E") => return "e".to_string(),
                _ => {}
            }
        }
        // `(x as Other).field`: the static type says one thing and the object another: read the field by name
        if let Expr::As(inner, t) = obj {
            let is_targs = matches!(t, Ty::Name(n, _) if n == "__targs");
            if !is_targs {
                let target_model = self.model_name(t);
                let inner_model = self.expr_type(inner).and_then(|x| self.model_name(&x));
                if target_model.is_none() || target_model != inner_model {
                    return format!("jsProp({}, '{}')", self.expr(inner, None), name);
                }
            }
        }
        // a value of `Model & { extra }`: a model or a map, read by name
        if matches!(self.expr_type(obj), Some(Ty::Inter(_))) {
            return format!("jsProp({}, '{}')", self.expr(obj, None), name);
        }
        // UserCredential.user is nullable in FlutterFire
        if self.firebase_used && name == "user" && !opt && matches!(obj, Expr::Ident(_) | Expr::Await(_) | Expr::Call(..)) {
            return format!("{}.user!", self.expr(obj, None));
        }
        let mut o = self.expr(obj, None);
        if name == "message" && matches!(self.expr_type(obj), Some(Ty::Name(ref n, _)) if n == "Error") {
            return format!("jsErrorMessage({})", o);
        }
        if !opt && self.is_nullable_ref(obj) {
            o = format!("{}!", o);
        }
        // a field of an anonymous object type or a Map: an index, not a getter
        if let Some(t) = self.expr_type(obj) {
            let (inner, _) = strip_null(&t);
            let is_map = match &inner {
                Ty::Obj(_) => true,
                Ty::Name(n, _) => n == "Record" || n == "Map" || n == "__json",
                _ => false,
            };
            if is_map && !matches!(name, "length" | "keys" | "values" | "entries" | "isEmpty" | "isNotEmpty" | "containsKey") {
                return format!("{}{}['{}']", o, if opt { "?" } else { "" }, name);
            }
        }
        let name_out = match name {
            "size" if !self.firebase_used => "length".to_string(),
            "toString" => "toString".to_string(),
            other if other.starts_with('#') => format!("_{}", &other[1..]),
            other => safe_ident(other),
        };
        let name_out = if name_out.ends_with('_') && !name.ends_with('_') && matches!(name, "default" | "in" | "is" | "new" | "var" | "final" | "const" | "class" | "enum" | "do" | "get" | "set" | "late" | "required") { name.to_string() } else { name_out };
        format!("{}{}.{}", o, if opt { "?" } else { "" }, name_out)
    }

    fn args(&mut self, args: &[Expr], types: Option<&[(String, Option<Ty>, bool)]>) -> String {
        let parts: Vec<String> = args
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let exp = types.and_then(|t| t.get(i)).and_then(|p| p.1.clone());
                if let Expr::Spread(inner) = a {
                    self.note("a spread argument `...x` in a call has no Dart equivalent: passed as one list");
                    return self.expr(inner, None);
                }
                let mut c = unparen(&self.expr(a, exp.as_ref()));
                // a value that may be null passed where the parameter cannot take null (TypeScript checked it already)
                if let Some(pt) = &exp {
                    let unknown_member = matches!(a, Expr::Member(_, _, false)) && self.expr_type(a).is_none();
                    if (self.is_nullable_ref(a) || unknown_member) && !strip_null(pt).1 && !matches!(strip_null(pt).0, Ty::Any) && !c.ends_with('!') {
                        c.push('!');
                    }
                }
                c
            })
            .collect();
        self.closure_hint = None;
        parts.join(", ")
    }

    fn new_expr(&mut self, callee: &Expr, args: &[Expr], exp: Option<&Ty>) -> String {
        let name = match callee {
            Expr::Ident(n) => n.clone(),
            Expr::Member(_, n, _) => n.clone(),
            _ => "Object".to_string(),
        };
        let a = self.args(args, None);
        match name.as_str() {
            "Error" | "TypeError" | "RangeError" => format!("Exception({})", a),
            "Date" => format!("jsDate({})", a),
            "Set" => if a.is_empty() { "<dynamic>{}".to_string() } else { format!("Set<dynamic>.from({})", a) },
            "Map" => if a.is_empty() { "<dynamic, dynamic>{}".to_string() } else { format!("Map<dynamic, dynamic>.from({})", a) },
            "Array" => format!("List.filled({}, null, growable: true)", a),
            "RecaptchaVerifier" if self.firebase_used => {
                self.note("RecaptchaVerifier is web-only; on Android/iOS use FirebaseAuth.verifyPhoneNumber");
                "webOnly('RecaptchaVerifier')".to_string()
            }
            "Promise" => {
                self.note("`new Promise(...)` is written as a Completer: check the resolve/reject calls");
                format!("jsPromise({})", a)
            }
            "RegExp" => format!("RegExp({})", a),
            "URL" | "URLSearchParams" | "FileReader" | "Blob" | "XMLHttpRequest" | "WebSocket" | "Audio" | "Image" | "TextEncoder" | "TextDecoder" | "Uint8Array" | "ArrayBuffer" | "Intl" => {
                self.note(format!("`new {}` is a browser class: written as webOnly", name));
                format!("webOnly('new {}')", name)
            }
            _ => {
                let types: Option<Vec<(String, Option<Ty>, bool)>> = self.info.models.get(&name).map(|fs| fs.iter().map(|f| (f.name.clone(), Some(f.ty.clone()), f.optional)).collect());
                let _ = exp;
                let a = self.args(args, types.as_deref());
                format!("{}({})", if self.info.models.contains_key(&name) { model_dart_name(&name) } else { safe_ident(&name) }, a)
            }
        }
    }

    // ---------------------------------------------------------------- object literals

    fn object(&mut self, props: &[Prop], exp: Option<&Ty>) -> String {
        let model = exp.and_then(|t| self.model_name(t));
        // `{ id, ...snap.data() }` where a model is expected and the spread is a plain map: a map made into the model
        if let Some(m) = &model {
            let unknown_spread = props.iter().any(|p| matches!(p, Prop::Spread(e) if self.expr_type(e).and_then(|t| self.model_name(&t)).is_none()));
            if unknown_spread {
                let map = self.object(props, None);
                return format!("{}.fromMap({})", model_dart_name(m), map);
            }
        }
        // `{ ...a, x: 1 }` where `a` is another model than the one expected: the shared fields are copied over
        if let (Some(base), Some(em)) = (props.first().and_then(|p| if let Prop::Spread(e) = p { Some(e) } else { None }), model.clone()) {
            let sm = self.expr_type(base).and_then(|t| self.model_name(&t));
            if let Some(sm) = sm {
                if sm != em && props.iter().skip(1).all(|p| !matches!(p, Prop::Spread(_))) {
                    let efields = self.info.models.get(&em).cloned().unwrap_or_default();
                    let sfields = self.info.models.get(&sm).cloned().unwrap_or_default();
                    let base_code = self.expr(base, None);
                    let mut parts: Vec<(String, String)> = efields.iter().filter(|f| sfields.iter().any(|s| s.name == f.name)).map(|f| (safe_ident(&f.name), format!("{}.{}", base_code, safe_ident(&f.name)))).collect();
                    for p in &props[1..] {
                        let (key, val) = match p {
                            Prop::Kv(PropKey::Name(k), v) => (k.clone(), Some(v.clone())),
                            Prop::Short(k) => (k.clone(), None),
                            _ => continue,
                        };
                        let fty = efields.iter().find(|f| f.name == key).map(|f| f.ty.clone());
                        let v = match val {
                            Some(v) => self.expr(&v, fty.as_ref()),
                            None => self.ident(&key),
                        };
                        let k = safe_ident(&key);
                        match parts.iter_mut().find(|(n, _)| *n == k) {
                            Some(slot) => slot.1 = v,
                            None => parts.push((k, v)),
                        }
                    }
                    let args: Vec<String> = parts.into_iter().map(|(k, v)| format!("{}: {}", k, v)).collect();
                    return format!("{}({})", model_dart_name(&em), args.join(", "));
                }
            }
        }
        // `{ ...prev, a: 1 }` of a model: copyWith
        let spread_base = props.first().and_then(|p| match p {
            Prop::Spread(e) => Some(e),
            _ => None,
        });
        let base_model = spread_base.and_then(|e| self.expr_type(e)).and_then(|t| self.model_name(&t)).or_else(|| if spread_base.is_some() { model.clone() } else { None });
        if let (Some(base), Some(m)) = (spread_base, base_model.clone()) {
            if props.iter().skip(1).all(|p| !matches!(p, Prop::Spread(_))) {
                let fields = self.info.models.get(&m).cloned().unwrap_or_default();
                let mut parts = Vec::new();
                for p in &props[1..] {
                    let (key, val) = match p {
                        Prop::Kv(PropKey::Name(k), v) => (k.clone(), Some(v.clone())),
                        Prop::Short(k) => (k.clone(), None),
                        _ => continue,
                    };
                    let fty = fields.iter().find(|f| f.name == key).map(|f| f.ty.clone());
                    let v = match val {
                        Some(v) => self.expr(&v, fty.as_ref()),
                        None => self.ident(&key),
                    };
                    parts.push(format!("{}: {}", safe_ident(&key), v));
                }
                let base_code = self.expr(base, None);
                let base_code = if self.is_nullable_ref(base) { format!("{}!", base_code) } else { base_code };
                return format!("{}.copyWith({})", base_code, parts.join(", "));
            }
        }
        if let Some(m) = model {
            let fields = self.info.models.get(&m).cloned().unwrap_or_default();
            if props.iter().all(|p| matches!(p, Prop::Kv(PropKey::Name(_), _) | Prop::Short(_))) {
                let mut parts = Vec::new();
                for p in props {
                    let (key, val) = match p {
                        Prop::Kv(PropKey::Name(k), v) => (k.clone(), Some(v.clone())),
                        Prop::Short(k) => (k.clone(), None),
                        _ => continue,
                    };
                    let Some(fd) = fields.iter().find(|f| f.name == key) else { continue };
                    let v = match val {
                        Some(v) => self.expr(&v, Some(&fd.ty)),
                        None => self.ident(&key),
                    };
                    parts.push(format!("{}: {}", safe_ident(&key), v));
                }
                return format!("{}({})", model_dart_name(&m), parts.join(", "));
            }
        }
        let map_args = exp.and_then(|t| match strip_null(t).0 {
            Ty::Name(n, a) if (n == "Record" || n == "Map") && a.len() == 2 => Some((a[0].clone(), a[1].clone())),
            _ => None,
        });
        let value_ty = map_args.as_ref().map(|(_, v)| v.clone());
        let (key_dart, val_dart) = match &map_args {
            Some((k, v)) => (self.dty(k), self.dty(v)),
            None => ("String".to_string(), "dynamic".to_string()),
        };
        let mut parts = Vec::new();
        for p in props {
            match p {
                Prop::Kv(PropKey::Name(k), v) => parts.push(format!("{}: {}", str_lit(k), self.expr(v, value_ty.as_ref()))),
                Prop::Kv(PropKey::Computed(k), v) => parts.push(format!("{}: {}", self.expr(k, None), self.expr(v, value_ty.as_ref()))),
                Prop::Short(k) => {
                    let v = self.ident(k);
                    parts.push(format!("{}: {}", str_lit(k), v));
                }
                Prop::Spread(e) => parts.push(format!("...{}", self.expr(e, None))),
            }
        }
        format!("<{}, {}>{{{}}}", key_dart, val_dart, parts.join(", "))
    }

    // ---------------------------------------------------------------- calls

    fn call(&mut self, callee: &Expr, args: &[Expr], opt: bool, exp: Option<&Ty>) -> String {
        match callee {
            Expr::Ident(name) => self.call_ident(name, args),
            Expr::Member(obj, method, mopt) => self.call_method(obj, method, args, *mopt, exp),
            other => {
                let c = self.expr(other, None);
                let a = self.args(args, None);
                let paren = if matches!(other, Expr::Arrow(_)) { format!("({})", c) } else { c };
                format!("{}{}({})", paren, if opt { "?." } else { "" }, a)
            }
        }
    }

    fn call_ident(&mut self, name: &str, args: &[Expr]) -> String {
        if let Some(orig) = self.firebase.get(name).cloned() {
            if let Some(code) = self.firebase_call(&orig, args) {
                return code;
            }
            self.note(format!("Firebase `{}` has no FlutterFire mapping yet: left as a call", orig));
        }
        if let Some((state, ty)) = self.scope.setters.get(name).cloned() {
            return self.set_state_call(&state, ty.as_ref(), args);
        }
        if self.scope.nav.contains(name) {
            return self.navigate_call(args);
        }
        let a0 = |cx: &mut Self| args.first().map(|a| cx.expr(a, None)).unwrap_or_default();
        match name {
            "parseInt" => return format!("jsParseInt({})", a0(self)),
            "parseFloat" | "Number" => return format!("jsNumber({})", a0(self)),
            "String" => return format!("'${{{}}}'", a0(self)),
            "Boolean" => return format!("jsTruthy({})", a0(self)),
            "isNaN" => return format!("jsIsNaN({})", a0(self)),
            "setTimeout" => {
                let f = args.first().map(|a| self.expr(a, None)).unwrap_or_default();
                let ms = args.get(1).map(|a| self.expr(a, None)).unwrap_or_else(|| "0".to_string());
                return format!("jsSetTimeout({}, {})", f, ms);
            }
            "setInterval" => {
                let f = args.first().map(|a| self.expr(a, None)).unwrap_or_default();
                let ms = args.get(1).map(|a| self.expr(a, None)).unwrap_or_else(|| "0".to_string());
                return format!("jsSetInterval({}, {})", f, ms);
            }
            "clearTimeout" | "clearInterval" => return format!("jsClear({})", a0(self)),
            "fetch" => return format!("jsFetch({})", self.args(args, None)),
            "encodeURIComponent" | "encodeURI" => return format!("Uri.encodeComponent({})", a0(self)),
            "decodeURIComponent" | "decodeURI" => return format!("Uri.decodeComponent({})", a0(self)),
            "alert" | "confirm" | "prompt" | "requestAnimationFrame" | "cancelAnimationFrame" | "atob" | "btoa" | "structuredClone" | "queueMicrotask" => {
                self.note(format!("`{}` is a browser function: written as webOnly", name));
                return format!("webOnly('{}')", name);
            }
            "useContext" => {
                let c = a0(self);
                return format!("{}.of(context)", c);
            }
            "createContext" => return format!("ReactContext({})", a0(self)),
            "require" => return "null".to_string(),
            _ => {}
        }
        if matches!(name, "useLocation" | "useParams") {
            return format!("{}(context)", name);
        }
        if self.info.hooks.contains(name) {
            let a = self.args(args, None);
            return if a.is_empty() { format!("{}(context)", name) } else { format!("{}(context, {})", name, a) };
        }
        let sig = self.info.funcs.get(name).map(|f| f.params.clone());
        let a = self.args(args, sig.as_deref());
        format!("{}({})", self.ident(name), a)
    }

    /// `setX(v)` / `setX(prev => ...)`
    fn set_state_call(&mut self, state: &str, ty: Option<&Ty>, args: &[Expr]) -> String {
        let value = match args.first() {
            Some(Expr::Arrow(f)) if f.params.len() == 1 => {
                // the updater: call it with the current value, the parameter typed as the state
                let prev = match &f.params[0].pat {
                    Pat::Ident(n) => n.clone(),
                    _ => "prev".to_string(),
                };
                self.push_scope();
                self.declare(&prev, ty.cloned());
                let body = match &f.body {
                    FnBody::Expr(e) => format!("(({}) => {})({})", prev, self.expr(e, ty), state),
                    FnBody::Block(b) => {
                        let inner = self.block(b, 3);
                        format!("(({}) {{\n{}      }})({})", prev, inner, state)
                    }
                };
                self.pop_scope();
                body
            }
            Some(a) => self.expr(a, ty),
            None => "null".to_string(),
        };
        format!("setState(() {{ {} = {}; }})", state, value)
    }

    fn navigate_call(&mut self, args: &[Expr]) -> String {
        let to = args.first().map(|a| self.expr(a, None)).unwrap_or_else(|| "'/'".to_string());
        let mut replace = false;
        if let Some(Expr::Object(props)) = args.get(1) {
            for p in props {
                if let Prop::Kv(PropKey::Name(k), Expr::Bool(true)) = p {
                    if k == "replace" {
                        replace = true;
                    }
                }
            }
        }
        format!("navigate(context, {}{})", to, if replace { ", replace: true" } else { "" })
    }

    fn call_method(&mut self, obj: &Expr, method: &str, args: &[Expr], mopt: bool, exp: Option<&Ty>) -> String {
        let _ = exp;
        // this.setState({...}) of a class component
        if matches!(obj, Expr::This) {
            if method == "setState" && self.scope.class_component {
                return self.class_set_state(args);
            }
            let a = self.args(args, None);
            return format!("{}({})", safe_ident(method), a);
        }
        // e.preventDefault()
        if let Expr::Ident(n) = obj {
            if self.scope.events.contains(n) && is_event_call(method) {
                return "null".to_string();
            }
        }
        if let Expr::Ident(n) = obj {
            if let Some(code) = self.global_call(n, method, args) {
                return code;
            }
        }
        let mut recv = self.expr(obj, None);
        if !mopt && self.is_nullable_ref(obj) {
            recv = format!("{}!", recv);
        }
        // a value assigned in place: `(a[k] ??= []).add(x)`
        if matches!(obj, Expr::Assign(..)) {
            recv = format!("({})", recv);
        }
        let q = if mopt { "?" } else { "" };
        // the first parameter of a list callback is an element of the list
        if matches!(method, "map" | "filter" | "find" | "findIndex" | "some" | "every" | "forEach") {
            self.closure_hint = self.expr_type(obj).and_then(|t| elem_type(&t));
        }
        // a callback that takes the index too: the helpers pass it
        if matches!(method, "forEach" | "map" | "filter") && matches!(args.first(), Some(Expr::Arrow(f)) if f.params.len() >= 2) {
            let f = args.first().map(|x| self.expr(x, None)).unwrap_or_default();
            let helper = match method {
                "forEach" => "jsForEach",
                "map" => "jsMap",
                _ => "jsFilter",
            };
            return format!("{}({}, {})", helper, recv, f);
        }
        let a = |cx: &mut Self| cx.args(args, None);
        match method {
            "push" => format!("{}{}.add({})", recv, q, a(self)),
            "includes" => format!("{}{}.contains({})", recv, q, a(self)),
            "filter" => format!("{}{}.where({}).toList()", recv, q, a(self)),
            "map" => {
                // the list the map result becomes says what each callback result is (`T[]` -> a `T` per element)
                if let (Some(el), Some(Expr::Arrow(f))) = (exp.and_then(elem_type), args.first()) {
                    let closure_ty = Ty::Fn(Vec::new(), Box::new(el));
                    let c = self.func(f, Some(&closure_ty));
                    return format!("{}{}.map({}).toList()", recv, q, c);
                }
                format!("{}{}.map({}).toList()", recv, q, a(self))
            }
            "find" => format!("jsFind({}, {})", recv, a(self)),
            "findIndex" => format!("{}{}.indexWhere({})", recv, q, a(self)),
            "some" => format!("{}{}.any({})", recv, q, a(self)),
            "forEach" => format!("{}{}.forEach({})", recv, q, a(self)),
            "reduce" => {
                let f = args.first().map(|x| self.expr(x, None)).unwrap_or_default();
                let init = args.get(1).map(|x| self.expr(x, None)).unwrap_or_else(|| "null".to_string());
                format!("{}.fold({}, {})", recv, init, f)
            }
            "slice" => format!("jsSlice({}, {})", recv, a(self)),
            "splice" => format!("jsSplice({}, {})", recv, a(self)),
            "bind" => recv,
            "indexOf" | "lastIndexOf" | "split" | "join" | "trim" | "toLowerCase" | "toUpperCase" | "startsWith" | "endsWith" | "replaceAll" | "every" | "clear" | "contains" | "remove" | "add" | "has" => {
                let mapped = if method == "has" { "contains" } else { method };
                format!("{}{}.{}({})", recv, q, mapped, a(self))
            }
            "replace" => {
                let all = matches!(args.first(), Some(Expr::Regex(r)) if r.ends_with('g') || r.contains("/g"));
                format!("{}{}.{}({})", recv, q, if all { "replaceAll" } else { "replaceFirst" }, a(self))
            }
            "padStart" => format!("{}{}.padLeft({})", recv, q, a(self)),
            "padEnd" => format!("{}{}.padRight({})", recv, q, a(self)),
            "charAt" => format!("{}[{}]", recv, a(self)),
            "charCodeAt" => format!("{}.codeUnitAt({})", recv, a(self)),
            "trimStart" => format!("{}.trimLeft()", recv),
            "trimEnd" => format!("{}.trimRight()", recv),
            "substring" | "substr" => format!("jsSlice({}, {})", recv, a(self)),
            "toFixed" => format!("{}.toStringAsFixed({})", recv, a(self)),
            "toString" => format!("{}{}.toString()", recv, q),
            "exists" if args.is_empty() && self.firebase_used => format!("{}.exists", recv),
            "useDeviceLanguage" => format!("{}.setLanguageCode(null)", recv),
            "toISOString" => format!("{}.toUtc().toIso8601String()", recv),
            "toDateString" => format!("jsDateString({})", recv),
            "test" => format!("{}.hasMatch(({}) ?? '')", recv, a(self)),
            "getTime" | "valueOf" => format!("jsTime({})", recv),
            "toLocaleString" | "toLocaleDateString" | "toLocaleTimeString" => format!("jsLocale({})", recv),
            "getFullYear" => format!("{}.year", recv),
            "getMonth" => format!("({}.month - 1)", recv),
            "getDate" => format!("{}.day", recv),
            "getDay" => format!("({}.weekday % 7)", recv),
            "getHours" => format!("{}.hour", recv),
            "getMinutes" => format!("{}.minute", recv),
            "getSeconds" => format!("{}.second", recv),
            "getUTCFullYear" => format!("{}.toUtc().year", recv),
            "getUTCMonth" => format!("({}.toUtc().month - 1)", recv),
            "getUTCDate" => format!("{}.toUtc().day", recv),
            "reverse" => format!("{}.reversed.toList()", recv),
            "sort" => format!("jsSort({}, {})", recv, a(self)),
            "concat" => format!("[...{}, ...{}]", recv, a(self)),
            "flat" => format!("jsFlat({})", recv),
            "keys" if args.is_empty() => format!("{}.keys", recv),
            "values" if args.is_empty() => format!("{}.values", recv),
            "entries" if args.is_empty() => format!("{}.entries", recv),
            "then" => {
                // promise.then(() => ...) -> future.then((_) => ...): a Dart callback always receives the value
                if let Some(Expr::Arrow(f)) = args.first() {
                    if f.params.is_empty() {
                        let param = Param { pat: Pat::Ident("_".to_string()), ty: None, default: None, optional: false, rest: false };
                        let closure = self.func(&Func { params: vec![param], ..(**f).clone() }, None);
                        return format!("{}.then({})", recv, closure);
                    }
                }
                format!("{}.then({})", recv, a(self))
            }
            "catch" => format!("jsCatch({}, {})", recv, a(self)),
            "finally" => format!("{}.whenComplete({})", recv, a(self)),
            "getItem" | "setItem" | "removeItem" => format!("{}.{}({})", recv, method, a(self)),
            "focus" | "click" | "select" | "blur" | "scrollIntoView" | "play" | "pause" | "load" => {
                self.note(format!("`.{}()` acts on a DOM element: written as a no-op", method));
                "null".to_string()
            }
            "stringify" | "parse" => format!("{}{}.{}({})", recv, q, method, a(self)),
            _ => format!("{}{}.{}({})", recv, q, safe_ident(method), a(self)),
        }
    }

    /// `Math.x`, `JSON.x`, `Object.x`, `console.x`, `Date.x`, `Promise.x`, `Array.x`, `Number.x`, `String.x`
    fn global_call(&mut self, global: &str, method: &str, args: &[Expr]) -> Option<String> {
        let a = |cx: &mut Self, i: usize| args.get(i).map(|x| cx.expr(x, None)).unwrap_or_else(|| "null".to_string());
        Some(match (global, method) {
            ("console", "log" | "info" | "debug" | "warn" | "error" | "trace") => {
                let parts: Vec<String> = args
                    .iter()
                    .map(|x| match x {
                        Expr::Str(s) => {
                            let lit = str_lit(s);
                            lit[1..lit.len() - 1].to_string()
                        }
                        other => format!("${{{}}}", self.expr(other, None)),
                    })
                    .collect();
                format!("debugPrint('{}')", parts.join(" "))
            }
            ("Math", "floor") => format!("({}).floor()", a(self, 0)),
            ("Math", "ceil") => format!("({}).ceil()", a(self, 0)),
            ("Math", "round") => format!("({}).round()", a(self, 0)),
            ("Math", "trunc") => format!("({}).truncate()", a(self, 0)),
            ("Math", "abs") => format!("({}).abs()", a(self, 0)),
            ("Math", "sqrt") => format!("sqrt({})", a(self, 0)),
            ("Math", "pow") => format!("pow({}, {})", a(self, 0), a(self, 1)),
            ("Math", "random") => "Random().nextDouble()".to_string(),
            ("Math", "max") => format!("jsMax([{}])", self.args(args, None)),
            ("Math", "min") => format!("jsMin([{}])", self.args(args, None)),
            ("Math", "sin" | "cos" | "tan" | "log" | "exp") => format!("{}({})", method, a(self, 0)),
            ("JSON", "stringify") => format!("jsonEncode({})", a(self, 0)),
            ("JSON", "parse") => format!("jsonDecode(({}) as String)", a(self, 0)),
            ("Object", "keys" | "values" | "entries") => {
                let mut src = a(self, 0);
                // a model object reads as its map
                if let Some(t) = args.first().and_then(|x| self.expr_type(x)) {
                    if self.model_name(&t).is_some() {
                        src = format!("{}.toMap()", src);
                    }
                }
                match method {
                    "keys" => format!("({}).keys.toList()", src),
                    "values" => format!("({}).values.toList()", src),
                    _ => format!("({}).entries.map((e) => [e.key, e.value]).toList()", src),
                }
            }
            ("Object", "assign") => format!("{{...{}, ...{}}}", a(self, 0), a(self, 1)),
            ("Object", "freeze" | "seal") => a(self, 0),
            ("Array", "isArray") => format!("({} is List)", a(self, 0)),
            ("Array", "from") => {
                let len = match args.first() {
                    Some(Expr::Object(props)) => props.iter().find_map(|p| match p {
                        Prop::Kv(PropKey::Name(k), v) if k == "length" => Some(v.clone()),
                        _ => None,
                    }),
                    _ => None,
                };
                match len {
                    Some(n) => {
                        let n = self.expr(&n, None);
                        let f = args.get(1).map(|x| self.expr(x, None)).unwrap_or_else(|| "(_, i) => i".to_string());
                        format!("jsArrayFrom({}, {})", n, f)
                    }
                    None => format!("List.from({})", a(self, 0)),
                }
            }
            ("Number", "isNaN") => format!("jsIsNaN({})", a(self, 0)),
            ("Number", "isInteger") => format!("(({}) is int)", a(self, 0)),
            ("Number", "parseFloat" | "parseInt") => format!("jsNumber({})", a(self, 0)),
            ("String", "fromCharCode") => format!("String.fromCharCode({})", a(self, 0)),
            ("Date", "now") => "DateTime.now().millisecondsSinceEpoch".to_string(),
            ("crypto", "randomUUID") if !self.names.contains("crypto") => "jsRandomUuid()".to_string(),
            ("Date", "UTC") => format!("DateTime.utc({}).millisecondsSinceEpoch", self.args(args, None)),
            ("Promise", "resolve") => format!("Future.value({})", a(self, 0)),
            ("Promise", "reject") => format!("Future.error({})", a(self, 0)),
            ("Promise", "all") => format!("Future.wait<dynamic>({})", a(self, 0)),
            ("Promise", "allSettled" | "race") => format!("Future.wait<dynamic>({})", a(self, 0)),
            _ => return None,
        })
    }

    fn class_set_state(&mut self, args: &[Expr]) -> String {
        let mut assigns = Vec::new();
        if let Some(Expr::Object(props)) = args.first() {
            for p in props {
                match p {
                    Prop::Kv(PropKey::Name(k), v) => {
                        let ty = self.type_of_name(k).cloned();
                        assigns.push(format!("{} = {};", safe_ident(k), self.expr(v, ty.as_ref())));
                    }
                    Prop::Short(k) => assigns.push(format!("{} = {};", safe_ident(k), safe_ident(k))),
                    _ => {}
                }
            }
        }
        format!("setState(() {{ {} }})", assigns.join(" "))
    }

    // ---------------------------------------------------------------- functions

    /// Parameters as Dart text, and the statements that unpack destructured ones.
    pub fn params(&mut self, ps: &[Param], typed: bool) -> (String, String) {
        let mut required = Vec::new();
        let mut optional = Vec::new();
        let mut prelude = String::new();
        for (i, p) in ps.iter().enumerate() {
            let name = match &p.pat {
                Pat::Ident(n) => safe_ident(n),
                _ => format!("p{}", i),
            };
            if let Pat::Ident(n) = &p.pat {
                self.declare(n, p.ty.clone());
            }
            let ty = p.ty.as_ref().map(|t| self.dty(t));
            let text = match (&ty, typed) {
                (Some(t), _) if p.optional || p.default.is_some() => format!("{} {}", nullable(t), name),
                (Some(t), _) => format!("{} {}", t, name),
                (None, true) if p.optional || p.default.is_some() => format!("dynamic {}", name),
                (None, true) => format!("dynamic {}", name),
                (None, false) => name.clone(),
            };
            if p.rest {
                required.push(format!("List<dynamic> {}", name));
            } else if p.optional || p.default.is_some() {
                let def = p.default.as_ref().map(|d| format!(" = {}", self.expr(d, p.ty.as_ref()))).unwrap_or_default();
                optional.push(format!("{}{}", text, def));
            } else {
                required.push(text);
            }
            if !matches!(p.pat, Pat::Ident(_)) {
                prelude.push_str(&self.destructure(&p.pat, &name, p.ty.as_ref(), 2, "final"));
            }
        }
        let mut out = required.join(", ");
        if !optional.is_empty() {
            if !out.is_empty() {
                out.push_str(", ");
            }
            out.push_str(&format!("[{}]", optional.join(", ")));
        }
        (out, prelude)
    }

    /// `const { a, b: c = 1 } = src;` / `const [x, y] = src;` as individual declarations.
    pub fn destructure(&mut self, pat: &Pat, src: &str, ty: Option<&Ty>, depth: usize, kw: &str) -> String {
        let pad = ind(depth);
        let mut out = String::new();
        match pat {
            Pat::Ident(n) => out.push_str(&format!("{}{} {} = {};\n", pad, kw, safe_ident(n), src)),
            Pat::Object(props) => {
                let src_owned = if ty.map_or(false, |t| strip_null(t).1 && !matches!(strip_null(t).0, Ty::Any)) { format!("{}!", src) } else { src.to_string() };
                let src = src_owned.as_str();
                let model = ty.and_then(|t| self.model_name(t));
                let fields = model.as_ref().and_then(|m| self.info.models.get(m)).cloned().unwrap_or_default();
                let is_map = ty.map_or(false, |t| matches!(strip_null(t).0, Ty::Name(ref n, _) if n == "Record" || n == "Map") || dart_type(t, &self.info.models).starts_with("Map<"));
                for pp in props {
                    if pp.rest {
                        let named: Vec<String> = props.iter().filter(|x| !x.rest).map(|x| str_lit(&x.key)).collect();
                        let map_src = if model.is_some() { format!("{}.toMap()", src) } else if is_map { src.to_string() } else { format!("jsToMap({})", src) };
                        if let Pat::Ident(n) = &pp.value {
                            self.map_vars.insert(n.clone());
                            out.push_str(&format!("{}{} {} = <String, dynamic>{{...{}}}..removeWhere((k, _) => [{}].contains(k));\n", pad, kw, safe_ident(n), map_src, named.join(", ")));
                        }
                        continue;
                    }
                    let access = if is_map { format!("{}['{}']", src, pp.key) } else { format!("{}.{}", src, safe_ident(&pp.key)) };
                    let access = match &pp.default {
                        Some(d) => format!("({} ?? {})", access, self.expr(d, None)),
                        None => access,
                    };
                    let fty = fields.iter().find(|f| f.name == pp.key).map(|f| f.ty.clone());
                    match &pp.value {
                        Pat::Ident(n) => {
                            self.declare(n, fty);
                            out.push_str(&format!("{}{} {} = {};\n", pad, kw, safe_ident(n), access));
                        }
                        other => {
                            let tmp = self.fresh("d");
                            out.push_str(&format!("{}final {} = {};\n", pad, tmp, access));
                            out.push_str(&self.destructure(other, &tmp, fty.as_ref(), depth, kw));
                        }
                    }
                }
            }
            Pat::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    if let Some(p) = item {
                        out.push_str(&self.destructure(p, &format!("{}[{}]", src, i), None, depth, kw));
                    }
                }
            }
        }
        out
    }

    /// A function as a Dart closure (`(a, b) => expr`, `(a) async { ... }`).
    pub fn func(&mut self, f: &Func, exp: Option<&Ty>) -> String {
        self.push_scope();
        let in_build = std::mem::replace(&mut self.scope.in_build, false);
        if let (Some(h), Some(p)) = (self.closure_hint.take(), f.params.first()) {
            if let (None, Pat::Ident(n)) = (&p.ty, &p.pat) {
                self.declare(n, Some(h));
            }
        }
        let (mut params, prelude) = self.params(&f.params, false);
        // JavaScript lets a callback ignore arguments: a closure for a function type with more parameters takes them too
        if let Some(Ty::Fn(expected, _)) = exp {
            if expected.len() > f.params.len() && !f.params.iter().any(|p| p.optional || p.default.is_some()) {
                for i in f.params.len()..expected.len() {
                    if !params.is_empty() {
                        params.push_str(", ");
                    }
                    params.push_str(&format!("dynamic _p{}", i));
                }
            }
        }
        let ret_exp = match exp {
            Some(Ty::Fn(_, r)) => Some((**r).clone()),
            _ => f.ret.clone(),
        };
        let ret_exp = ret_exp.map(|t| if f.is_async { unwrap_promise(&t) } else { t });
        let out = match &f.body {
            FnBody::Expr(e) if prelude.is_empty() => format!("({}){} => {}", params, if f.is_async { " async" } else { "" }, self.expr(e, ret_exp.as_ref())),
            FnBody::Expr(e) => format!("({}){} {{\n{}{}return {};\n{}}}", params, if f.is_async { " async" } else { "" }, prelude, ind(3), self.expr(e, ret_exp.as_ref()), ind(2)),
            FnBody::Block(b) => {
                let body = self.block_ret(b, 3, ret_exp.as_ref());
                format!("({}){} {{\n{}{}{}}}", params, if f.is_async { " async" } else { "" }, prelude, body, ind(2))
            }
        };
        self.scope.in_build = in_build;
        self.pop_scope();
        out
    }

    // ---------------------------------------------------------------- statements

    pub fn block(&mut self, stmts: &[Stmt], depth: usize) -> String {
        self.block_ret(stmts, depth, None)
    }

    pub fn block_ret(&mut self, stmts: &[Stmt], depth: usize, ret: Option<&Ty>) -> String {
        let mut out = String::new();
        for s in stmts {
            out.push_str(&self.stmt(s, depth, ret));
        }
        out
    }

    fn body(&mut self, s: &Stmt, depth: usize, ret: Option<&Ty>) -> String {
        match s {
            Stmt::Block(b) => self.block_ret(b, depth + 1, ret),
            other => self.stmt(other, depth + 1, ret),
        }
    }

    pub fn stmt(&mut self, s: &Stmt, depth: usize, ret: Option<&Ty>) -> String {
        let pad = ind(depth);
        match s {
            Stmt::Empty => String::new(),
            Stmt::Expr(e) => {
                // e.preventDefault() has nothing to do in Flutter
                if let Expr::Call(callee, _, _) = e {
                    if let Expr::Member(obj, m, _) = &**callee {
                        if matches!(&**obj, Expr::Ident(n) if self.scope.events.contains(n)) && is_event_call(m) {
                            return String::new();
                        }
                    }
                }
                format!("{}{};\n", pad, unparen(&self.expr(e, None)))
            }
            Stmt::Return(None) if self.scope.in_build => format!("{}return const SizedBox.shrink();\n", pad),
            Stmt::Return(Some(Expr::Null | Expr::Undefined)) if self.scope.in_build => format!("{}return const SizedBox.shrink();\n", pad),
            Stmt::Return(None) => format!("{}return;\n", pad),
            Stmt::Return(Some(e)) => {
                let c = unparen(&self.expr(e, ret));
                let c = if self.scope.in_build { self.nn_widget(c) } else { c };
                format!("{}return {};\n", pad, c)
            }
            Stmt::Throw(e) => format!("{}throw {};\n", pad, self.expr(e, None)),
            Stmt::Break => format!("{}break;\n", pad),
            Stmt::Continue => format!("{}continue;\n", pad),
            Stmt::Block(b) => {
                self.push_scope();
                let inner = self.block_ret(b, depth + 1, ret);
                self.pop_scope();
                format!("{}{{\n{}{}}}\n", pad, inner, pad)
            }
            Stmt::Var(kind, decls) => {
                let mut out = String::new();
                for d in decls {
                    out.push_str(&self.var_decl(kind, d, depth));
                }
                out
            }
            Stmt::If(c, t, e) => {
                let test = unparen(&self.cond(c));
                self.push_scope();
                let then = self.body(t, depth, ret);
                self.pop_scope();
                let mut out = format!("{}if ({}) {{\n{}{}}}", pad, test, then, pad);
                if let Some(e) = e {
                    if let Stmt::If(..) = **e {
                        let nested = self.stmt(e, depth, ret);
                        out.push_str(&format!(" else {}", nested.trim_start()));
                        return out;
                    }
                    self.push_scope();
                    let els = self.body(e, depth, ret);
                    self.pop_scope();
                    out.push_str(&format!(" else {{\n{}{}}}", els, pad));
                }
                out.push('\n');
                out
            }
            Stmt::For(init, test, update, body) => {
                self.push_scope();
                let init_s = match init {
                    Some(b) => match &**b {
                        Stmt::Var(kind, decls) => {
                            let d = &decls[0];
                            let name = match &d.pat {
                                Pat::Ident(n) => safe_ident(n),
                                _ => "i".to_string(),
                            };
                            let v = d.init.as_ref().map(|e| self.expr(e, None)).unwrap_or_else(|| "0".to_string());
                            format!("{} {} = {}", if kind == "const" { "final" } else { "var" }, name, v)
                        }
                        Stmt::Expr(e) => self.expr(e, None),
                        _ => String::new(),
                    },
                    None => String::new(),
                };
                let test_s = test.as_ref().map(|e| unparen(&self.cond(e))).unwrap_or_default();
                let upd_s = update.as_ref().map(|e| self.expr(e, None)).unwrap_or_default();
                let b = self.body(body, depth, ret);
                self.pop_scope();
                format!("{}for ({}; {}; {}) {{\n{}{}}}\n", pad, init_s, test_s, upd_s, b, pad)
            }
            Stmt::ForOf(kind, pat, iter, body, is_in) => {
                self.push_scope();
                let it = self.expr(iter, None);
                let it = if *is_in { format!("jsKeys({})", it) } else { it };
                let kw = if kind == "const" { "final" } else { "var" };
                let (head, prelude) = match pat {
                    Pat::Ident(n) => (format!("{} {}", kw, safe_ident(n)), String::new()),
                    other => {
                        let tmp = self.fresh("it");
                        (format!("final {}", tmp), self.destructure(other, &tmp, None, depth + 1, kw))
                    }
                };
                let b = self.body(body, depth, ret);
                self.pop_scope();
                format!("{}for ({} in {}) {{\n{}{}{}}}\n", pad, head, it, prelude, b, pad)
            }
            Stmt::While(c, body) => {
                let test = unparen(&self.cond(c));
                let b = self.body(body, depth, ret);
                format!("{}while ({}) {{\n{}{}}}\n", pad, test, b, pad)
            }
            Stmt::DoWhile(body, c) => {
                let b = self.body(body, depth, ret);
                let test = unparen(&self.cond(c));
                format!("{}do {{\n{}{}}} while ({});\n", pad, b, pad, test)
            }
            Stmt::Try(body, catch, fin) => {
                self.push_scope();
                let mut out = format!("{}try {{\n{}{}}}", pad, self.block_ret(body, depth + 1, ret), pad);
                self.pop_scope();
                if let Some(c) = catch {
                    let name = match &c.param {
                        Some(Pat::Ident(n)) => safe_ident(n),
                        _ => "_".to_string(),
                    };
                    self.catch_params.push(name.clone());
                    self.push_scope();
                    let b = self.block_ret(&c.body, depth + 1, ret);
                    self.pop_scope();
                    self.catch_params.pop();
                    out.push_str(&format!(" catch ({}) {{\n{}{}}}", name, b, pad));
                } else if fin.is_none() {
                    out.push_str(" catch (_) {}");
                }
                if let Some(f) = fin {
                    self.push_scope();
                    let b = self.block_ret(f, depth + 1, ret);
                    self.pop_scope();
                    out.push_str(&format!(" finally {{\n{}{}}}", b, pad));
                }
                out.push('\n');
                out
            }
            Stmt::Switch(d, cases) => {
                let mut out = format!("{}switch ({}) {{\n", pad, self.expr(d, None));
                for (test, body) in cases {
                    match test {
                        Some(t) => out.push_str(&format!("{}  case {}:\n", pad, self.expr(t, None))),
                        None => out.push_str(&format!("{}  default:\n", pad)),
                    }
                    self.push_scope();
                    let b = self.block_ret(body, depth + 2, ret);
                    self.pop_scope();
                    out.push_str(&b);
                    let ends = body.last().map_or(false, |s| matches!(s, Stmt::Break | Stmt::Return(_) | Stmt::Throw(_) | Stmt::Continue));
                    if !ends && !body.is_empty() {
                        out.push_str(&format!("{}    break;\n", pad));
                    }
                }
                out.push_str(&format!("{}}}\n", pad));
                out
            }
            Stmt::Func(f) => {
                let name = f.name.clone().unwrap_or_else(|| "fn".to_string());
                self.push_scope();
                let (params, prelude) = self.params(&f.params, true);
                let ret_ty = self.return_type(f);
                let ret_exp = f.ret.as_ref().map(|t| if f.is_async { unwrap_promise(t) } else { t.clone() });
                let body = match &f.body {
                    FnBody::Block(b) => self.block_ret(b, depth + 1, ret_exp.as_ref()),
                    FnBody::Expr(e) => format!("{}return {};\n", ind(depth + 1), self.expr(e, ret_exp.as_ref())),
                };
                self.pop_scope();
                format!("{}{} {}({}){} {{\n{}{}{}}}\n", pad, ret_ty, safe_ident(&name), params, if f.is_async { " async" } else { "" }, prelude, body, pad)
            }
        }
    }

    pub fn return_type(&mut self, f: &Func) -> String {
        match &f.ret {
            Some(t) => {
                let d = self.dty(t);
                if f.is_async && !d.starts_with("Future") { format!("Future<{}>", d) } else { d }
            }
            None => if f.is_async { "Future<dynamic>".to_string() } else { "dynamic".to_string() },
        }
    }

    fn var_decl(&mut self, kind: &str, d: &VarDecl, depth: usize) -> String {
        let pad = ind(depth);
        let kw = if kind == "const" { "final" } else { "var" };
        match &d.pat {
            Pat::Ident(n) => {
                let name = safe_ident(n);
                // `const copy = error` inside a catch keeps being the error
                let mut src_expr = d.init.as_ref();
                while let Some(Expr::As(x, _) | Expr::NonNull(x)) = src_expr {
                    src_expr = Some(x);
                }
                if let Some(Expr::Ident(src)) = src_expr {
                    if self.catch_params.contains(src) {
                        self.catch_params.push(n.clone());
                    }
                }
                self.declare(n, d.ty.clone().or_else(|| d.init.as_ref().and_then(|e| self.expr_type(e))));
                let init = d.init.as_ref().map(|e| unparen(&self.expr(e, d.ty.as_ref())));
                let widget_init = matches!(d.init, Some(Expr::Jsx(_)));
                if widget_init {
                    self.scope.widgets.insert(n.clone());
                }
                match (&d.ty, init) {
                    (Some(t), Some(i)) => format!("{}{} {} = {};\n", pad, self.dty(t), name, i),
                    (Some(t), None) => format!("{}late {} {};\n", pad, self.dty(t), name),
                    (None, Some(i)) => format!("{}{} {} = {};\n", pad, if widget_init { "final Widget" } else { kw }, name, i).replace("final Widget ", if widget_init { "final Widget " } else { "" }),
                    (None, None) => format!("{}dynamic {};\n", pad, name),
                }
            }
            other => {
                let src_expr = d.init.as_ref();
                let src = match src_expr {
                    Some(e) => self.expr(e, d.ty.as_ref()),
                    None => "null".to_string(),
                };
                let ty = d.ty.clone().or_else(|| src_expr.and_then(|e| self.expr_type(e)));
                let tmp = self.fresh("v");
                let mut out = format!("{}final {} = {};\n", pad, tmp, src);
                out.push_str(&self.destructure(other, &tmp, ty.as_ref(), depth, kw));
                out
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::convert::parse::parse_module;

    fn dart_expr(src: &str) -> String {
        let m = parse_module(&format!("const x = {};", src));
        let Item::Var { decls, .. } = &m.items[0] else { panic!() };
        let info = ProjectInfo::default();
        let mut cx = Cx::new(&info, "t.ts");
        cx.expr(decls[0].init.as_ref().unwrap(), None)
    }

    fn dart_stmts(src: &str) -> String {
        let m = parse_module(&format!("function f() {{ {} }}", src));
        let Item::Func { func, .. } = &m.items[0] else { panic!() };
        let FnBody::Block(b) = &func.body else { panic!() };
        let info = ProjectInfo::default();
        let mut cx = Cx::new(&info, "t.ts");
        cx.block(b, 0)
    }

    #[test]
    fn operators_and_literals_read_as_dart() {
        assert_eq!(dart_expr("a === b"), "(a == b)");
        assert_eq!(dart_expr("a !== undefined"), "(a != null)");
        assert_eq!(dart_expr("`n=${n} \\${x} it's`"), "'n=$n \\${x} it\\'s'");
        assert_eq!(dart_expr("'a' + b + 1"), "'a${b}${1}'");
        assert_eq!(dart_expr("typeof x === 'string'"), "(x is String)");
        assert_eq!(dart_expr("a ?? b"), "(a ?? b)");
        assert_eq!(dart_expr("err.message || 'x'"), "(err.message ?? 'x')");
        assert_eq!(dart_expr("a.b || c.d || 'z'"), "(a.b ?? c.d ?? 'z')");
        assert_eq!(dart_expr("xs.length > 0"), "xs.isNotEmpty");
        assert_eq!(dart_expr("xs.length === 0"), "xs.isEmpty");
    }

    #[test]
    fn truthiness_becomes_the_explicit_dart_test() {
        let s = dart_stmts("if (user) { a(); }\nif (!user) { b(); }\nif (isOpen && user) { c(); }\nif (loading || !items) { d(); }");
        assert!(s.contains("if (user != null) {"), "{}", s);
        assert!(s.contains("if (user == null) {"), "{}", s);
        assert!(s.contains("if ((isOpen == true) && user != null) {") || s.contains("if (isOpen == true && user != null) {"), "{}", s);
        assert!(!s.contains("jsTruthy"), "{}", s);
    }

    #[test]
    fn methods_are_renamed() {
        assert_eq!(dart_expr("xs.filter(x => x > 1)"), "xs.where((x) => (x > 1)).toList()");
        assert_eq!(dart_expr("xs.map(x => x * 2)"), "xs.map((x) => (x * 2)).toList()");
        assert_eq!(dart_expr("s.includes('a')"), "s.contains('a')");
        assert_eq!(dart_expr("Math.floor(a / b)"), "((a / b)).floor()");
        assert_eq!(dart_expr("Math.max(1, 2)"), "jsMax([1, 2])");
        assert_eq!(dart_expr("JSON.stringify(o)"), "jsonEncode(o)");
        assert_eq!(dart_expr("xs.push(1)"), "xs.add(1)");
    }

    #[test]
    fn statements_read_as_dart() {
        let s = dart_stmts("try { const r = await go(a); if (!r) { return; } } catch (e: any) { console.error(e.message); } finally { done(); }\nfor (const k of ks) { total += k; }\nswitch (a) { case 'x': foo(); break; default: bar(); }");
        assert!(s.contains("final r = await go(a);"), "{}", s);
        assert!(s.contains("if (r == null || r == false) {"), "{}", s);
        assert!(s.contains("catch (e) {") && s.contains("jsErrorMessage(e)"), "{}", s);
        assert!(s.contains("for (final k in ks)") && s.contains("switch (a) {") && s.contains("case 'x':"), "{}", s);
    }
}
