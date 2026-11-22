//! A whole React project -> a Flutter `lib/`: reading the files, deciding where each goes, and writing them.
//!
//! Folder mapping (names become snake_case, the folder structure is kept):
//! `pages/` -> `lib/pages/`, `components/` -> `lib/components/`, `hooks/` and `contexts/` -> `lib/hooks/`,
//! `utils/`, `lib/` and `data/` -> `lib/utils/` and `lib/data/`, `services/` -> `lib/services/`, `types/` -> `lib/models/`,
//! `App.tsx` -> `lib/app.dart`, `main.tsx` -> `lib/main.dart`.

use super::ast::*;
use super::component::*;
use super::cx::*;
use super::parse::parse_module;
use super::shim;
use super::types::*;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Page,
    Component,
    Hook,
    Util,
    Service,
    Model,
    Data,
    Entry,
    Other,
}

pub struct SrcFile {
    /// path inside the source folder, with `/`
    pub rel: String,
    pub kind: Kind,
    /// path inside `lib/`, with `/`
    pub dart_rel: String,
    pub module: Module,
}

pub struct Converted {
    /// Flutter packages the converted code needs (`flutter pub add ...`)
    pub deps: Vec<String>,
    pub files: Vec<(String, String)>,
    pub notes: Vec<String>,
    pub problems: Vec<String>,
    pub skipped: Vec<String>,
}

pub fn snake(s: &str) -> String {
    let mut out = String::new();
    let cs: Vec<char> = s.chars().collect();
    for (i, &c) in cs.iter().enumerate() {
        if c == '-' || c == '.' || c == ' ' {
            if !out.ends_with('_') {
                out.push('_');
            }
            continue;
        }
        if c.is_uppercase() {
            let prev_lower = i > 0 && (cs[i - 1].is_lowercase() || cs[i - 1].is_ascii_digit());
            let next_lower = cs.get(i + 1).map_or(false, |n| n.is_lowercase());
            let prev_upper = i > 0 && cs[i - 1].is_uppercase();
            if (prev_lower || (prev_upper && next_lower)) && !out.ends_with('_') {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Where a source file goes: its kind and its path inside `lib/`.
pub fn classify(rel: &str) -> (Kind, String) {
    let no_ext = rel.rsplit_once('.').map_or(rel, |(a, _)| a);
    let mut parts: Vec<&str> = no_ext.split('/').collect();
    let file = parts.pop().unwrap_or("file");
    let file_snake = snake(file);
    let dirs: Vec<String> = parts.iter().skip(1).map(|p| snake(p)).collect();
    let top = parts.first().copied().unwrap_or("");
    let join = |root: &str| {
        let mut p = vec![root.to_string()];
        p.extend(dirs.clone());
        p.push(format!("{}.dart", file_snake));
        p.join("/")
    };
    match (top, parts.len()) {
        ("pages", n) if n >= 1 => (Kind::Page, join("pages")),
        ("components", n) if n >= 1 => (Kind::Component, join("components")),
        ("hooks", n) if n >= 1 => (Kind::Hook, join("hooks")),
        ("contexts", n) if n >= 1 => (Kind::Hook, join("hooks")),
        ("utils", n) if n >= 1 => (Kind::Util, join("utils")),
        ("lib", n) if n >= 1 => (Kind::Util, join("utils")),
        ("services", n) if n >= 1 => (Kind::Service, join("services")),
        ("types", n) if n >= 1 => (Kind::Model, join("models")),
        ("data", n) if n >= 1 => (Kind::Data, join("data")),
        _ if rel == "main.tsx" || rel == "main.ts" => (Kind::Entry, "main.dart".to_string()),
        _ if parts.is_empty() && file == "App" => (Kind::Component, "app.dart".to_string()),
        _ => {
            let mut p = parts.iter().map(|p| snake(p)).collect::<Vec<_>>();
            p.push(format!("{}.dart", file_snake));
            (Kind::Other, p.join("/"))
        }
    }
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if p.is_dir() {
            if matches!(name.as_str(), "node_modules" | ".git" | "dist" | "build") {
                continue;
            }
            walk(&p, base, out);
        } else {
            out.push(p);
        }
    }
}

pub fn scan(src: &Path) -> (Vec<SrcFile>, Vec<String>, Vec<String>) {
    let mut paths = Vec::new();
    walk(src, src, &mut paths);
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    let mut problems = Vec::new();
    for p in paths {
        let rel = p.strip_prefix(src).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if !matches!(ext, "ts" | "tsx") || rel.ends_with(".d.ts") {
            skipped.push(rel);
            continue;
        }
        let text = match fs::read_to_string(&p) {
            Ok(t) => t,
            Err(e) => {
                problems.push(format!("{}: cannot read ({})", rel, e));
                continue;
            }
        };
        let module = parse_module(&text);
        for (line, msg) in &module.problems {
            problems.push(format!("{}:{}: {}", rel, line, msg));
        }
        let (kind, dart_rel) = classify(&rel);
        files.push(SrcFile { rel, kind, dart_rel, module });
    }
    // two sources that land on the same Dart file get a numeric suffix
    let mut seen: HashMap<String, usize> = HashMap::new();
    for f in &mut files {
        let n = seen.entry(f.dart_rel.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            f.dart_rel = f.dart_rel.replace(".dart", &format!("_{}.dart", n));
        }
    }
    (files, skipped, problems)
}

fn is_external_module(from: &str) -> bool {
    !from.starts_with('.') && !from.starts_with('/') && !from.starts_with("firebase/") && !matches!(from, "react" | "react-dom" | "react-dom/client" | "lucide-react" | "react-router-dom" | "react-router") && !from.ends_with(".css")
}

/// `React.FC<Props>` / `FC<Props>` / `React.FunctionComponent<Props>` -> `Props`
pub fn fc_props(ty: &Option<Ty>) -> Option<Ty> {
    match ty {
        Some(Ty::Name(n, args)) if matches!(n.trim_start_matches("React."), "FC" | "FunctionComponent" | "VFC" | "NamedExoticComponent" | "ComponentType") => args.first().cloned(),
        _ => None,
    }
}

fn props_fields(info_models: &Models, f: &Func, hint: Option<Ty>) -> Option<Vec<DField>> {
    let p = f.params.first()?;
    let own_ty = p.ty.clone().or(hint);
    let declared = |t: &Ty| -> Vec<DField> {
        match t {
            Ty::Name(n, _) => info_models.get(n).cloned().unwrap_or_default(),
            Ty::Obj(fields) => fields.iter().map(|f| DField { name: f.name.clone(), ty: f.ty.clone(), optional: f.optional }).collect(),
            _ => Vec::new(),
        }
    };
    let mut out = own_ty.as_ref().map(declared).unwrap_or_default();
    if let Some(Ty::Inter(parts)) = &own_ty {
        out = parts.iter().flat_map(|t| declared(t)).collect();
    }
    if out.is_empty() {
        if let Pat::Object(props) = &p.pat {
            out = props.iter().filter(|x| !x.rest).map(|x| DField { name: x.key.clone(), ty: Ty::Any, optional: true }).collect();
        }
    }
    Some(out)
}

pub fn build_info(files: &[SrcFile]) -> ProjectInfo {
    let mut info = ProjectInfo::default();
    let items: Vec<&Item> = files.iter().flat_map(|f| f.module.items.iter()).collect();
    info.models = collect_models(&items);
    // `Partial<X>` is a class of its own: every field optional and assignable
    for f in files {
        let dbg = format!("{:?}", f.module.items);
        let mut rest = dbg.as_str();
        const NEEDLE: &str = "Name(\"Partial\", [Name(\"";
        while let Some(i) = rest.find(NEEDLE) {
            rest = &rest[i + NEEDLE.len()..];
            let name: String = rest.chars().take_while(|c| *c != '"').collect();
            if let Some(fields) = info.models.get(&name).cloned() {
                let partial: Vec<DField> = fields.into_iter().map(|f| DField { optional: true, ..f }).collect();
                info.models.insert(format!("{}Partial", name), partial);
            }
        }
    }
    for f in files {
        for it in &f.module.items {
            match it {
                Item::Interface { name, .. } if name.ends_with("Props") => {
                    info.props_only.insert(name.clone());
                }
                Item::Import(i) if is_external_module(&i.from) => {
                    let mut names: Vec<String> = i.named.iter().map(|(_, local)| local.clone()).collect();
                    names.extend(i.default.clone());
                    names.extend(i.namespace.clone());
                    for n in names {
                        info.externals.push((n, i.from.clone()));
                    }
                }
                Item::Func { func, .. } => register_fn(&mut info, func.name.clone().unwrap_or_default(), func, None),
                Item::Var { decls, .. } => {
                    for d in decls {
                        let Pat::Ident(n) = &d.pat else { continue };
                        if let Some(init) = &d.init {
                            let (call, _) = unwrap_targs(init);
                            if let Expr::Call(c, _, _) = call {
                                if matches!(&**c, Expr::Ident(x) if x == "createContext") {
                                    info.contexts.insert(n.clone());
                                    continue;
                                }
                            }
                            if let Some(func) = function_of(init) {
                                register_fn(&mut info, n.clone(), func, fc_props(&d.ty));
                            }
                        }
                    }
                }
                Item::Class { name, extends: Some(Ty::Name(base, args)), .. } if base.ends_with("Component") => {
                    let fields = match args.first() {
                        Some(Ty::Name(p, _)) => info.models.get(p).cloned().unwrap_or_default(),
                        _ => Vec::new(),
                    };
                    info.comps.insert(name.clone(), fields);
                }
                _ => {}
            }
        }
    }
    // props of components named in `extends React.XAttributes` already live in the models; the used-as-props interfaces
    // are those named `...Props`
    info
}

fn register_fn(info: &mut ProjectInfo, name: String, f: &Func, hint: Option<Ty>) {
    if name.is_empty() {
        return;
    }
    if is_component_name(&name) && returns_jsx(f) {
        let fields = props_fields(&info.models, f, hint).unwrap_or_default();
        info.comps.insert(name, fields);
        return;
    }
    if is_hook_name(&name) {
        info.hooks.insert(name.clone());
    }
    let params = f.params.iter().enumerate().map(|(i, p)| (match &p.pat {
        Pat::Ident(n) => n.clone(),
        _ => format!("p{}", i),
    }, p.ty.clone(), p.optional || p.default.is_some())).collect();
    info.funcs.insert(name, FuncSig { params, ret: f.ret.clone() });
}

/// Hooks take the BuildContext first: the signatures the callers see include it only in the emitted Dart, not here.
fn rel_path(from_dir: &str, to: &str) -> String {
    let a: Vec<&str> = if from_dir.is_empty() { Vec::new() } else { from_dir.split('/').collect() };
    let b: Vec<&str> = to.split('/').collect();
    let mut i = 0;
    while i < a.len() && i + 1 < b.len() && a[i] == b[i] {
        i += 1;
    }
    let mut out: Vec<String> = vec!["..".to_string(); a.len() - i];
    out.extend(b[i..].iter().map(|s| s.to_string()));
    out.join("/")
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn resolve_import(from_rel: &str, spec: &str, files: &[SrcFile]) -> Option<usize> {
    let mut segs: Vec<&str> = dir_of(from_rel).split('/').filter(|s| !s.is_empty()).collect();
    for part in spec.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                segs.pop();
            }
            p => segs.push(p),
        }
    }
    let base = segs.join("/");
    for cand in [format!("{}.tsx", base), format!("{}.ts", base), format!("{}/index.tsx", base), format!("{}/index.ts", base), base.clone()] {
        if let Some(i) = files.iter().position(|f| f.rel == cand) {
            return Some(i);
        }
    }
    None
}

/// `lazy(() => import('./pages/Scanner')...)` -> the path
fn lazy_import(e: &Expr) -> Option<String> {
    fn find(e: &Expr) -> Option<String> {
        match e {
            Expr::Call(c, args, _) => {
                if matches!(&**c, Expr::Ident(n) if n == "import") {
                    if let Some(Expr::Str(s)) = args.first() {
                        return Some(s.clone());
                    }
                }
                find(c).or_else(|| args.iter().find_map(find))
            }
            Expr::Arrow(f) => match &f.body {
                FnBody::Expr(b) => find(b),
                FnBody::Block(b) => b.iter().find_map(|s| match s {
                    Stmt::Return(Some(e)) => find(e),
                    _ => None,
                }),
            },
            Expr::Member(o, _, _) => find(o),
            Expr::As(x, _) => find(x),
            _ => None,
        }
    }
    let (call, _) = unwrap_targs(e);
    match call {
        Expr::Call(c, args, _) if matches!(&**c, Expr::Ident(n) if n == "lazy") => args.first().and_then(find),
        _ => None,
    }
}

fn first_jsx(e: &Expr) -> Option<Jsx> {
    match e {
        Expr::Jsx(j) => Some((**j).clone()),
        Expr::Call(c, args, _) => first_jsx(c).or_else(|| args.iter().find_map(first_jsx)),
        Expr::Member(o, _, _) => first_jsx(o),
        _ => None,
    }
}

fn collect_jsx_tags(e: &Expr, out: &mut Vec<String>) {
    fn walk(j: &Jsx, out: &mut Vec<String>) {
        out.push(j.tag.clone());
        for k in &j.children {
            if let JsxChild::Element(c) = k {
                walk(c, out);
            }
        }
    }
    match e {
        Expr::Jsx(j) => walk(j, out),
        Expr::Call(c, args, _) => {
            collect_jsx_tags(c, out);
            for a in args {
                collect_jsx_tags(a, out);
            }
        }
        Expr::Member(o, _, _) => collect_jsx_tags(o, out),
        _ => {}
    }
}

fn emit_item(cx: &mut Cx, f: &SrcFile, it: &Item, lazy_names: &HashSet<String>) -> String {
    let mut out = String::new();
    match it {
        Item::Import(_) | Item::ExportNames(_) | Item::ExportDefault(_) => {}
        Item::Interface { name, .. } => {
            if !cx.info.props_only.contains(name) {
                if let Some(fields) = cx.info.models.get(name) {
                    out.push_str(&model_class(&model_dart_name(name), fields, &cx.info.models));
                    out.push('\n');
                }
                let pname = format!("{}Partial", name);
                if let Some(fields) = cx.info.models.get(&pname) {
                    out.push_str(&model_class(&model_dart_name(&pname), fields, &cx.info.models));
                    out.push('\n');
                }
            }
        }
        Item::TypeAlias { name, ty, .. } => {
            if let Some(code) = alias_code(name, ty, &cx.info.models) {
                out.push_str(&code);
                out.push('\n');
            } else if matches!(ty, Ty::Obj(_) | Ty::Inter(_)) && !cx.info.props_only.contains(name) {
                if let Some(fields) = cx.info.models.get(name) {
                    out.push_str(&model_class(&model_dart_name(name), fields, &cx.info.models));
                    out.push('\n');
                }
            }
        }
        Item::Enum { name, members, .. } => {
            out.push_str(&enum_code(name, members));
            out.push('\n');
        }
        Item::Func { func, .. } => {
            let name = func.name.clone().unwrap_or_default();
            if name.is_empty() {
                return out;
            }
            if is_component_name(&name) && returns_jsx(func) {
                out.push_str(&cx.component(&name, func, f.kind));
            } else {
                out.push_str(&cx.function(&name, func, is_hook_name(&name)));
            }
            out.push('\n');
        }
        Item::Var { decls, kind, .. } => {
            for d in decls {
                let Pat::Ident(n) = &d.pat else {
                    cx.note("a destructuring declaration at the top level is not converted");
                    continue;
                };
                if lazy_names.contains(n) {
                    continue;
                }
                match &d.init {
                    Some(init) if function_of(init).is_some() => {
                        let func = function_of(init).unwrap();
                        if is_component_name(n) && returns_jsx(func) {
                            cx.props_hint = fc_props(&d.ty);
                            out.push_str(&cx.component(n, func, f.kind));
                            cx.props_hint = None;
                        } else {
                            let mut func = func.clone();
                            func.name = Some(n.clone());
                            out.push_str(&cx.function(n, &func, is_hook_name(n)));
                        }
                        out.push('\n');
                    }
                    _ => {
                        if let Some(Expr::Str(s)) = &d.init {
                            cx.consts.insert(n.clone(), s.clone());
                        }
                        let _ = kind;
                        out.push_str(&cx.top_const(n, d, kind != "const"));
                        out.push('\n');
                    }
                }
            }
        }
        Item::Class { name, extends, members, .. } => match extends {
            Some(Ty::Name(base, _)) if base.ends_with("Component") => {
                out.push_str(&cx.class_component(name, extends.as_ref().unwrap(), members, f.kind));
                out.push('\n');
            }
            _ => {
                out.push_str(&cx.plain_class(name, extends.as_ref(), members));
                out.push('\n');
            }
        },
        Item::Stmt(_) => {}
    }
    out
}

/// One file's Dart source.
/// (Dart source, notes, the FirebaseOptions arguments found in this file)
fn emit_file(info: &ProjectInfo, files: &[SrcFile], idx: usize, app_title: &str, fb_options: bool) -> (String, Vec<String>, Option<String>) {
    let f = &files[idx];
    let mut cx = Cx::new(info, &f.rel);
    cx.app_title = app_title.to_string();
    // lucide names and lazy pages
    let mut imports: BTreeSet<String> = BTreeSet::new();
    let mut lazy_names: HashSet<String> = HashSet::new();
    let mut ext_names: Vec<String> = Vec::new();
    let mut fb_modules: BTreeSet<String> = BTreeSet::new();
    let mut prefixed: Vec<(String, String)> = Vec::new();
    let dir = dir_of(&f.dart_rel).to_string();
    let to_root = |p: &str| rel_path(&dir, p);
    for it in &f.module.items {
        match it {
            Item::Import(i) => {
                if i.from == "lucide-react" {
                    for (orig, local) in &i.named {
                        cx.lucide_imports.insert(local.clone());
                        if orig != local {
                            cx.lucide_alias.insert(local.clone(), orig.clone());
                        }
                    }
                } else if i.from.starts_with("firebase/") {
                    cx.firebase_used = true;
                    fb_modules.insert(i.from.clone());
                    for (orig, local) in &i.named {
                        cx.firebase.insert(local.clone(), orig.clone());
                    }
                } else if is_external_module(&i.from) {
                    ext_names.extend(i.named.iter().map(|(_, l)| l.clone()));
                    ext_names.extend(i.default.clone());
                    ext_names.extend(i.namespace.clone());
                } else if i.from.starts_with('.') {
                    if let Some(t) = resolve_import(&f.rel, &i.from, files) {
                        // `import { a as b }`: read through a prefix, so a local named `a` cannot hide it
                        for (orig, local) in &i.named {
                            if orig != local {
                                let pfx = format!("imp{}", prefixed.len() + 1);
                                prefixed.push((to_root(&files[t].dart_rel), pfx.clone()));
                                cx.import_alias.insert(local.clone(), format!("{}.{}", pfx, orig));
                            }
                        }
                        if t != idx {
                            imports.insert(to_root(&files[t].dart_rel));
                        }
                    }
                }
            }
            Item::Var { decls, .. } => {
                for d in decls {
                    if let (Pat::Ident(n), Some(init)) = (&d.pat, &d.init) {
                        if let Some(path) = lazy_import(init) {
                            lazy_names.insert(n.clone());
                            if let Some(t) = resolve_import(&f.rel, &path, files) {
                                imports.insert(to_root(&files[t].dart_rel));
                            }
                        }
                        if let Expr::Str(s) = init {
                            cx.consts.insert(n.clone(), s.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // the entry file: runApp(<the whole tree of the render call>)
    if f.kind == Kind::Entry {
        let mut tags = Vec::new();
        let mut tree: Option<Jsx> = None;
        for it in &f.module.items {
            if let Item::Stmt(Stmt::Expr(e)) = it {
                collect_jsx_tags(e, &mut tags);
                if tree.is_none() {
                    tree = first_jsx(e);
                }
            }
        }
        let mut imp = String::new();
        let mut seen = HashSet::new();
        for t in tags.iter().filter(|t| info.comps.contains_key(*t)) {
            let target = files.iter().position(|x| x.module.items.iter().any(|it| matches!(it, Item::Func { func, .. } if func.name.as_deref() == Some(t.as_str())) || matches!(it, Item::Var { decls, .. } if decls.iter().any(|d| matches!(&d.pat, Pat::Ident(n) if n == t)))));
            if let Some(t) = target {
                if seen.insert(t) {
                    imp.push_str(&format!("import '{}';\n", rel_path("", &files[t].dart_rel)));
                }
            }
        }
        let widget = match &tree {
            Some(j) => cx.jsx_widget(j),
            None => "const SizedBox.shrink()".to_string(),
        };
        let uses_firebase = files.iter().any(|x| x.module.items.iter().any(|it| matches!(it, Item::Import(i) if i.from.starts_with("firebase/"))));
        let code = if uses_firebase {
            let (opts_import, opts_arg) = if fb_options { ("import 'firebase_options.dart';\n", "options: DefaultFirebaseOptions.currentPlatform") } else { ("", "") };
            format!(
                "import 'package:firebase_core/firebase_core.dart';\nimport 'package:flutter/material.dart';\n\n{}{}\nFuture<void> main() async {{\n  WidgetsFlutterBinding.ensureInitialized();\n  await Firebase.initializeApp({});\n  runApp({});\n}}\n",
                opts_import, imp, opts_arg, widget
            )
        } else {
            format!("import 'package:flutter/material.dart';\n\n{}\nvoid main() {{\n  runApp({});\n}}\n", imp, widget)
        };
        return (code, cx.notes, None);
    }
    let mut body = String::new();
    for it in &f.module.items {
        body.push_str(&emit_item(&mut cx, f, it, &lazy_names));
    }
    // names the project defines that Flutter's material library also exports are hidden from the Flutter import
    const FLUTTER_NAMES: &[&str] = &["Card", "Notification", "Form", "Image", "Icon", "Text", "Switch", "Slider", "Radio", "Checkbox", "Chip", "Badge", "Banner", "Tooltip", "Stepper", "Table", "Dialog", "Drawer", "Theme", "Overlay", "Navigator", "Placeholder", "Divider", "Builder", "Material", "Scaffold", "Container", "Router", "Action", "Intent", "Title", "Page"];
    let defined: Vec<String> = FLUTTER_NAMES.iter().filter(|n| info.comps.contains_key(**n) || info.models.contains_key(**n)).map(|n| n.to_string()).collect();
    let own: Vec<&String> = defined.iter().filter(|n| !f.module.items.iter().any(|it| matches!(it, Item::Func { func, .. } if func.name.as_deref() == Some(n.as_str())) || matches!(it, Item::Var { decls, .. } if decls.iter().any(|d| matches!(&d.pat, Pat::Ident(x) if x == *n))))).collect();
    let _ = own;
    // the config object handed to initializeApp becomes FirebaseOptions
    let mut options: Option<String> = None;
    if let Some(cfg) = cx.firebase_config_of.clone() {
        for it in &f.module.items {
            if let Item::Var { decls, .. } = it {
                for d in decls {
                    if matches!(&d.pat, Pat::Ident(n) if *n == cfg) {
                        if let Some(init) = &d.init {
                            options = cx.firebase_options_from(init);
                        }
                    }
                }
            }
        }
    }
    let word = |w: &str| regex::Regex::new(&format!(r"\b{}\b", regex::escape(w))).map_or(true, |re| re.is_match(&body));
    let uses_stubs = ext_names.iter().any(|n| word(n));
    let needs_material = ["Widget", "BuildContext", "Icons.", "Color(", "Colors.", "EdgeInsets", "TextStyle", "debugPrint", "VoidCallback", "IconData", "State<", "Navigator", "MaterialApp", "SizedBox", "Container(", "Text("].iter().any(|k| body.contains(k));
    let mut header = if !needs_material {
        String::new()
    } else if defined.is_empty() {
        String::from("import 'package:flutter/material.dart';\n")
    } else {
        format!("import 'package:flutter/material.dart' hide {};\n", defined.join(", "))
    };
    if body.contains("jsonEncode(") || body.contains("jsonDecode(") {
        header.push_str("import 'dart:convert';\n");
    }
    if body.contains("Random()") || body.contains("pow(") || body.contains("sqrt(") || body.contains("pi)") {
        header.push_str("import 'dart:math' show Random, pow, sqrt, pi;\n");
    }
    // the compatibility file only when the code calls something from it
    let uses_compat = regex::Regex::new(r"\b(js[A-Z]\w*|webOnly|LocalStorage|SessionStorage|Ref<|ReactContext|navigate\(|useLocation|useParams|Navigate\(|ControlledField|FlexColumn|envVar|openUrl|JsResponse|jsErrorMessage|jsErrorCode)").map_or(true, |re| re.is_match(&body));
    if uses_compat {
        header.push_str(&format!("import '{}';\n", to_root("utils/js_compat.dart")));
    }
    if body.contains("Tw.") {
        header.push_str(&format!("import '{}';\n", to_root("utils/tw.dart")));
    }
    if uses_stubs {
        header.push_str(&format!("import '{}';\n", to_root("utils/external_stubs.dart")));
    }
    for m in &fb_modules {
        let pkg = match m.as_str() {
            "firebase/auth" => "firebase_auth/firebase_auth",
            "firebase/firestore" => "cloud_firestore/cloud_firestore",
            "firebase/app" => "firebase_core/firebase_core",
            _ => continue,
        };
        header.push_str(&format!("import 'package:{}.dart';\n", pkg));
    }
    for i in &imports {
        header.push_str(&format!("import '{}';\n", i));
    }
    for (path, pfx) in &prefixed {
        header.push_str(&format!("import '{}' as {};\n", path, pfx));
    }
    let uses_keep = body.contains("_keep");
    let keep = if uses_keep { "\nconst Object _keep = Object();\n" } else { "" };
    (format!("{}{}\n{}", header, keep, body), cx.notes, options)
}

pub fn convert_project(files: &[SrcFile], info: &ProjectInfo, app_title: &str) -> Converted {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let mut fb_options: Option<String> = None;
    // the entry file last: it needs to know whether FirebaseOptions were found
    let order: Vec<usize> = (0..files.len()).filter(|i| files[*i].kind != Kind::Entry).chain((0..files.len()).filter(|i| files[*i].kind == Kind::Entry)).collect();
    for i in order {
        let (code, n, opts) = emit_file(info, files, i, app_title, fb_options.is_some());
        if opts.is_some() {
            fb_options = opts;
        }
        out.push((files[i].dart_rel.clone(), code));
        notes.extend(n);
    }
    if let Some(params) = &fb_options {
        out.push((
            "firebase_options.dart".to_string(),
            format!(
                "// Generated by `mysid convert` from the Firebase config of the React app.\n// The values come from the environment: flutter run --dart-define-from-file=.env  (the same VITE_FIREBASE_* names)\nimport 'package:firebase_core/firebase_core.dart';\n\nclass DefaultFirebaseOptions {{\n  static FirebaseOptions get currentPlatform => FirebaseOptions(\n{}\n  );\n}}\n",
                params
            ),
        ));
    }
    let mut deps: Vec<String> = Vec::new();
    for (_, code) in &out {
        for (needle, pkg) in [("package:firebase_core/", "firebase_core"), ("package:firebase_auth/", "firebase_auth"), ("package:cloud_firestore/", "cloud_firestore")] {
            if code.contains(needle) && !deps.iter().any(|d| d == pkg) {
                deps.push(pkg.to_string());
            }
        }
    }
    out.push(("utils/js_compat.dart".to_string(), shim::JS_COMPAT.to_string()));
    let uses_tw = out.iter().any(|(_, c)| c.contains("utils/tw.dart'"));
    if uses_tw {
        out.push(("utils/tw.dart".to_string(), shim::tw_dart()));
    }
    if out.iter().any(|(_, c)| c.contains("external_stubs.dart'")) {
        let models: HashSet<&String> = info.models.keys().collect();
        let externals: Vec<(String, String)> = info.externals.iter().filter(|(n, _)| !models.contains(n)).cloned().collect();
        out.push(("utils/external_stubs.dart".to_string(), shim::external_stubs(&externals)));
    }
    Converted { deps, files: out, notes, problems: Vec::new(), skipped: Vec::new() }
}

/// Replaces `<dest>/lib` with the converted files; the old `lib` is kept in `<dest>/.mysid/lib-backup-<time>`.
pub fn write_lib(dest: &Path, files: &[(String, String)]) -> Result<Option<PathBuf>, String> {
    let lib = dest.join("lib");
    let staging = dest.join(".mysid").join("lib-new");
    let _ = fs::remove_dir_all(&staging);
    for (rel, code) in files {
        let p = staging.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        fs::write(&p, code).map_err(|e| format!("cannot write {}: {}", p.display(), e))?;
    }
    let mut backup = None;
    if lib.exists() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let b = dest.join(".mysid").join(format!("lib-backup-{}", stamp));
        fs::create_dir_all(dest.join(".mysid")).map_err(|e| e.to_string())?;
        fs::rename(&lib, &b).map_err(|e| format!("cannot move the old lib away ({}): nothing was replaced", e))?;
        backup = Some(b);
    }
    fs::rename(&staging, &lib).map_err(|e| format!("cannot put the new lib in place: {}", e))?;
    Ok(backup)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_become_snake_case_in_the_right_folder() {
        assert_eq!(classify("pages/auth/Login.tsx"), (Kind::Page, "pages/auth/login.dart".to_string()));
        assert_eq!(classify("pages/guard/GuardLogin.tsx"), (Kind::Page, "pages/guard/guard_login.dart".to_string()));
        assert_eq!(classify("components/UI/Button.tsx"), (Kind::Component, "components/ui/button.dart".to_string()));
        assert_eq!(classify("contexts/AuthContext.tsx"), (Kind::Hook, "hooks/auth_context.dart".to_string()));
        assert_eq!(classify("types/auth.types.ts"), (Kind::Model, "models/auth_types.dart".to_string()));
        assert_eq!(classify("lib/firebase.ts"), (Kind::Util, "utils/firebase.dart".to_string()));
        assert_eq!(classify("utils/cryptoUtils.ts"), (Kind::Util, "utils/crypto_utils.dart".to_string()));
        assert_eq!(classify("App.tsx"), (Kind::Component, "app.dart".to_string()));
        assert_eq!(classify("main.tsx"), (Kind::Entry, "main.dart".to_string()));
    }

    #[test]
    fn relative_import_paths_are_computed() {
        assert_eq!(rel_path("pages/auth", "components/ui/button.dart"), "../../components/ui/button.dart");
        assert_eq!(rel_path("", "app.dart"), "app.dart");
        assert_eq!(rel_path("pages", "pages/home.dart"), "home.dart");
    }
}
