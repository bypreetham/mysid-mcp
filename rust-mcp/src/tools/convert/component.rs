//! React components, hooks and plain functions -> Dart classes and functions.
//!
//! A component becomes a `StatelessWidget`, or a `StatefulWidget` + `State` when it uses state, refs or effects:
//! `useState` -> a field and `setState`, `useRef` -> a `Ref` field, `useEffect` -> a method run after the frame when its
//! dependencies change (cleanup in `dispose`), `useCallback` / arrow constants -> methods, `useMemo` and other constants ->
//! getters, props -> constructor parameters (read as `widget.x` inside the State).

use super::ast::*;
use super::cx::*;
use super::expr::str_lit;
use super::project::Kind;
use super::types::*;

#[derive(Clone)]
pub struct PropSpec {
    pub field: DField,
    pub alias: Option<String>,
    pub default: Option<Expr>,
}

#[derive(Clone)]
struct StateVar {
    name: String,
    setter: Option<String>,
    ty: Option<Ty>,
    init: Option<Expr>,
    is_ref: bool,
}

#[derive(Clone)]
struct EffectSpec {
    body: Vec<Stmt>,
    cleanup: Option<Expr>,
    deps: Option<Vec<Expr>>,
}

#[derive(Clone)]
struct MethodSpec {
    name: String,
    func: Func,
    /// a `static` or lifecycle method that is not carried over
    skip: bool,
}

#[derive(Default, Clone)]
struct Spec {
    props: Vec<PropSpec>,
    state: Vec<StateVar>,
    effects: Vec<EffectSpec>,
    methods: Vec<MethodSpec>,
    getters: Vec<(String, Expr, Option<Ty>)>,
    prelude: Vec<Stmt>,
    ret: Option<Expr>,
    /// the name of a `...rest` props parameter and the props it carries
    rest: Option<(String, Vec<String>)>,
    /// the whole return statement list when the body is more than a single `return`
    class_render: Option<Vec<Stmt>>,
    props_ident: Option<String>,
}

// ---------------------------------------------------------------- recognising things

fn callee_name(e: &Expr) -> Option<&str> {
    match e {
        Expr::Call(c, _, _) => match &**c {
            Expr::Ident(n) => Some(n),
            Expr::Member(o, m, _) if matches!(&**o, Expr::Ident(r) if r == "React") => Some(m),
            _ => None,
        },
        _ => None,
    }
}

fn call_args(e: &Expr) -> &[Expr] {
    match e {
        Expr::Call(_, a, _) => a,
        _ => &[],
    }
}

pub fn contains_jsx(e: &Expr) -> bool {
    match e {
        Expr::Jsx(_) => true,
        Expr::Cond(_, a, b) => contains_jsx(a) || contains_jsx(b),
        Expr::Binary(op, _, r) if op == "&&" || op == "||" || op == "??" => contains_jsx(r),
        Expr::As(x, _) | Expr::NonNull(x) => contains_jsx(x),
        _ => false,
    }
}

pub fn returns_jsx(f: &Func) -> bool {
    fn walk(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Return(Some(e)) => contains_jsx(e),
            Stmt::If(_, t, e) => walk(std::slice::from_ref(t)) || e.as_ref().map_or(false, |e| walk(std::slice::from_ref(e))),
            Stmt::Block(b) => walk(b),
            Stmt::Try(b, c, f) => walk(b) || c.as_ref().map_or(false, |c| walk(&c.body)) || f.as_ref().map_or(false, |f| walk(f)),
            Stmt::Switch(_, cases) => cases.iter().any(|(_, b)| walk(b)),
            _ => false,
        })
    }
    match &f.body {
        FnBody::Expr(e) => contains_jsx(e),
        FnBody::Block(b) => walk(b),
    }
}

/// `React.memo(fn)`, `forwardRef(fn)` and a plain arrow: the function inside.
pub fn function_of(e: &Expr) -> Option<&Func> {
    match e {
        Expr::Arrow(f) => Some(f),
        Expr::As(x, _) | Expr::NonNull(x) => function_of(x),
        Expr::Call(_, args, _) if matches!(callee_name(e), Some("memo" | "forwardRef")) => args.first().and_then(function_of),
        _ => None,
    }
}

pub fn is_component_name(n: &str) -> bool {
    n.chars().next().map_or(false, |c| c.is_uppercase())
}

pub fn is_hook_name(n: &str) -> bool {
    n.len() > 3 && n.starts_with("use") && n[3..].chars().next().map_or(false, |c| c.is_uppercase())
}

fn has_value_return(f: &Func) -> bool {
    fn walk(stmts: &[Stmt]) -> bool {
        stmts.iter().any(|s| match s {
            Stmt::Return(Some(_)) => true,
            Stmt::If(_, t, e) => walk(std::slice::from_ref(t)) || e.as_ref().map_or(false, |e| walk(std::slice::from_ref(e))),
            Stmt::Block(b) => walk(b),
            Stmt::Try(b, c, f) => walk(b) || c.as_ref().map_or(false, |c| walk(&c.body)) || f.as_ref().map_or(false, |f| walk(f)),
            Stmt::For(_, _, _, b) | Stmt::ForOf(_, _, _, b, _) | Stmt::While(_, b) | Stmt::DoWhile(b, _) => walk(std::slice::from_ref(b)),
            Stmt::Switch(_, cases) => cases.iter().any(|(_, b)| walk(b)),
            _ => false,
        })
    }
    match &f.body {
        FnBody::Expr(_) => true,
        FnBody::Block(b) => walk(b),
    }
}


fn required_params(f: &Func) -> usize {
    f.params.iter().filter(|p| !p.optional && p.default.is_none() && !p.rest).count()
}

// ---------------------------------------------------------------- reading a component

impl<'a> Cx<'a> {
    /// The props of a component from its first parameter: (specs, name of a non-destructured props parameter).
    fn read_props(&mut self, f: &Func) -> (Vec<PropSpec>, Option<String>, Option<(String, Vec<String>)>) {
        let Some(p) = f.params.first() else { return (Vec::new(), None, None) };
        // the props type is on the parameter, or on the variable: `const X: React.FC<Props> = ({ a }) => ...`
        let ty_src = p.ty.clone().or_else(|| self.props_hint.take());
        let declared: Vec<DField> = match &ty_src {
            Some(Ty::Name(n, _)) => self.info.comps_by_type(n),
            Some(Ty::Obj(fields)) => fields.iter().map(|f| DField { name: f.name.clone(), ty: f.ty.clone(), optional: f.optional }).collect(),
            Some(Ty::Inter(parts)) => parts.iter().flat_map(|t| match t {
                Ty::Name(n, _) => self.info.comps_by_type(n),
                Ty::Obj(fields) => fields.iter().map(|f| DField { name: f.name.clone(), ty: f.ty.clone(), optional: f.optional }).collect(),
                _ => Vec::new(),
            }).collect(),
            _ => Vec::new(),
        };
        match &p.pat {
            Pat::Object(props) => {
                let mut out = Vec::new();
                let mut rest_name: Option<String> = None;
                for pp in props {
                    if pp.rest {
                        rest_name = Some(pp.key.clone());
                        continue;
                    }
                    let field = declared.iter().find(|d| d.name == pp.key).cloned().unwrap_or(DField { name: pp.key.clone(), ty: Ty::Any, optional: pp.default.is_some() });
                    let alias = match &pp.value {
                        Pat::Ident(n) if n != &pp.key => Some(n.clone()),
                        _ => None,
                    };
                    out.push(PropSpec { field, alias, default: pp.default.clone() });
                }
                // the props the `...rest` stands for (those declared in the type but not named): they are accepted too
                let mut rest = None;
                if let Some(rn) = rest_name {
                    let mut names = Vec::new();
                    for d in &declared {
                        if !out.iter().any(|o| o.field.name == d.name) {
                            out.push(PropSpec { field: DField { optional: true, ..d.clone() }, alias: None, default: None });
                            names.push(d.name.clone());
                        }
                    }
                    rest = Some((rn, names));
                }
                (out, None, rest)
            }
            Pat::Ident(n) => {
                let out = declared.into_iter().map(|field| PropSpec { field, alias: None, default: None }).collect();
                (out, Some(n.clone()), None)
            }
            _ => (Vec::new(), None, None),
        }
    }

    fn analyse_function(&mut self, f: &Func) -> Spec {
        let mut spec = Spec::default();
        let (props, props_ident, rest) = self.read_props(f);
        spec.props = props;
        spec.props_ident = props_ident;
        spec.rest = rest;
        let stmts: Vec<Stmt> = match &f.body {
            FnBody::Block(b) => b.clone(),
            FnBody::Expr(e) => vec![Stmt::Return(Some((**e).clone()))],
        };
        let mut seen_effect_or_code = false;
        for s in &stmts {
            match s {
                Stmt::Var(kind, decls) => {
                    for d in decls {
                        self.analyse_decl(&mut spec, kind, d, seen_effect_or_code);
                    }
                }
                Stmt::Func(func) => {
                    let name = func.name.clone().unwrap_or_default();
                    spec.methods.push(MethodSpec { name, func: func.clone(), skip: false });
                }
                Stmt::Expr(e) if matches!(callee_name(e), Some("useEffect" | "useLayoutEffect" | "useInsertionEffect")) => {
                    let args = call_args(e);
                    let (body, cleanup) = match args.first().and_then(function_of) {
                        Some(Func { body: FnBody::Block(b), .. }) => {
                            let mut b = b.clone();
                            let cleanup = match b.last() {
                                Some(Stmt::Return(Some(c))) => Some(c.clone()),
                                _ => None,
                            };
                            if cleanup.is_some() {
                                b.pop();
                            }
                            (b, cleanup)
                        }
                        Some(Func { body: FnBody::Expr(e), .. }) => (vec![Stmt::Expr((**e).clone())], None),
                        _ => (Vec::new(), None),
                    };
                    let deps = match args.get(1) {
                        Some(Expr::Array(items)) => Some(items.clone()),
                        _ => None,
                    };
                    spec.effects.push(EffectSpec { body, cleanup, deps });
                }
                Stmt::Return(Some(e)) if matches!(stmts.last(), Some(l) if std::ptr::eq(l, s)) => spec.ret = Some(e.clone()),
                other => {
                    seen_effect_or_code = true;
                    spec.prelude.push(other.clone());
                }
            }
        }
        spec
    }

    fn analyse_decl(&mut self, spec: &mut Spec, kind: &str, d: &VarDecl, after_code: bool) {
        let init = d.init.as_ref();
        let (call, targs) = init.map(|e| unwrap_targs(e)).map_or((None, &[][..]), |(c, t)| (Some(c), t));
        let hook = call.and_then(callee_name);
        match (&d.pat, hook) {
            (Pat::Array(items), Some("useState")) | (Pat::Array(items), Some("useReducer")) => {
                let name = match items.first() {
                    Some(Some(Pat::Ident(n))) => n.clone(),
                    _ => "state".to_string(),
                };
                let setter = match items.get(1) {
                    Some(Some(Pat::Ident(n))) => Some(n.clone()),
                    _ => None,
                };
                let args = call.map(call_args).unwrap_or(&[]);
                let init_e = args.first().cloned();
                let ty = targs.first().cloned().or_else(|| d.ty.clone()).or_else(|| init_e.as_ref().and_then(|e| self.expr_type(e)));
                spec.state.push(StateVar { name, setter, ty, init: init_e, is_ref: false });
            }
            (Pat::Ident(n), Some("useRef")) => {
                let args = call.map(call_args).unwrap_or(&[]);
                spec.state.push(StateVar { name: n.clone(), setter: None, ty: targs.first().cloned(), init: args.first().cloned(), is_ref: true });
            }
            (Pat::Ident(n), Some("useNavigate")) => {
                self.scope.nav.insert(n.clone());
            }
            (Pat::Ident(n), Some("useCallback")) => {
                if let Some(f) = call.map(call_args).and_then(|a| a.first()).and_then(function_of) {
                    spec.methods.push(MethodSpec { name: n.clone(), func: f.clone(), skip: false });
                }
            }
            (Pat::Ident(n), Some("useMemo")) => {
                if let Some(Func { body, .. }) = call.map(call_args).and_then(|a| a.first()).and_then(function_of) {
                    match body {
                        FnBody::Expr(e) => spec.getters.push((n.clone(), (**e).clone(), d.ty.clone())),
                        FnBody::Block(_) => {
                            let f = call.map(call_args).and_then(|a| a.first()).and_then(function_of).unwrap().clone();
                            spec.methods.push(MethodSpec { name: format!("_memo_{}", n), func: f, skip: false });
                            spec.getters.push((n.clone(), Expr::Call(Box::new(Expr::Ident(format!("_memo_{}", n))), Vec::new(), false), d.ty.clone()));
                        }
                    }
                }
            }
            (Pat::Ident(n), None) if init.and_then(function_of).is_some() && !hook_like(init) => {
                if let Some(Expr::Str(s)) = init {
                    self.consts.insert(n.clone(), s.clone());
                }
                spec.methods.push(MethodSpec { name: n.clone(), func: init.and_then(function_of).unwrap().clone(), skip: false });
            }
            (pat, _) if kind == "const" && init.is_some() && !after_code => {
                // any other constant: a getter (one per name bound by a pattern)
                let e = init.unwrap().clone();
                if let (Pat::Ident(n), Expr::Str(s)) = (pat, &e) {
                    self.consts.insert(n.clone(), s.clone());
                }
                match pat {
                    Pat::Ident(n) => spec.getters.push((n.clone(), e, d.ty.clone())),
                    Pat::Object(props) => {
                        let tmp = self.fresh("src");
                        spec.getters.push((tmp.clone(), e, d.ty.clone()));
                        for pp in props {
                            if let Pat::Ident(n) = &pp.value {
                                let mut access = Expr::Member(Box::new(Expr::Ident(tmp.clone())), pp.key.clone(), false);
                                if let Some(def) = &pp.default {
                                    access = Expr::Binary("??".to_string(), Box::new(access), Box::new(def.clone()));
                                }
                                spec.getters.push((n.clone(), access, None));
                            }
                        }
                    }
                    Pat::Array(items) => {
                        let tmp = self.fresh("src");
                        spec.getters.push((tmp.clone(), e, d.ty.clone()));
                        for (i, it) in items.iter().enumerate() {
                            if let Some(Pat::Ident(n)) = it {
                                spec.getters.push((n.clone(), Expr::Index(Box::new(Expr::Ident(tmp.clone())), Box::new(Expr::Num(i.to_string())), false), None));
                            }
                        }
                    }
                }
            }
            _ => spec.prelude.push(Stmt::Var(kind.to_string(), vec![d.clone()])),
        }
    }

    // ---------------------------------------------------------------- emitting

    /// Registers the names the emitted code resolves against (setters, methods, props).
    fn register(&mut self, spec: &Spec, stateful: bool) {
        for p in &spec.props {
            let name = p.alias.clone().unwrap_or_else(|| p.field.name.clone());
            let target = if stateful { format!("widget.{}", safe_ident(&p.field.name)) } else { safe_ident(&p.field.name) };
            if stateful || p.alias.is_some() {
                self.scope.renames.insert(name.clone(), target);
            }
            // an optional prop without a default may be null
            let ty = if p.field.optional && p.default.is_none() && !matches!(&p.field.ty, Ty::Union(_)) { Ty::Union(vec![p.field.ty.clone(), Ty::Name("undefined".to_string(), Vec::new())]) } else { p.field.ty.clone() };
            self.declare(&name, Some(ty));
            if matches!(&p.field.ty, Ty::Name(n, _) if n == "LucideIcon") {
                self.scope.icon_vars.insert(name.clone());
            }
            if is_widget_type(&p.field.ty) {
                self.scope.nullable_widgets.insert(name.clone());
            }
        }
        for (n, _, _) in &spec.getters {
            self.names.insert(n.clone());
        }
        for m in &spec.methods {
            self.names.insert(m.name.clone());
        }
        for s in &spec.state {
            self.names.insert(s.name.clone());
            if let Some(st) = &s.setter {
                self.names.insert(st.clone());
            }
        }
        if let Some(pi) = &spec.props_ident {
            self.scope.renames.insert(pi.clone(), if stateful { "widget".to_string() } else { "this".to_string() });
        }
        for s in &spec.state {
            self.declare(&s.name, s.ty.clone());
            self.scope.state_fields.insert(s.name.clone());
            if let Some(setter) = &s.setter {
                self.scope.setters.insert(setter.clone(), (safe_ident(&s.name), s.ty.clone()));
            }
        }
        for m in &spec.methods {
            self.scope.methods.insert(m.name.clone(), required_params(&m.func));
        }
        self.scope.in_state = stateful;
        self.scope.rest = spec.rest.clone();
    }

    fn state_type(&self, s: &StateVar) -> String {
        if s.is_ref {
            return "Ref<dynamic>".to_string();
        }
        match &s.ty {
            Some(t) => {
                let d = self.dty(t);
                // an unset state of a type that allows null starts as null
                if matches!(s.init, Some(Expr::Null) | Some(Expr::Undefined) | None) { nullable(&d) } else { d }
            }
            None => "dynamic".to_string(),
        }
    }

    fn method_code(&mut self, name: &str, f: &Func, depth: usize) -> String {
        self.push_scope();
        let before = self.scope.events.len();
        for p in &f.params {
            if let (Pat::Ident(n), Some(Ty::Name(t, _))) = (&p.pat, &p.ty) {
                if t.contains("Event") {
                    self.scope.events.push(n.clone());
                }
            }
        }
        if f.params.len() == 1 && f.params[0].ty.is_none() {
            if let Pat::Ident(n) = &f.params[0].pat {
                if matches!(n.as_str(), "e" | "ev" | "event") {
                    self.scope.events.push(n.clone());
                }
            }
        }
        let (params, prelude) = self.params(&f.params, true);
        let value = has_value_return(f);
        let ret = match &f.ret {
            Some(t) => {
                let d = self.dty(t);
                if f.is_async && !d.starts_with("Future") { format!("Future<{}>", d) } else { d }
            }
            None => match (f.is_async, value) {
                (true, true) => "Future<dynamic>".to_string(),
                (true, false) => "Future<void>".to_string(),
                (false, true) => "dynamic".to_string(),
                (false, false) => "void".to_string(),
            },
        };
        let ret_exp = f.ret.as_ref().map(|t| if f.is_async { super::expr::unwrap_promise(t) } else { t.clone() });
        let pad = ind(depth);
        let out = match &f.body {
            FnBody::Expr(e) if prelude.is_empty() && ret == "void" => {
                let c = self.expr(e, ret_exp.as_ref());
                format!("{}{} {}({}){} {{\n{}  {};\n{}}}\n", pad, ret, safe_ident(name), params, if f.is_async { " async" } else { "" }, pad, c, pad)
            }
            FnBody::Expr(e) => {
                let c = self.expr(e, ret_exp.as_ref());
                format!("{}{} {}({}){} {{\n{}{}  return {};\n{}}}\n", pad, ret, safe_ident(name), params, if f.is_async { " async" } else { "" }, prelude, pad, c, pad)
            }
            FnBody::Block(b) => {
                let body = self.block_ret(b, depth + 1, ret_exp.as_ref());
                format!("{}{} {}({}){} {{\n{}{}{}}}\n", pad, ret, safe_ident(name), params, if f.is_async { " async" } else { "" }, prelude, body, pad)
            }
        };
        self.scope.events.truncate(before);
        self.pop_scope();
        out
    }

    /// The widget class (or classes) for a function component.
    pub fn component(&mut self, name: &str, f: &Func, kind: Kind) -> String {
        let spec = self.analyse_function(f);
        self.emit_component(name, spec, kind)
    }

    fn emit_component(&mut self, name: &str, spec: Spec, kind: Kind) -> String {
        // Methods and getters that need `context` (navigate, useLocation, ...) can only live in a State: try as a stateless
        // widget first, and redo it as a StatefulWidget when the body asks for the context.
        match self.emit_inner(name, spec.clone(), kind, false) {
            Ok(code) => code,
            Err(()) => self.emit_inner(name, spec, kind, true).unwrap_or_default(),
        }
    }

    fn emit_inner(&mut self, name: &str, mut spec: Spec, kind: Kind, force_stateful: bool) -> Result<String, ()> {
        let stateful = force_stateful || !spec.state.is_empty() || !spec.effects.is_empty();
        let saved = self.scope.clone();
        self.register(&spec, stateful);
        let mut out = String::new();

        // ---- the widget class: props
        let mut fields = String::new();
        let mut ctor = Vec::new();
        for p in &spec.props {
            let t = field_type(&p.field, &self.info.models);
            let fname = safe_ident(&p.field.name);
            let is_children = p.field.name == "children";
            let t = if is_children { "Widget?".to_string() } else { t };
            // a prop with a default value is never null
            let t = if p.default.is_some() && !matches!(p.default, Some(Expr::Null) | Some(Expr::Undefined)) { t.trim_end_matches('?').to_string() } else { t };
            fields.push_str(&format!("  final {} {};\n", t, fname));
            let required = !p.field.optional && !t.ends_with('?') && t != "dynamic" && p.default.is_none();
            match &p.default {
                Some(d) => {
                    let dv = self.expr(d, Some(&p.field.ty));
                    ctor.push(format!("this.{} = {}", fname, dv));
                }
                None if required => ctor.push(format!("required this.{}", fname)),
                None => ctor.push(format!("this.{}", fname)),
            }
        }
        let mut ctor_args = vec!["super.key".to_string()];
        ctor_args.extend(ctor);
        let base = if stateful { "StatefulWidget" } else { "StatelessWidget" };
        out.push_str(&format!("class {} extends {} {{\n{}", name, base, fields));
        out.push_str(&format!("\n  {}({{{}}});\n", name, ctor_args.join(", ")));

        // ---- the class that holds the body
        let mut body = String::new();
        let mut field_lines = String::new();
        for s in &spec.state {
            let t = self.state_type(s);
            let nm = safe_ident(&s.name);
            let init = match &s.init {
                Some(e) => {
                    let ty = s.ty.clone();
                    let code = if s.is_ref { self.expr(e, None) } else { self.expr(e, ty.as_ref()) };
                    // a lazy initial value `useState(() => x)`
                    let code = match e {
                        Expr::Arrow(_) if !s.is_ref => format!("({})()", code),
                        _ => code,
                    };
                    Some(code)
                }
                None => None,
            };
            let simple = s.init.as_ref().map_or(true, is_simple_literal);
            if s.is_ref {
                field_lines.push_str(&format!("  late final Ref<dynamic> {} = Ref<dynamic>({});\n", nm, init.unwrap_or_default()));
            } else {
                // a value computed from props, getters or other state is a `late` field: it may read the instance
                match init {
                    Some(i) if !simple => field_lines.push_str(&format!("  late {} {} = {};\n", t, nm, i)),
                    Some(i) => field_lines.push_str(&format!("  {} {} = {};\n", t, nm, i)),
                    None => field_lines.push_str(&format!("  {} {};\n", nullable(&t), nm)),
                }
            }
        }
        // getters
        for (n, e, ty) in &spec.getters {
            let t = ty.clone().or_else(|| self.expr_type(e));
            let code = self.expr(e, t.as_ref());
            self.declare(n, t.clone());
            let tstr = match &t {
                Some(t) if !matches!(t, Ty::Fn(..)) => self.dty(t),
                _ => "dynamic".to_string(),
            };
            body.push_str(&format!("\n  {} get {} => {};\n", tstr, safe_ident(n), code));
        }
        for m in &spec.methods {
            if m.skip {
                continue;
            }
            body.push('\n');
            body.push_str(&self.method_code(&m.name, &m.func, 1));
        }

        if !stateful && body.contains("context") {
            self.scope = saved;
            return Err(());
        }

        // ---- effects
        let mut effect_methods = String::new();
        let mut run_effects = String::new();
        let mut cleanup_fields = String::new();
        let mut dispose = String::new();
        for (i, e) in spec.effects.iter().enumerate() {
            let n = i + 1;
            self.push_scope();
            // `return () => cleanup` anywhere in the effect stores the cleanup for dispose
            let body = cleanup_rewrite(&e.body, n);
            let code = self.block(&body, 2);
            let cleanup = e.cleanup.as_ref().map(|c| match c {
                Expr::Arrow(f) => {
                    let inner = self.func(f, None);
                    format!("    _cleanup{} = {};\n", n, inner)
                }
                other => {
                    let inner = self.expr(other, None);
                    format!("    _cleanup{} = {};\n", n, inner)
                }
            });
            self.pop_scope();
            effect_methods.push_str(&format!("\n  void _effect{}() {{\n{}{}  }}\n", n, code, cleanup.unwrap_or_default()));
            cleanup_fields.push_str(&format!("  VoidCallback? _cleanup{};\n", n));
            match &e.deps {
                Some(d) if d.is_empty() => cleanup_fields.push_str(&format!("  bool _ran{} = false;\n", n)),
                Some(_) => cleanup_fields.push_str(&format!("  List<dynamic>? _deps{};\n", n)),
                None => {}
            }
            dispose.push_str(&format!("    _cleanup{}?.call();\n", n));
            match &e.deps {
                None => run_effects.push_str(&format!("    _cleanup{n}?.call();\n    _cleanup{n} = null;\n    _effect{n}();\n", n = n)),
                Some(d) if d.is_empty() => run_effects.push_str(&format!("    if (!_ran{n}) {{\n      _ran{n} = true;\n      _effect{n}();\n    }}\n", n = n)),
                Some(d) => {
                    let items: Vec<String> = d.iter().map(|x| self.expr(x, None)).collect();
                    run_effects.push_str(&format!(
                        "    final deps{n} = <dynamic>[{items}];\n    if (!jsDepsEqual(_deps{n}, deps{n})) {{\n      _deps{n} = deps{n};\n      _cleanup{n}?.call();\n      _cleanup{n} = null;\n      _effect{n}();\n    }}\n",
                        n = n,
                        items = items.join(", ")
                    ));
                }
            }
        }

        // ---- build
        self.scope.in_build = true;
        let prelude_code = self.block(&std::mem::take(&mut spec.prelude), 2);
        let ret_code = match &spec.ret {
            Some(e) => {
                let c = self.expr(e, None);
                let c = self.nn_widget(c);
                if kind == Kind::Page { format!("Scaffold(body: SafeArea(child: SingleChildScrollView(child: {})))", c) } else { c }
            }
            None => "const SizedBox.shrink()".to_string(),
        };
        let render_override = spec.class_render.take();
        self.scope.in_build = false;
        let build_body = match render_override {
            Some(stmts) => {
                self.scope.in_build = true;
                let c = self.block(&stmts, 2);
                self.scope.in_build = false;
                c
            }
            None => format!("{}    return {};\n", prelude_code, ret_code),
        };
        let schedule = if spec.effects.is_empty() { String::new() } else { "    WidgetsBinding.instance.addPostFrameCallback((_) => _runEffects());\n".to_string() };
        let build = format!("\n  @override\n  Widget build(BuildContext context) {{\n{}{}  }}\n", schedule, build_body);

        if stateful {
            out.push_str(&format!("\n  @override\n  State<{}> createState() => _{}State();\n}}\n\n", name, name));
            out.push_str(&format!("class _{}State extends State<{}> {{\n", name, name));
            out.push_str(&field_lines);
            out.push_str(&cleanup_fields);
            out.push_str(&body);
            out.push_str(&effect_methods);
            if !spec.effects.is_empty() {
                out.push_str(&format!("\n  void _runEffects() {{\n    if (!mounted) return;\n{}  }}\n", run_effects));
                out.push_str(&format!("\n  @override\n  void dispose() {{\n{}    super.dispose();\n  }}\n", dispose));
            }
            out.push_str(&build);
            out.push_str("}\n");
        } else {
            out.push_str(&body);
            out.push_str(&build);
            out.push_str("}\n");
        }
        self.scope = saved;
        Ok(out)
    }

    /// A `class X extends Component<Props, State>`.
    pub fn class_component(&mut self, name: &str, extends: &Ty, members: &[ClassMember], kind: Kind) -> String {
        let mut spec = Spec::default();
        let (props_ty, state_ty) = match extends {
            Ty::Name(_, args) => (args.first().cloned(), args.get(1).cloned()),
            _ => (None, None),
        };
        if let Some(Ty::Name(n, _)) = &props_ty {
            for f in self.info.comps_by_type(n) {
                spec.props.push(PropSpec { field: f, alias: None, default: None });
            }
        }
        let state_fields: Vec<DField> = match &state_ty {
            Some(Ty::Name(n, _)) => self.info.models.get(n).cloned().unwrap_or_default(),
            _ => Vec::new(),
        };
        // the initial state: `state = {...}` or `this.state = {...}` in the constructor
        let mut initial: Vec<(String, Expr)> = Vec::new();
        for m in members {
            match &m.kind {
                MemberKind::Field(_, Some(Expr::Object(props))) if m.name == "state" => {
                    for p in props {
                        if let Prop::Kv(PropKey::Name(k), v) = p {
                            initial.push((k.clone(), v.clone()));
                        }
                    }
                }
                MemberKind::Method(f) if m.name == "constructor" => {
                    if let FnBody::Block(b) = &f.body {
                        for s in b {
                            if let Stmt::Expr(Expr::Assign(_, l, r)) = s {
                                if matches!(&**l, Expr::Member(t, n, _) if n == "state" && matches!(**t, Expr::This)) {
                                    if let Expr::Object(props) = &**r {
                                        for p in props {
                                            if let Prop::Kv(PropKey::Name(k), v) = p {
                                                initial.push((k.clone(), v.clone()));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for f in &state_fields {
            let init = initial.iter().find(|(k, _)| k == &f.name).map(|(_, v)| v.clone());
            spec.state.push(StateVar { name: f.name.clone(), setter: None, ty: Some(f.ty.clone()), init, is_ref: false });
        }
        for m in members {
            match &m.kind {
                MemberKind::Method(f) if m.name == "render" => {
                    if let FnBody::Block(b) = &f.body {
                        spec.class_render = Some(b.clone());
                    }
                }
                MemberKind::Method(_) if m.is_static || matches!(m.name.as_str(), "constructor" | "componentDidCatch" | "getDerivedStateFromError" | "getDerivedStateFromProps" | "shouldComponentUpdate") => {
                    if m.name == "componentDidCatch" || m.name == "getDerivedStateFromError" {
                        self.note(format!("{}.{}: Flutter has no error-boundary lifecycle; the fallback UI is never shown automatically (use FlutterError.onError / ErrorWidget.builder)", name, m.name));
                    }
                }
                MemberKind::Method(f) if m.name == "componentDidMount" => {
                    if let FnBody::Block(b) = &f.body {
                        spec.effects.push(EffectSpec { body: b.clone(), cleanup: None, deps: Some(Vec::new()) });
                    }
                }
                MemberKind::Method(f) if m.name == "componentWillUnmount" => {
                    if let (Some(last), FnBody::Block(b)) = (spec.effects.last_mut(), &f.body) {
                        last.cleanup = Some(Expr::Arrow(Box::new(Func { name: None, params: Vec::new(), ret: None, body: FnBody::Block(b.clone()), is_async: false })));
                    }
                }
                MemberKind::Method(f) => spec.methods.push(MethodSpec { name: m.name.clone(), func: f.clone(), skip: false }),
                MemberKind::Field(_, Some(e)) if m.name != "state" => {
                    if let Some(f) = function_of(e) {
                        spec.methods.push(MethodSpec { name: m.name.clone(), func: f.clone(), skip: false });
                    }
                }
                _ => {}
            }
        }
        // a class component is always stateful when it has state
        let saved_cc = self.scope.class_component;
        self.scope.class_component = true;
        let out = self.emit_component(name, spec, kind);
        self.scope.class_component = saved_cc;
        out
    }

    // ---------------------------------------------------------------- plain functions

    /// A top-level function (a util, a service, or a hook, which takes the BuildContext first).
    pub fn function(&mut self, name: &str, f: &Func, hook: bool) -> String {
        self.push_scope();
        let mut f2 = f.clone();
        if hook {
            f2.params.insert(0, Param { pat: Pat::Ident("context".to_string()), ty: Some(Ty::Name("BuildContext".to_string(), Vec::new())), default: None, optional: false, rest: false });
        }
        let saved = self.scope.clone();
        let mut code = self.method_code(name, &f2, 0);
        if hook {
            code = code.replacen("dynamic context", "BuildContext context", 1);
        }
        self.scope = saved;
        self.pop_scope();
        code
    }

    /// A class that is not a React component: fields, a constructor, methods (static ones stay static).
    pub fn plain_class(&mut self, name: &str, extends: Option<&Ty>, members: &[ClassMember]) -> String {
        let base = match extends {
            Some(Ty::Name(b, _)) if self.info.models.contains_key(b) || self.info.funcs.contains_key(b) => format!(" extends {}", model_dart_name(b)),
            _ => String::new(),
        };
        let mut out = format!("class {}{} {{\n", name, base);
        let saved = self.scope.clone();
        for m in members {
            self.names.insert(m.name.trim_start_matches('#').to_string());
            if let MemberKind::Field(Some(t), _) = &m.kind {
                self.declare(m.name.trim_start_matches('#'), Some(t.clone()));
            }
        }
        for m in members {
            let mname = safe_ident(&m.name.replace('#', "_"));
            match &m.kind {
                MemberKind::Field(ty, init) => {
                    let t = ty.as_ref().map(|t| self.dty(t)).unwrap_or_else(|| "dynamic".to_string());
                    let st = if m.is_static { "static " } else { "" };
                    match init {
                        Some(e) => {
                            let code = self.expr(e, ty.as_ref());
                            let late = if !m.is_static && !is_simple_literal(e) { "late " } else { "" };
                            out.push_str(&format!("  {}{}{} {} = {};\n", st, late, t, mname, code));
                        }
                        None => out.push_str(&format!("  {}{} {};\n", if m.is_static { "static " } else { "late " }, if m.is_static { nullable(&t) } else { t }, mname)),
                    }
                }
                MemberKind::Method(f) if m.name == "constructor" => {
                    self.push_scope();
                    let (params, prelude) = self.params(&f.params, true);
                    let body = match &f.body {
                        FnBody::Block(b) => self.block(b, 2),
                        FnBody::Expr(_) => String::new(),
                    };
                    self.pop_scope();
                    out.push_str(&format!("\n  {}({}) {{\n{}{}  }}\n", name, params, prelude, body));
                }
                MemberKind::Method(f) => {
                    let code = self.method_code(&mname, f, 1);
                    out.push('\n');
                    out.push_str(&if m.is_static { code.replacen("  ", "  static ", 1) } else { code });
                }
            }
        }
        self.scope = saved;
        out.push_str("}\n");
        out
    }

    /// A top-level constant: `final T name = value;`
    pub fn top_const(&mut self, name: &str, d: &VarDecl, mutable: bool) -> String {
        let ty = d.ty.clone();
        let init = d.init.as_ref();
        let (call, _targs) = init.map(|e| unwrap_targs(e)).map_or((None, &[][..]), |(c, t)| (Some(c), t));
        if let Some(c) = call {
            if callee_name(c) == Some("createContext") {
                let arg = call_args(c).first().map(|a| self.expr(a, _targs.first())).unwrap_or_else(|| "null".to_string());
                self.declare(name, None);
                return format!("final {} = ReactContext<dynamic>({});\n", safe_ident(name), arg);
            }
        }
        let code = init.map(|e| self.expr(e, ty.as_ref())).unwrap_or_else(|| "null".to_string());
        let ty_str = match (&ty, init) {
            (Some(t), _) => self.dty(t),
            (None, Some(Expr::Array(_))) => "List<dynamic>".to_string(),
            (None, Some(Expr::Object(_))) => "Map<String, dynamic>".to_string(),
            _ => "final".to_string(),
        };
        self.declare(name, ty.clone().or_else(|| init.and_then(|e| self.expr_type(e))));
        if mutable {
            let t = if ty_str == "final" { "dynamic".to_string() } else { ty_str };
            return format!("{} {} = {};\n", t, safe_ident(name), code);
        }
        if ty_str == "final" { format!("final {} = {};\n", safe_ident(name), code) } else { format!("final {} {} = {};\n", ty_str, safe_ident(name), code) }
    }
}

/// A value that can be written as a field initializer without reading the instance.
fn is_simple_literal(e: &Expr) -> bool {
    match e {
        Expr::Str(_) | Expr::Num(_) | Expr::Bool(_) | Expr::Null | Expr::Undefined => true,
        Expr::Template(_, ex) => ex.is_empty(),
        Expr::Unary(op, x) if op == "-" => matches!(**x, Expr::Num(_)),
        Expr::Array(items) => items.iter().all(is_simple_literal),
        Expr::Object(props) => props.iter().all(|p| matches!(p, Prop::Kv(PropKey::Name(_), v) if is_simple_literal(v))),
        _ => false,
    }
}

/// Every `return <fn>` of an effect body becomes `_cleanupN = <fn>; return;`.
fn cleanup_rewrite(stmts: &[Stmt], n: usize) -> Vec<Stmt> {
    stmts
        .iter()
        .map(|s| match s {
            Stmt::Return(Some(e)) => Stmt::Block(vec![Stmt::Expr(Expr::Assign("=".to_string(), Box::new(Expr::Ident(format!("_cleanup{}", n))), Box::new(e.clone()))), Stmt::Return(None)]),
            Stmt::If(c, t, e) => Stmt::If(c.clone(), Box::new(cleanup_rewrite(std::slice::from_ref(t), n).remove(0)), e.as_ref().map(|e| Box::new(cleanup_rewrite(std::slice::from_ref(e), n).remove(0)))),
            Stmt::Block(b) => Stmt::Block(cleanup_rewrite(b, n)),
            Stmt::Try(b, c, f) => Stmt::Try(cleanup_rewrite(b, n), c.as_ref().map(|c| CatchClause { param: c.param.clone(), body: cleanup_rewrite(&c.body, n) }), f.as_ref().map(|f| cleanup_rewrite(f, n))),
            other => other.clone(),
        })
        .collect()
}

fn hook_like(init: Option<&Expr>) -> bool {
    init.map_or(false, |e| matches!(callee_name(e), Some(n) if is_hook_name(n)))
}

/// A `typedef` or `enum` for a type alias / enum item.
pub fn alias_code(name: &str, ty: &Ty, models: &Models) -> Option<String> {
    if is_string_literal_union(ty) {
        let values: Vec<String> = match ty {
            Ty::Union(v) => v.iter().filter_map(|x| match x {
                Ty::Lit(l) => Some(l.trim_matches('\'').to_string()),
                _ => None,
            }).collect(),
            Ty::Lit(l) => vec![l.trim_matches('\'').to_string()],
            _ => Vec::new(),
        };
        return Some(format!("/// one of: {}\ntypedef {} = String;\n", values.iter().map(|v| str_lit(v)).collect::<Vec<_>>().join(", "), name));
    }
    match ty {
        Ty::Obj(_) | Ty::Inter(_) => None,
        other => Some(format!("typedef {} = {};\n", name, dart_type(other, models))),
    }
}

pub fn enum_code(name: &str, members: &[(String, Option<Expr>)]) -> String {
    let names: Vec<String> = members.iter().map(|(n, _)| safe_ident(n)).collect();
    format!("enum {} {{ {} }}\n", name, names.join(", "))
}

