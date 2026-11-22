//! JSX + Tailwind `Style` -> Flutter widgets.
//!
//! Layout follows CSS the way a phone shows it: `flex` is a Row/Column, a plain `div` is a Column that stretches its children,
//! `gap-N` / `space-y-N` become `spacing:`, padding / background / border / radius / shadow become one `Container`, and
//! `absolute` children go into a `Stack`. Text styling flows down to the text inside, like CSS inheritance.
//! A `className` that is computed at run time is handed to `Tw` (the small runtime that ships with the output).

use super::ast::*;
use super::cx::*;
use super::expr::str_lit;
use super::tailwind::{self, Rgba, Size, Style};
use super::types::*;

#[derive(Clone, Default, Debug)]
pub struct Inherit {
    pub color: Option<Rgba>,
    pub size: Option<f64>,
    pub weight: Option<u16>,
    pub align: Option<String>,
    pub leading: Option<f64>,
    pub italic: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Parent {
    Block,
    Row,
    Column,
    Stack,
}

#[derive(Clone, Default, Debug)]
pub struct Abs {
    pub inset: [Option<Size>; 4],
    pub tx: Option<f64>,
    pub ty: Option<f64>,
}

#[derive(Clone, Default, Debug)]
pub struct Built {
    pub code: String,
    pub abs: Option<Abs>,
    pub expanded: bool,
    pub fixed_w: bool,
    /// an `if (..) w` / `for (..) w` element of a list: it cannot be wrapped again
    pub collection: bool,
    pub self_center: bool,
}

pub fn lucide(name: &str) -> Option<&'static str> {
    Some(match name {
        "AlertCircle" => "Icons.error_outline",
        "AlertTriangle" | "TriangleAlert" => "Icons.warning_amber_rounded",
        "ArrowLeft" => "Icons.arrow_back",
        "ArrowRight" => "Icons.arrow_forward",
        "ArrowUp" => "Icons.arrow_upward",
        "ArrowDown" => "Icons.arrow_downward",
        "Building" | "Building2" => "Icons.apartment",
        "Bell" => "Icons.notifications_none",
        "Calendar" => "Icons.calendar_today",
        "Briefcase" => "Icons.work_outline",
        "ClipboardList" | "Clipboard" => "Icons.assignment_outlined",
        "Camera" => "Icons.photo_camera",
        "Check" => "Icons.check",
        "CheckCircle" | "CheckCircle2" => "Icons.check_circle_outline",
        "ChevronDown" => "Icons.keyboard_arrow_down",
        "ChevronUp" => "Icons.keyboard_arrow_up",
        "ChevronLeft" => "Icons.chevron_left",
        "ChevronRight" => "Icons.chevron_right",
        "Clock" => "Icons.access_time",
        "Copy" => "Icons.content_copy",
        "Download" => "Icons.download",
        "Edit" | "Edit2" | "Pencil" => "Icons.edit_outlined",
        "ExternalLink" => "Icons.open_in_new",
        "Eye" => "Icons.visibility_outlined",
        "EyeOff" => "Icons.visibility_off_outlined",
        "FileText" => "Icons.description_outlined",
        "Filter" => "Icons.filter_list",
        "Globe" => "Icons.language",
        "Heart" => "Icons.favorite_border",
        "Home" => "Icons.home_outlined",
        "Image" => "Icons.image_outlined",
        "Info" => "Icons.info_outline",
        "Key" => "Icons.vpn_key_outlined",
        "Loader2" | "Loader" | "RefreshCcw" => "Icons.autorenew",
        "Lock" => "Icons.lock_outline",
        "LogIn" => "Icons.login",
        "LogOut" => "Icons.logout",
        "Mail" => "Icons.mail_outline",
        "MapPin" => "Icons.place_outlined",
        "Menu" => "Icons.menu",
        "Minus" => "Icons.remove",
        "MoreVertical" => "Icons.more_vert",
        "Phone" => "Icons.phone_outlined",
        "Play" => "Icons.play_arrow",
        "Plus" => "Icons.add",
        "Printer" => "Icons.print_outlined",
        "QrCode" => "Icons.qr_code",
        "RefreshCw" => "Icons.refresh",
        "RotateCcw" => "Icons.rotate_left",
        "Save" => "Icons.save_outlined",
        "ScanLine" => "Icons.qr_code_scanner",
        "Search" => "Icons.search",
        "Send" => "Icons.send",
        "Settings" => "Icons.settings_outlined",
        "Share2" | "Share" => "Icons.share",
        "Shield" => "Icons.shield_outlined",
        "ShieldCheck" => "Icons.verified_user_outlined",
        "Square" => "Icons.crop_square",
        "Star" => "Icons.star_border",
        "Trash" | "Trash2" => "Icons.delete_outline",
        "TrendingUp" => "Icons.trending_up",
        "Upload" => "Icons.upload",
        "User" => "Icons.person_outline",
        "UserCog" => "Icons.manage_accounts_outlined",
        "UserPlus" => "Icons.person_add_alt_1_outlined",
        "Users" => "Icons.group_outlined",
        "X" => "Icons.close",
        "XCircle" => "Icons.cancel_outlined",
        _ => return None,
    })
}

/// A `px` size as a Dart number (`16` or `16.5`).
fn px(v: f64) -> String {
    if v.fract() == 0.0 { format!("{}", v as i64) } else { format!("{}", v) }
}

fn edge_insets(a: &[Option<f64>; 4]) -> Option<String> {
    if a.iter().all(|x| x.is_none()) {
        return None;
    }
    let v = |i: usize| a[i].unwrap_or(0.0);
    let (t, r, b, l) = (v(0), v(1), v(2), v(3));
    Some(if t == r && r == b && b == l {
        format!("EdgeInsets.all({})", px(t))
    } else if t == b && l == r {
        match (l == 0.0, t == 0.0) {
            (true, _) => format!("EdgeInsets.symmetric(vertical: {})", px(t)),
            (_, true) => format!("EdgeInsets.symmetric(horizontal: {})", px(l)),
            _ => format!("EdgeInsets.symmetric(horizontal: {}, vertical: {})", px(l), px(t)),
        }
    } else {
        format!("EdgeInsets.fromLTRB({}, {}, {}, {})", px(l), px(t), px(r), px(b))
    })
}

fn border_radius(r: &[Option<f64>; 4]) -> Option<String> {
    if r.iter().all(|x| x.is_none()) {
        return None;
    }
    let v = |i: usize| r[i].unwrap_or(0.0).min(999.0);
    if r.iter().all(|x| *x == r[0]) {
        return Some(format!("BorderRadius.circular({})", px(v(0))));
    }
    Some(format!("BorderRadius.only(topLeft: Radius.circular({}), topRight: Radius.circular({}), bottomRight: Radius.circular({}), bottomLeft: Radius.circular({}))", px(v(0)), px(v(1)), px(v(2)), px(v(3))))
}

fn shadow(level: &str) -> String {
    let (alpha, blur, dy) = match level {
        "sm" => (0x0D, 2.0, 1.0),
        "base" => (0x1A, 3.0, 1.0),
        "md" => (0x1A, 6.0, 4.0),
        "lg" => (0x1A, 15.0, 10.0),
        "xl" => (0x1A, 25.0, 20.0),
        "2xl" => (0x40, 50.0, 25.0),
        _ => (0x0D, 2.0, 1.0),
    };
    format!("[BoxShadow(color: Color(0x{:02X}000000), blurRadius: {}, offset: Offset(0, {}))]", alpha, px(blur), px(dy))
}

fn alignment_of(x: &str, y: &str) -> String {
    format!("Alignment({}, {})", x, y)
}

fn main_axis(j: &str) -> &'static str {
    match j {
        "center" => "MainAxisAlignment.center",
        "end" => "MainAxisAlignment.end",
        "between" => "MainAxisAlignment.spaceBetween",
        "around" => "MainAxisAlignment.spaceAround",
        "evenly" => "MainAxisAlignment.spaceEvenly",
        _ => "MainAxisAlignment.start",
    }
}

fn cross_axis(i: &str) -> &'static str {
    match i {
        "center" => "CrossAxisAlignment.center",
        "end" => "CrossAxisAlignment.end",
        "stretch" => "CrossAxisAlignment.stretch",
        "baseline" => "CrossAxisAlignment.center",
        _ => "CrossAxisAlignment.start",
    }
}

fn text_align(a: &str) -> &'static str {
    match a {
        "center" => "TextAlign.center",
        "right" => "TextAlign.right",
        "justify" => "TextAlign.justify",
        _ => "TextAlign.left",
    }
}

/// `TextStyle(...)` for a style merged over the inherited text properties, or None when nothing is set.
fn text_style(st: &Style, inh: &Inherit) -> Option<String> {
    let color = st.color.or(inh.color);
    let size = st.size.or(inh.size);
    let weight = st.weight.or(inh.weight);
    let leading = if st.size.is_some() { st.leading } else { st.leading.or(inh.leading) };
    let mut parts = Vec::new();
    if let Some(c) = color {
        parts.push(format!("color: {}", c.dart()));
    }
    if let Some(s) = size {
        parts.push(format!("fontSize: {}", px(s)));
    }
    if let Some(w) = weight {
        parts.push(format!("fontWeight: FontWeight.w{}", w));
    }
    if let Some(l) = leading {
        parts.push(format!("height: {}", (l * 1000.0).round() / 1000.0));
    }
    if st.italic || inh.italic {
        parts.push("fontStyle: FontStyle.italic".to_string());
    }
    if let Some(t) = st.tracking {
        parts.push(format!("letterSpacing: {}", t));
    }
    let mut deco = Vec::new();
    if st.underline {
        deco.push("TextDecoration.underline");
    }
    if st.line_through {
        deco.push("TextDecoration.lineThrough");
    }
    if !deco.is_empty() {
        parts.push(format!("decoration: {}", if deco.len() == 1 { deco[0].to_string() } else { format!("TextDecoration.combine([{}])", deco.join(", ")) }));
    }
    if parts.is_empty() { None } else { Some(format!("TextStyle({})", parts.join(", "))) }
}

/// The text properties that flow down from this element.
fn merge_inherit(st: &Style, inh: &Inherit) -> Inherit {
    Inherit {
        color: st.color.or(inh.color),
        size: st.size.or(inh.size),
        weight: st.weight.or(inh.weight),
        align: st.align.clone().or_else(|| inh.align.clone()),
        leading: if st.size.is_some() { st.leading } else { st.leading.or(inh.leading) },
        italic: st.italic || inh.italic,
    }
}

fn size_code(s: &Size, axis_w: bool) -> String {
    match s {
        Size::Px(p) => px(*p),
        Size::Full => "double.infinity".to_string(),
        Size::Screen => if axis_w { "MediaQuery.of(context).size.width".to_string() } else { "MediaQuery.of(context).size.height".to_string() },
        Size::Frac(_) | Size::Auto => "double.infinity".to_string(),
    }
}

fn attr_str(j: &Jsx, name: &str) -> Option<String> {
    j.attrs.iter().find_map(|a| match a {
        Attr::Kv(n, AttrVal::Str(s)) if n == name => Some(s.clone()),
        _ => None,
    })
}

fn attr<'j>(j: &'j Jsx, name: &str) -> Option<&'j AttrVal> {
    j.attrs.iter().find_map(|a| match a {
        Attr::Kv(n, v) if n == name => Some(v),
        _ => None,
    })
}

fn is_text_tag(t: &str) -> bool {
    matches!(t, "p" | "span" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "label" | "strong" | "b" | "em" | "i" | "small" | "code" | "pre" | "legend" | "summary" | "dt" | "dd" | "time" | "mark" | "u" | "kbd" | "abbr" | "cite")
}

fn is_inline_text_tag(t: &str) -> bool {
    matches!(t, "span" | "strong" | "b" | "em" | "i" | "small" | "code" | "u" | "mark" | "a" | "time" | "kbd" | "abbr" | "cite")
}

fn default_style_for(tag: &str, st: &mut Style) {
    match tag {
        "strong" | "b" if st.weight.is_none() => st.weight = Some(700),
        "em" | "i" => st.italic = true,
        "small" if st.size.is_none() => st.size = Some(13.0),
        "code" | "pre" | "kbd" => {}
        "u" => st.underline = true,
        "a" if st.color.is_none() => st.color = Some(Rgba::rgb(0x2563EB)),
        _ => {}
    }
}

impl<'a> Cx<'a> {
    /// A JSX element as a Dart widget expression (the entry point from expressions).
    pub fn jsx_widget(&mut self, j: &Jsx) -> String {
        let b = self.element(j, &Inherit::default(), Parent::Block);
        b.code
    }

    // ---------------------------------------------------------------- class names

    /// (static style, dynamic class expression as Dart): a className that folds to a constant string is static.
    fn class_of(&mut self, j: &Jsx) -> (Style, Option<String>) {
        match attr(j, "className").or_else(|| attr(j, "class")) {
            None => (Style::default(), None),
            Some(AttrVal::Str(s)) => (tailwind::parse(s), None),
            Some(AttrVal::True) => (Style::default(), None),
            Some(AttrVal::Expr(e)) => match self.fold_class(e) {
                Some(s) => (tailwind::parse(&s), None),
                None => {
                    self.uses_tw = true;
                    let code = self.class_expr(e);
                    (Style::default(), Some(code))
                }
            },
        }
    }

    fn fold_class(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Str(s) => Some(s.clone()),
            Expr::Template(q, ex) => {
                let mut out = String::new();
                for (i, quasi) in q.iter().enumerate() {
                    out.push_str(quasi);
                    if let Some(x) = ex.get(i) {
                        out.push_str(&self.fold_class(x)?);
                    }
                }
                Some(out)
            }
            Expr::Ident(n) => self.consts.get(n).cloned(),
            Expr::Binary(op, l, r) if op == "+" => Some(format!("{}{}", self.fold_class(l)?, self.fold_class(r)?)),
            Expr::Cond(..) | Expr::Binary(..) => None,
            _ => None,
        }
    }

    /// A class expression as a Dart string: constant parts folded, the rest left as Dart expressions.
    fn class_expr(&mut self, e: &Expr) -> String {
        match e {
            Expr::Template(q, ex) => {
                let mut out = String::from("'");
                for (i, quasi) in q.iter().enumerate() {
                    out.push_str(&quasi.replace('\\', "\\\\").replace('\'', "\\'").replace('$', "\\$").replace('\n', " "));
                    if let Some(x) = ex.get(i) {
                        match self.fold_class(x) {
                            Some(s) => out.push_str(&s.replace('\\', "\\\\").replace('\'', "\\'").replace('$', "\\$")),
                            None => {
                                let c = self.expr(x, None);
                                out.push_str(&format!("${{{}}}", c));
                            }
                        }
                    }
                }
                out.push('\'');
                out
            }
            other => match self.fold_class(other) {
                Some(s) => str_lit(&s),
                None => format!("'${{{}}}'", self.expr(other, None)),
            },
        }
    }

    // ---------------------------------------------------------------- children

    fn flatten<'j>(&self, kids: &'j [JsxChild], out: &mut Vec<&'j JsxChild>) {
        for k in kids {
            match k {
                JsxChild::Element(e) if e.tag.is_empty() || matches!(e.tag.as_str(), "Fragment" | "React.Fragment" | "Suspense" | "StrictMode" | "React.StrictMode") => self.flatten(&e.children, out),
                other => out.push(other),
            }
        }
    }

    pub fn children(&mut self, kids: &[JsxChild], inh: &Inherit, parent: Parent) -> Vec<Built> {
        let mut flat = Vec::new();
        self.flatten(kids, &mut flat);
        let mut out = Vec::new();
        for k in flat {
            match k {
                JsxChild::Text(t) => {
                    if t.trim().is_empty() {
                        continue;
                    }
                    let style = text_style(&Style::default(), inh).map(|s| format!(", style: {}", s)).unwrap_or_default();
                    let align = inh.align.as_ref().map(|a| format!(", textAlign: {}", text_align(a))).unwrap_or_default();
                    out.push(Built { code: format!("Text({}{}{})", str_lit(t.trim()), align, style), ..Default::default() });
                }
                JsxChild::Element(e) => out.push(self.element(e, inh, parent)),
                JsxChild::Expr(e) => {
                    if let Some(b) = self.child_expr(e, inh, parent) {
                        out.push(b);
                    }
                }
            }
        }
        out
    }

    /// `{expr}` in a child list: a conditional, a list from `.map`, a widget, or text.
    fn child_expr(&mut self, e: &Expr, inh: &Inherit, parent: Parent) -> Option<Built> {
        match e {
            Expr::Null | Expr::Undefined | Expr::Bool(false) => None,
            Expr::Jsx(j) => {
                if j.tag.is_empty() {
                    // a fragment in an expression: its children as one column
                    let kids = self.children(&j.children, inh, parent);
                    return Some(Built { code: self.stack_of(kids, "Column"), ..Default::default() });
                }
                Some(self.element(j, inh, parent))
            }
            Expr::Binary(op, l, r) if op == "&&" => {
                let c = super::expr::unparen(&self.cond(l));
                let inner = self.child_expr(r, inh, parent)?;
                Some(Built { code: format!("if ({}) {}", c, inner.code), collection: true, ..Default::default() })
            }
            Expr::Cond(c, t, f) => {
                let test = super::expr::unparen(&self.cond(c));
                let a = self.child_expr(t, inh, parent);
                let b = self.child_expr(f, inh, parent);
                match (a, b) {
                    (Some(a), Some(b)) => Some(Built { code: format!("if ({}) {} else {}", test, a.code, b.code), collection: true, ..Default::default() }),
                    (Some(a), None) => Some(Built { code: format!("if ({}) {}", test, a.code), collection: true, ..Default::default() }),
                    (None, Some(b)) => Some(Built { code: format!("if (!({})) {}", test, b.code), collection: true, ..Default::default() }),
                    (None, None) => None,
                }
            }
            Expr::Call(callee, args, _) => {
                if let (Expr::Member(obj, m, _), Some(Expr::Arrow(f))) = (&**callee, args.first()) {
                    if m == "map" && f.params.len() <= 2 {
                        return Some(self.map_children(obj, f, inh, parent));
                    }
                }
                Some(Built { code: self.dynamic_child(e, inh), ..Default::default() })
            }
            Expr::Ident(n) if self.scope.nullable_widgets.contains(n) || n == "children" && !self.scope.widgets.contains(n) => {
                let c = self.expr(e, None);
                Some(Built { code: format!("({} ?? const SizedBox.shrink())", c), ..Default::default() })
            }
            Expr::Ident(n) if self.scope.widgets.contains(n) || n == "child" => Some(Built { code: self.expr(e, None), ..Default::default() }),
            Expr::Ident(n) if self.scope.renames.get(n).map_or(false, |r| r.ends_with(".children")) => Some(Built { code: self.expr(e, None), ..Default::default() }),
            Expr::Member(obj, name, _) if name == "children" && matches!(&**obj, Expr::Member(i, p, _) if p == "props" && matches!(**i, Expr::This)) => Some(Built { code: self.expr(e, None), ..Default::default() }),
            Expr::Spread(inner) => Some(Built { code: format!("...{}", self.expr(inner, None)), collection: true, ..Default::default() }),
            _ => Some(Built { code: self.dynamic_child(e, inh), ..Default::default() }),
        }
    }

    /// A value that is shown: text when it is a string or number, the widget itself when it is one.
    fn dynamic_child(&mut self, e: &Expr, inh: &Inherit) -> String {
        let style = text_style(&Style::default(), inh).map(|s| format!(", style: {}", s)).unwrap_or_default();
        let align = inh.align.as_ref().map(|a| format!(", textAlign: {}", text_align(a))).unwrap_or_default();
        let textish = matches!(e, Expr::Str(_) | Expr::Template(..) | Expr::Num(_)) || self.expr_type(e).map_or(false, |t| matches!(strip_null(&t).0, Ty::Name(ref n, _) if n == "string" || n == "number"));
        let code = self.expr(e, None);
        if textish {
            format!("Text({}{}{})", if matches!(e, Expr::Str(_) | Expr::Template(..)) { code } else { format!("'${{{}}}'", code) }, align, style)
        } else {
            format!("jsChild({}{}{})", code, if style.is_empty() { String::new() } else { format!(", textStyle: {}", &style[8..]) }, if align.is_empty() { String::new() } else { format!(", textAlign: {}", &align[12..]) })
        }
    }

    fn map_children(&mut self, source: &Expr, f: &Func, inh: &Inherit, parent: Parent) -> Built {
        let list = self.expr(source, None);
        self.push_scope();
        let item = match f.params.first().map(|p| &p.pat) {
            Some(Pat::Ident(n)) => safe_ident(n),
            _ => "_item".to_string(),
        };
        if let (Some(Pat::Ident(n)), Some(t)) = (f.params.first().map(|p| &p.pat), self.expr_type(source)) {
            if let Some(inner) = elem_type(&t) {
                self.declare(n, Some(inner));
            }
        }
        let index = f.params.get(1).and_then(|p| match &p.pat {
            Pat::Ident(n) => Some(safe_ident(n)),
            _ => None,
        });
        let prelude = match f.params.first() {
            Some(p) if !matches!(p.pat, Pat::Ident(_)) => self.destructure(&p.pat, &item, None, 3, "final"),
            _ => String::new(),
        };
        let head = match &index {
            Some(i) => format!("for (final ({}, {}) in {}.indexed)", i, item, list),
            None => format!("for (final {} in {})", item, list),
        };
        let out = match &f.body {
            FnBody::Expr(e) if prelude.is_empty() => match self.child_expr(e, inh, parent) {
                Some(b) if b.collection => format!("{} {}", head, b.code),
                Some(b) => format!("{} {}", head, b.code),
                None => String::new(),
            },
            FnBody::Expr(e) => {
                let w = self.child_expr(e, inh, parent).map(|b| b.code).unwrap_or_else(|| "const SizedBox.shrink()".to_string());
                let w = if w.starts_with("if ") || w.starts_with("for ") || w.starts_with("...") { "jsChild(null)".to_string() } else { w };
                match &index {
                    Some(i) => format!("...{}.asMap().entries.map((_e) {{\n final {} = _e.key; final {} = _e.value;\n{} return {};\n }}).toList()", list, i, item, prelude, w),
                    None => format!("...{}.map(({}) {{\n{} return {};\n }}).toList()", list, item, prelude, w),
                }
            }
            FnBody::Block(stmts) => {
                // a block body: statements, then `return <jsx>`: a function that builds the widget
                let (before, ret): (Vec<&Stmt>, Option<&Expr>) = {
                    let mut ret = None;
                    let mut before = Vec::new();
                    for s in stmts {
                        match s {
                            Stmt::Return(Some(e)) => ret = Some(e),
                            other => before.push(other),
                        }
                    }
                    (before, ret)
                };
                let mut body = String::new();
                for s in before {
                    body.push_str(&self.stmt(s, 4, None));
                }
                let w = ret.and_then(|e| self.child_expr(e, inh, parent)).map(|b| b.code).unwrap_or_else(|| "const SizedBox.shrink()".to_string());
                let w = if w.starts_with("if ") || w.starts_with("for ") || w.starts_with("...") { "jsChild(null)".to_string() } else { w };
                match &index {
                    Some(i) => format!("...{}.asMap().entries.map((_e) {{\n final {} = _e.key; final {} = _e.value;\n{}{}      return {};\n    }}).toList()", list, i, item, prelude, body, w),
                    None => format!("...{}.map(({}) {{\n{}{}      return {};\n    }}).toList()", list, item, prelude, body, w),
                }
            }
        };
        self.pop_scope();
        Built { code: out, collection: true, ..Default::default() }
    }

    fn stack_of(&mut self, kids: Vec<Built>, kind: &str) -> String {
        match kids.len() {
            0 => "const SizedBox.shrink()".to_string(),
            1 if !kids[0].collection => kids.into_iter().next().unwrap().code,
            _ => format!("{}(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: <Widget>[{}])", if kind == "Column" { "FlexColumn" } else { kind }, kids.into_iter().map(|k| k.code).collect::<Vec<_>>().join(", ")),
        }
    }

    /// Wraps a built child for its parent: `Expanded`, `Positioned`, an `Align` for a fixed-width child of a stretching column.
    fn place(&self, b: Built, parent: Parent, center: bool) -> String {
        if b.collection {
            return b.code;
        }
        if parent == Parent::Stack {
            if let Some(a) = &b.abs {
                let mut parts = Vec::new();
                for (i, name) in ["top", "right", "bottom", "left"].iter().enumerate() {
                    if let Some(Size::Px(p)) = a.inset[i] {
                        parts.push(format!("{}: {}", name, px(p)));
                    }
                }
                let both_x = a.inset[1].is_some() && a.inset[3].is_some();
                let _ = both_x;
                // top-1/2 -translate-y-1/2: centred on the line
                let frac = |s: &Option<Size>| matches!(s, Some(Size::Frac(f)) if *f == 0.5);
                if frac(&a.inset[0]) && a.ty.is_some() {
                    return format!("Positioned.fill(child: Align(alignment: {}, child: {}))", alignment_of(if a.inset[3].is_some() { "-1" } else if a.inset[1].is_some() { "1" } else { "0" }, "0"), wrap_padding(&b.code, &a.inset));
                }
                if frac(&a.inset[3]) && a.tx.is_some() {
                    return format!("Positioned.fill(child: Align(alignment: {}, child: {}))", alignment_of("0", if a.inset[0].is_some() { "-1" } else { "1" }), b.code);
                }
                if a.inset.iter().all(|s| matches!(s, Some(Size::Px(_)))) {
                    return format!("Positioned.fill(child: {})", b.code);
                }
                return format!("Positioned({}, child: {})", parts.join(", "), b.code);
            }
            return b.code;
        }
        let mut code = b.code;
        // a flex item shrinks when the row is too narrow (CSS flex-shrink): text wraps instead of overflowing
        if parent == Parent::Row && !b.expanded && !b.fixed_w && !icon_like(&code) {
            code = format!("Flexible(child: {})", code);
        }
        if b.fixed_w && parent == Parent::Column {
            code = format!("Align(alignment: {}, child: {})", if center { "Alignment.topCenter" } else { "Alignment.topLeft" }, code);
        }
        if b.expanded && matches!(parent, Parent::Row | Parent::Column) {
            code = format!("Expanded(child: {})", code);
        }
        code
    }

    // ---------------------------------------------------------------- elements

    pub fn element(&mut self, j: &Jsx, inh: &Inherit, parent: Parent) -> Built {
        // `<div {...props}>`: the props the `...rest` stands for become attributes
        let expanded;
        let j = match &self.scope.rest {
            Some((name, fields)) if j.attrs.iter().any(|a| matches!(a, Attr::Spread(Expr::Ident(n)) if n == name)) && !j.tag.chars().next().map_or(false, |c| c.is_uppercase()) => {
                let mut attrs: Vec<Attr> = j.attrs.iter().filter(|a| !matches!(a, Attr::Spread(Expr::Ident(n)) if n == name)).cloned().collect();
                for f in fields {
                    if f != "children" && f != "className" && !attrs.iter().any(|a| matches!(a, Attr::Kv(n, _) if n == f)) {
                        attrs.push(Attr::Kv(f.clone(), AttrVal::Expr(Expr::Ident(f.clone()))));
                    }
                }
                expanded = Jsx { tag: j.tag.clone(), attrs, children: j.children.clone() };
                &expanded
            }
            _ => j,
        };
        let tag = j.tag.as_str();
        let upper = tag.chars().next().map_or(false, |c| c.is_uppercase());
        if upper || tag.contains('.') {
            return self.component_element(j, inh, parent);
        }
        let (mut st, dyn_class) = self.class_of(j);
        default_style_for(tag, &mut st);
        for u in st.unknown.clone() {
            self.note(format!("Tailwind class `{}` has no Flutter mapping (ignored)", u));
        }
        if st.hidden {
            return Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() };
        }
        let mut b = match tag {
            "input" => self.input_element(j, &st, inh),
            "textarea" => self.input_element(j, &st, inh),
            "select" => self.select_element(j, &st, inh),
            "button" => self.button_element(j, &st, dyn_class.clone(), inh),
            "img" => self.image_element(j, &st),
            "a" => self.link_element(j, &st, inh),
            "svg" | "path" | "circle" | "line" | "rect" | "polyline" | "polygon" | "g" => Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() },
            "br" => Built { code: "const SizedBox(height: 8)".to_string(), ..Default::default() },
            "hr" => Built { code: "const Divider(height: 1)".to_string(), ..Default::default() },
            "canvas" | "video" | "audio" | "iframe" => {
                self.note(format!("<{}> has no widget here: a placeholder box is written", tag));
                Built { code: "Container(color: const Color(0x1F000000), height: 200)".to_string(), ..Default::default() }
            }
            "option" => Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() },
            t if is_text_tag(t) => self.text_element(j, &st, dyn_class.clone(), inh),
            "li" | "td" | "th" | "dd" | "dt" => self.box_element(j, &st, dyn_class.clone(), inh, parent),
            "tr" => self.table_row(j, &st, inh),
            "form" => self.form_element(j, &st, dyn_class.clone(), inh, parent),
            _ => self.box_element(j, &st, dyn_class.clone(), inh, parent),
        };
        // a click on a plain element
        if !matches!(tag, "button" | "a" | "input" | "select" | "textarea" | "form") {
            if let Some(AttrVal::Expr(e)) = attr(j, "onClick") {
                let h = self.handler_call(e);
                b.code = format!("GestureDetector(onTap: {}, behavior: HitTestBehavior.opaque, child: {})", h, b.code);
            }
        }
        // placement facts the parent needs
        if st.position.as_deref() == Some("absolute") || st.position.as_deref() == Some("fixed") {
            b.abs = Some(Abs { inset: st.inset, tx: st.translate_x_half, ty: st.translate_y_half });
        }
        if st.flex_1 || st.grow || (st.w == Some(Size::Full) && parent == Parent::Row) {
            b.expanded = true;
        }
        if matches!(st.w, Some(Size::Px(_))) && tag != "input" {
            b.fixed_w = true;
        }
        if st.mx_auto {
            b.self_center = true;
        }
        b
    }

    /// Everything a container style adds around `content`: sizes, padding, margin, decoration, clipping, opacity.
    fn decorate(&mut self, st: &Style, content: String, opts: DecorOpts) -> String {
        let mut code = content;
        // SizedBox for fixed sizes without other decoration
        let has_deco = st.bg.is_some() || st.gradient.is_some() || st.border.iter().any(|b| b.map_or(false, |w| w > 0.0)) || st.radius.iter().any(|r| r.is_some()) || st.shadow.is_some() || st.ring.is_some();
        let pad = edge_insets(&st.pad);
        let mar = edge_insets(&st.mar);
        let mut args: Vec<String> = Vec::new();
        let width = st.w.as_ref().filter(|w| !matches!(w, Size::Auto | Size::Frac(_)) && !(opts.in_row && matches!(w, Size::Full)));
        let height = st.h.as_ref().filter(|h| !matches!(h, Size::Auto | Size::Frac(_)));
        if let Some(w) = width {
            args.push(format!("width: {}", size_code(w, true)));
        }
        if let Some(h) = height {
            args.push(format!("height: {}", size_code(h, false)));
        }
        let mut constraints = Vec::new();
        if let Some(s) = &st.max_w {
            if !matches!(s, Size::Auto) {
                constraints.push(format!("maxWidth: {}", size_code(s, true)));
            }
        }
        if let Some(s) = &st.min_w {
            if !matches!(s, Size::Auto) {
                constraints.push(format!("minWidth: {}", size_code(s, true)));
            }
        }
        if let Some(s) = &st.max_h {
            if !matches!(s, Size::Auto) {
                constraints.push(format!("maxHeight: {}", size_code(s, false)));
            }
        }
        if let Some(s) = &st.min_h {
            if !matches!(s, Size::Auto) {
                constraints.push(format!("minHeight: {}", if matches!(s, Size::Full) { "0".to_string() } else { size_code(s, false) }));
            }
        }
        // mx-auto with a max width: centred, as wide as the parent allows up to the maximum
        let centered_max = st.mx_auto && st.max_w.is_some();
        if centered_max {
            constraints.retain(|c| !c.starts_with("maxWidth"));
        }
        if !constraints.is_empty() {
            args.push(format!("constraints: BoxConstraints({})", constraints.join(", ")));
        }
        if let Some(p) = &pad {
            args.push(format!("padding: {}", p));
        }
        if let Some(a) = &opts.alignment {
            args.push(format!("alignment: {}", a));
        }
        if has_deco {
            let mut d = Vec::new();
            if let Some(g) = &st.gradient {
                let (begin, end) = match g.dir.as_str() {
                    "r" => ("Alignment.centerLeft", "Alignment.centerRight"),
                    "l" => ("Alignment.centerRight", "Alignment.centerLeft"),
                    "t" => ("Alignment.bottomCenter", "Alignment.topCenter"),
                    "b" => ("Alignment.topCenter", "Alignment.bottomCenter"),
                    "tr" => ("Alignment.bottomLeft", "Alignment.topRight"),
                    "tl" => ("Alignment.bottomRight", "Alignment.topLeft"),
                    "bl" => ("Alignment.topRight", "Alignment.bottomLeft"),
                    _ => ("Alignment.topLeft", "Alignment.bottomRight"),
                };
                let mut colors = Vec::new();
                for c in [&g.from, &g.via, &g.to].into_iter().flatten() {
                    colors.push(c.dart());
                }
                if colors.len() >= 2 {
                    d.push(format!("gradient: LinearGradient(begin: {}, end: {}, colors: [{}])", begin, end, colors.join(", ")));
                } else if let Some(c) = colors.first() {
                    d.push(format!("color: {}", c));
                }
            } else if let Some(bg) = &st.bg {
                d.push(format!("color: {}", bg.dart()));
            }
            let full_round = st.radius.iter().all(|r| *r == Some(9999.0)) && matches!((&st.w, &st.h), (Some(Size::Px(a)), Some(Size::Px(b))) if a == b);
            if full_round {
                d.push("shape: BoxShape.circle".to_string());
            } else if let Some(r) = border_radius(&st.radius) {
                d.push(format!("borderRadius: {}", r));
            }
            let bw: Vec<f64> = st.border.iter().map(|b| b.unwrap_or(0.0)).collect();
            if bw.iter().any(|w| *w > 0.0) {
                let col = st.border_color.map(|c| c.dart()).unwrap_or_else(|| "const Color(0xFFE5E7EB)".to_string());
                if bw.iter().all(|w| *w == bw[0]) {
                    d.push(format!("border: Border.all(color: {}, width: {})", col, px(bw[0])));
                } else {
                    let side = |w: f64| if w > 0.0 { format!("BorderSide(color: {}, width: {})", col, px(w)) } else { "BorderSide.none".to_string() };
                    d.push(format!("border: Border(top: {}, right: {}, bottom: {}, left: {})", side(bw[0]), side(bw[1]), side(bw[2]), side(bw[3])));
                }
            } else if let Some((w, c)) = &st.ring {
                if st.shadow.is_none() {
                    d.push(format!("border: Border.all(color: {}, width: {})", c.map(|c| c.dart()).unwrap_or_else(|| "const Color(0x803B82F6)".to_string()), px(*w)));
                }
            }
            if let Some(s) = &st.shadow {
                d.push(format!("boxShadow: {}", shadow(s)));
            }
            args.push(format!("decoration: BoxDecoration({})", d.join(", ")));
            if st.overflow_hidden || st.radius.iter().any(|r| r.is_some()) {
                args.push("clipBehavior: Clip.antiAlias".to_string());
            }
        }
        if let Some(m) = &mar {
            args.push(format!("margin: {}", m));
        }
        if !args.is_empty() {
            code = format!("Container({}, child: {})", args.join(", "), code);
        }
        if centered_max {
            let max = size_code(st.max_w.as_ref().unwrap(), true);
            code = format!("Align(alignment: Alignment.topCenter, child: ConstrainedBox(constraints: BoxConstraints(maxWidth: {}), child: SizedBox(width: double.infinity, child: {})))", max, code);
        } else if st.mx_auto && matches!(st.w, Some(Size::Px(_))) {
            code = format!("Center(child: {})", code);
        }
        if st.scroll_y && !opts.no_scroll {
            code = format!("SingleChildScrollView(child: {})", code);
        }
        if st.scroll_x {
            code = format!("SingleChildScrollView(scrollDirection: Axis.horizontal, child: {})", code);
        }
        if let Some(o) = st.opacity {
            code = format!("Opacity(opacity: {}, child: {})", o, code);
        }
        code
    }

    /// `div` and the other boxes: flex, grid, or a plain block.
    fn box_element(&mut self, j: &Jsx, st: &Style, dyn_class: Option<String>, inh: &Inherit, parent: Parent) -> Built {
        let inh2 = merge_inherit(st, inh);
        if let Some(classes) = dyn_class {
            let kids = self.children(&j.children, &inh2, Parent::Column);
            let kids: Vec<String> = kids.into_iter().map(|k| k.code).collect();
            return Built { code: format!("Tw.view({}, children: <Widget>[{}])", classes, kids.join(", ")), ..Default::default() };
        }
        // a spinner: a bordered round box that rotates
        if st.spin && st.radius.iter().all(|r| *r == Some(9999.0)) {
            let w = match st.w {
                Some(Size::Px(p)) => p,
                _ => 16.0,
            };
            let h = match st.h {
                Some(Size::Px(p)) => p,
                _ => w,
            };
            let col = st.border_color.map(|c| c.dart()).unwrap_or_else(|| "Colors.blue".to_string());
            let sw = st.border.iter().flatten().cloned().fold(2.0, f64::max);
            let mar = edge_insets(&st.mar);
            let sp = format!("SizedBox(width: {}, height: {}, child: CircularProgressIndicator(strokeWidth: {}, color: {}))", px(w), px(h), px(sw), col);
            return Built { code: match mar { Some(m) => format!("Padding(padding: {}, child: {})", m, sp), None => sp }, fixed_w: true, ..Default::default() };
        }
        let is_grid = st.grid;
        let is_row = st.flex && !st.col;
        let child_parent = if is_grid { Parent::Column } else if is_row { Parent::Row } else { Parent::Column };
        let kid_inh = &inh2;
        let kids = self.children(&j.children, kid_inh, child_parent);
        let has_abs = kids.iter().any(|k| k.abs.is_some()) || st.position.as_deref() == Some("relative") && kids.iter().any(|k| k.abs.is_some());
        let center_children = st.items.as_deref() == Some("center") || st.align.as_deref() == Some("center");

        let mut opts = DecorOpts { in_row: parent == Parent::Row, ..Default::default() };
        let content: String;
        if has_abs {
            // Stack: the normal-flow children first (as one column), then the positioned ones
            let (abs, flow): (Vec<Built>, Vec<Built>) = kids.into_iter().partition(|k| k.abs.is_some());
            let flow_code: Vec<String> = flow.into_iter().map(|k| self.place(k, Parent::Column, center_children)).collect();
            let mut layers: Vec<String> = Vec::new();
            if !flow_code.is_empty() {
                layers.push(if flow_code.len() == 1 && !flow_code[0].starts_with("if ") && !flow_code[0].starts_with("for ") { flow_code[0].clone() } else { format!("FlexColumn(crossAxisAlignment: CrossAxisAlignment.stretch, mainAxisSize: MainAxisSize.min, children: <Widget>[{}])", flow_code.join(", ")) });
            }
            for a in abs {
                layers.push(self.place(a, Parent::Stack, false));
            }
            let fit = if st.position.as_deref() == Some("fixed") || st.inset.iter().all(|i| i.is_some()) { "StackFit.expand" } else { "StackFit.loose" };
            content = format!("Stack(fit: {}, clipBehavior: Clip.none, children: <Widget>[{}])", fit, layers.join(", "));
        } else if is_grid && st.grid_cols.unwrap_or(1) > 1 {
            let cols = st.grid_cols.unwrap_or(2);
            let gap = st.gap_x.or(st.gap_y).unwrap_or(0.0);
            let items: Vec<String> = kids.into_iter().map(|k| k.code).collect();
            content = format!("GridView.count(crossAxisCount: {}, shrinkWrap: true, physics: const NeverScrollableScrollPhysics(), mainAxisSpacing: {}, crossAxisSpacing: {}, children: <Widget>[{}])", cols, px(gap), px(gap), items.join(", "));
        } else if kids.len() == 1 && !kids[0].collection && !st.wrap && st.divide_y.is_none() && st.divide_x.is_none() && (st.flex || is_grid || true) && (!st.flex || st.justify.is_some() || st.items.is_some()) && !(st.flex && st.min_h.is_none() && st.h.is_none() && st.justify.as_deref() == Some("between")) {
            // one child: alignment instead of a Row/Column
            let k = kids.into_iter().next().unwrap();
            let (jx, ix) = (st.justify.as_deref().unwrap_or("start"), st.items.as_deref().unwrap_or("start"));
            if st.flex {
                let ax = |v: &str| match v {
                    "center" => "0",
                    "end" => "1",
                    _ => "-1",
                };
                let (x, y) = if is_row { (ax(jx), ax(ix)) } else { (ax(ix), ax(jx)) };
                let vertical_matters = st.min_h.is_some() || st.h.is_some() || y != "-1";
                opts.alignment = Some(alignment_of(x, if vertical_matters { y } else { "-1" }));
                let code = self.place(k, Parent::Block, false);
                content = if x == "-1" && y == "-1" { code } else { code };
            } else {
                content = self.place(k, Parent::Block, center_children);
            }
        } else {
            let gap = if is_row { st.gap_x.or(st.space_x) } else { st.gap_y.or(st.space_y) };
            let placed: Vec<String> = kids.into_iter().map(|k| self.place(k, child_parent, center_children)).collect();
            let mut items = placed;
            if let Some(color) = st.divide_y.or(st.divide_x) {
                let mut with_div = Vec::new();
                let n = items.len();
                for (i, it) in items.into_iter().enumerate() {
                    with_div.push(it);
                    if i + 1 < n {
                        with_div.push(format!("Divider(height: 1, thickness: 1, color: {})", color.dart()));
                    }
                }
                items = with_div;
            }
            let spacing = gap.filter(|g| *g > 0.0).map(|g| format!("spacing: {}, ", px(g))).unwrap_or_default();
            let bounded = st.h.is_some() || st.min_h.is_some();
            let main_size = if bounded && st.flex && (st.justify.is_some()) { "MainAxisSize.max" } else { "MainAxisSize.min" };
            if st.wrap && st.flex {
                let wrap_gap_x = st.gap_x.unwrap_or(0.0);
                let wrap_gap_y = st.gap_y.unwrap_or(0.0);
                content = format!("Wrap(spacing: {}, runSpacing: {}, crossAxisAlignment: WrapCrossAlignment.center, children: <Widget>[{}])", px(wrap_gap_x), px(wrap_gap_y), items.join(", "));
            } else if is_row {
                let main = main_axis(st.justify.as_deref().unwrap_or("start"));
                let cross = cross_axis(st.items.as_deref().unwrap_or("start"));
                // a Row shrink-wraps unless it should spread
                let size = if matches!(st.justify.as_deref(), Some("between" | "around" | "evenly" | "center" | "end")) || st.w == Some(Size::Full) { "MainAxisSize.max" } else { "MainAxisSize.min" };
                content = format!("Row({}mainAxisAlignment: {}, crossAxisAlignment: {}, mainAxisSize: {}, children: <Widget>[{}])", spacing, main, cross, size, items.join(", "));
                let _ = main_size;
            } else {
                let cross = if st.flex { cross_axis(st.items.as_deref().unwrap_or("stretch")) } else if center_children { "CrossAxisAlignment.center" } else { "CrossAxisAlignment.stretch" };
                let main = main_axis(st.justify.as_deref().unwrap_or("start"));
                // a Column that stretches would get infinite width inside a Row: FlexColumn stretches only when it can
                let col_widget = if cross == "CrossAxisAlignment.stretch" { "FlexColumn" } else { "Column" };
                content = format!("{}({}mainAxisAlignment: {}, crossAxisAlignment: {}, mainAxisSize: {}, children: <Widget>[{}])", col_widget, spacing, main, cross, main_size, items.join(", "));
            }
        }
        let mut code = self.decorate(st, content, opts);
        // min-h-screen: at least the screen's height
        if matches!(st.min_h, Some(Size::Screen)) && !code.contains("minHeight") {
            code = format!("ConstrainedBox(constraints: BoxConstraints(minHeight: MediaQuery.of(context).size.height), child: {})", code);
        }
        // text-center on a container centres the text inside
        Built { code, ..Default::default() }
    }

    fn form_element(&mut self, j: &Jsx, st: &Style, dyn_class: Option<String>, inh: &Inherit, parent: Parent) -> Built {
        let handler = match attr(j, "onSubmit") {
            Some(AttrVal::Expr(e)) => Some(self.handler_call(e)),
            _ => None,
        };
        self.forms.push(handler);
        let inner = self.box_element(j, st, dyn_class, inh, parent);
        self.forms.pop();
        Built { code: format!("Form(child: {})", inner.code), ..inner }
    }

    /// An event handler as a no-argument closure: `() => handleX(null)` / `() => navigate(context, '/x')`.
    pub fn handler_call(&mut self, e: &Expr) -> String {
        match e {
            Expr::Arrow(f) => {
                let names: Vec<String> = f.params.iter().filter_map(|p| match &p.pat {
                    Pat::Ident(n) => Some(n.clone()),
                    _ => None,
                }).collect();
                let before = self.scope.events.len();
                self.scope.events.extend(names.iter().cloned());
                let code = self.func(f, None);
                self.scope.events.truncate(before);
                if f.params.is_empty() {
                    code
                } else {
                    let nulls = vec!["null"; f.params.len()].join(", ");
                    format!("() {{ ({})({}); }}", code, nulls)
                }
            }
            Expr::Ident(n) => {
                let arity = self.scope.methods.get(n).copied().unwrap_or(0);
                let code = self.expr(e, None);
                if arity > 0 { format!("() {{ {}({}); }}", code, vec!["null"; arity].join(", ")) } else { code }
            }
            other => {
                let c = self.expr(other, None);
                format!("() {{ ({})(); }}", c)
            }
        }
    }

    /// A handler that receives the new value (`onChange`).
    fn change_handler(&mut self, e: &Expr) -> String {
        match e {
            Expr::Arrow(f) => {
                let names: Vec<String> = f.params.iter().filter_map(|p| match &p.pat {
                    Pat::Ident(n) => Some(n.clone()),
                    _ => None,
                }).collect();
                let before = self.scope.events.len();
                self.scope.events.extend(names);
                let code = self.func(f, None);
                self.scope.events.truncate(before);
                code
            }
            other => self.expr(other, None),
        }
    }

    fn text_element(&mut self, j: &Jsx, st: &Style, dyn_class: Option<String>, inh: &Inherit) -> Built {
        let inh2 = merge_inherit(st, inh);
        // a paragraph made of text and inline elements -> Text.rich; only text and expressions -> one Text
        let mut flat = Vec::new();
        self.flatten(&j.children, &mut flat);
        let all_text = flat.iter().all(|k| match k {
            JsxChild::Text(_) => true,
            JsxChild::Expr(e) => !matches!(e, Expr::Jsx(_) | Expr::Cond(..)) && !matches!(e, Expr::Binary(op, _, _) if op == "&&"),
            JsxChild::Element(_) => false,
        });
        let inline_only = flat.iter().all(|k| match k {
            JsxChild::Element(e) => is_inline_text_tag(&e.tag) && e.children.iter().all(|c| matches!(c, JsxChild::Text(_) | JsxChild::Expr(_))),
            _ => true,
        });
        if let Some(classes) = dyn_class {
            let content = self.text_template(&flat);
            return Built { code: format!("Tw.text({}, {})", classes, content), ..Default::default() };
        }
        let style = text_style(st, inh).map(|s| format!(", style: {}", s)).unwrap_or_default();
        let align = st.align.as_ref().or(inh.align.as_ref()).map(|a| format!(", textAlign: {}", text_align(a))).unwrap_or_default();
        let mut extra = String::new();
        if st.truncate {
            extra.push_str(", maxLines: 1, overflow: TextOverflow.ellipsis");
        } else if let Some(n) = st.line_clamp {
            extra.push_str(&format!(", maxLines: {}, overflow: TextOverflow.ellipsis", n));
        }
        let code = if all_text {
            let mut content = self.text_template(&flat);
            if st.uppercase {
                content = format!("({}).toUpperCase()", content);
            } else if st.lowercase {
                content = format!("({}).toLowerCase()", content);
            } else if st.capitalize {
                content = format!("jsCapitalize({})", content);
            }
            format!("Text({}{}{}{})", content, align, style, extra)
        } else if inline_only {
            let mut spans = Vec::new();
            for k in &flat {
                match k {
                    JsxChild::Text(t) => spans.push(format!("TextSpan(text: {})", str_lit(t))),
                    JsxChild::Expr(e) => spans.push(format!("TextSpan(text: {})", self.text_part(e))),
                    JsxChild::Element(e) => {
                        let (mut es, _) = self.class_of(e);
                        default_style_for(&e.tag, &mut es);
                        let mut inner = Vec::new();
                        let mut f2 = Vec::new();
                        self.flatten(&e.children, &mut f2);
                        for c in &f2 {
                            match c {
                                JsxChild::Text(t) => inner.push(str_lit(t)),
                                JsxChild::Expr(x) => inner.push(self.text_part(x)),
                                _ => {}
                            }
                        }
                        let st2 = text_style(&es, &Inherit::default()).map(|s| format!(", style: {}", s)).unwrap_or_default();
                        spans.push(format!("TextSpan(text: {}{})", inner.join(" + "), st2));
                    }
                }
            }
            format!("Text.rich(TextSpan(children: [{}]){}{}{})", spans.join(", "), align, style, extra)
        } else {
            // block children inside a text tag: lay them out as a box
            let kids = self.children(&j.children, &inh2, Parent::Column);
            let items: Vec<String> = kids.into_iter().map(|k| k.code).collect();
            format!("Column(crossAxisAlignment: CrossAxisAlignment.start, mainAxisSize: MainAxisSize.min, children: <Widget>[{}])", items.join(", "))
        };
        let mut o = DecorOpts::default();
        o.no_scroll = true;
        let mut box_st = st.clone();
        box_st.color = None;
        box_st.size = None;
        let code = self.decorate(&box_st, code, o);
        Built { code, ..Default::default() }
    }

    /// The text children joined into one Dart string expression.
    fn text_template(&mut self, flat: &[&JsxChild]) -> String {
        let mut out = String::from("'");
        let mut all_plain = true;
        for k in flat {
            match k {
                JsxChild::Text(t) => out.push_str(&str_lit(t)[1..str_lit(t).len() - 1]),
                JsxChild::Expr(e) => {
                    all_plain = false;
                    match e {
                        Expr::Str(s) => out.push_str(&str_lit(s)[1..str_lit(s).len() - 1]),
                        other => {
                            let c = self.expr(other, None);
                            out.push_str(&format!("${{{}}}", c));
                        }
                    }
                }
                _ => {}
            }
        }
        out.push('\'');
        let _ = all_plain;
        out
    }

    fn text_part(&mut self, e: &Expr) -> String {
        match e {
            Expr::Str(s) => str_lit(s),
            other => format!("'${{{}}}'", self.expr(other, None)),
        }
    }

    fn button_element(&mut self, j: &Jsx, st: &Style, dyn_class: Option<String>, inh: &Inherit) -> Built {
        let inh2 = merge_inherit(st, inh);
        let ty = attr_str(j, "type").unwrap_or_default();
        let on_tap = match attr(j, "onClick") {
            Some(AttrVal::Expr(e)) => Some(self.handler_call(e)),
            _ if ty == "submit" => self.forms.last().cloned().flatten(),
            _ => None,
        };
        let disabled = match attr(j, "disabled") {
            Some(AttrVal::Expr(e)) => Some(self.cond(e)),
            Some(AttrVal::True) => Some("true".to_string()),
            _ => None,
        };
        let kids = self.children(&j.children, &inh2, Parent::Row);
        let items: Vec<String> = kids.into_iter().map(|k| if k.collection || k.abs.is_some() || icon_like(&k.code) { k.code } else { format!("Flexible(child: {})", k.code) }).collect();
        let tap = match (&on_tap, &disabled) {
            (Some(t), Some(d)) => format!("({}) ? null : {}", d, t),
            (Some(t), None) => t.clone(),
            (None, _) => "null".to_string(),
        };
        if let Some(classes) = dyn_class {
            return Built { code: format!("Tw.button({}, onTap: {}, children: <Widget>[{}])", classes, tap, items.join(", ")), ..Default::default() };
        }
        // layout inside the button
        let spacing = st.gap_x.filter(|g| *g > 0.0).map(|g| format!("spacing: {}, ", px(g))).unwrap_or_default();
        let main = main_axis(st.justify.as_deref().unwrap_or(if st.flex { "start" } else { "center" }));
        let row = format!("Row({}mainAxisSize: MainAxisSize.min, mainAxisAlignment: {}, crossAxisAlignment: CrossAxisAlignment.center, children: <Widget>[{}])", spacing, main, items.join(", "));
        let radius = border_radius(&st.radius).unwrap_or_else(|| "BorderRadius.zero".to_string());
        let side = match (st.border[0], st.border_color) {
            (Some(w), c) if w > 0.0 => format!(", side: BorderSide(color: {}, width: {})", c.map(|c| c.dart()).unwrap_or_else(|| "const Color(0xFFE5E7EB)".to_string()), px(w)),
            _ => String::new(),
        };
        let color = st.bg.map(|c| c.dart()).unwrap_or_else(|| "Colors.transparent".to_string());
        let padding = edge_insets(&st.pad).unwrap_or_else(|| "EdgeInsets.zero".to_string());
        let ts = text_style(st, inh).unwrap_or_else(|| "TextStyle()".to_string());
        let icon_color = (st.color.or(inh.color)).map(|c| format!("color: {}", c.dart())).unwrap_or_default();
        let inner = format!("DefaultTextStyle.merge(style: {}, child: IconTheme.merge(data: IconThemeData({}), child: {}))", ts, icon_color, row);
        let mut code = format!(
            "Material(color: {}, shape: RoundedRectangleBorder(borderRadius: {}{}), clipBehavior: Clip.antiAlias, child: InkWell(onTap: {}, child: Padding(padding: {}, child: {})))",
            color, radius, side, tap, padding, inner
        );
        if let (Some(op), Some(d)) = (st.opacity_disabled, &disabled) {
            code = format!("Opacity(opacity: ({}) ? {} : 1.0, child: {})", d, op, code);
        } else if let Some(op) = st.opacity_disabled {
            let _ = op;
        }
        if let Some(m) = edge_insets(&st.mar) {
            code = format!("Padding(padding: {}, child: {})", m, code);
        }
        if st.w == Some(Size::Full) {
            code = format!("SizedBox(width: double.infinity, child: {})", code);
        } else if let Some(Size::Px(w)) = st.w {
            code = format!("SizedBox(width: {}, child: {})", px(w), code);
        }
        let code = match st.h {
            Some(Size::Px(h)) => format!("SizedBox(height: {}, child: {})", px(h), code),
            _ => code,
        };
        Built { code, ..Default::default() }
    }

    fn link_element(&mut self, j: &Jsx, st: &Style, inh: &Inherit) -> Built {
        let inh2 = merge_inherit(st, inh);
        let kids = self.children(&j.children, &inh2, Parent::Row);
        let items: Vec<String> = kids.into_iter().map(|k| k.code).collect();
        let content = if items.len() == 1 { items[0].clone() } else { format!("Row(mainAxisSize: MainAxisSize.min, children: <Widget>[{}])", items.join(", ")) };
        let target = attr_str(j, "href");
        let tap = match (attr(j, "onClick"), target) {
            (Some(AttrVal::Expr(e)), _) => self.handler_call(e),
            (_, Some(t)) if t.starts_with('/') => format!("() => navigate(context, {})", str_lit(&t)),
            (_, Some(t)) => format!("() => openUrl({})", str_lit(&t)),
            _ => "null".to_string(),
        };
        Built { code: format!("InkWell(onTap: {}, child: {})", tap, content), ..Default::default() }
    }

    fn image_element(&mut self, j: &Jsx, st: &Style) -> Built {
        let src = match attr(j, "src") {
            Some(AttrVal::Str(s)) => str_lit(s),
            Some(AttrVal::Expr(e)) => self.expr(e, None),
            _ => "''".to_string(),
        };
        let w = match st.w {
            Some(Size::Px(p)) => format!(", width: {}", px(p)),
            _ => String::new(),
        };
        let h = match st.h {
            Some(Size::Px(p)) => format!(", height: {}", px(p)),
            _ => String::new(),
        };
        let fit = match st.object_fit.as_deref() {
            Some("cover") => ", fit: BoxFit.cover",
            Some("contain") => ", fit: BoxFit.contain",
            _ => "",
        };
        let img = format!("Image.network({}{}{}{}, errorBuilder: (context, error, stack) => const SizedBox.shrink())", src, w, h, fit);
        let code = self.decorate(&Style { w: None, h: None, ..st.clone() }, img, DecorOpts::default());
        Built { code, ..Default::default() }
    }

    fn table_row(&mut self, j: &Jsx, st: &Style, inh: &Inherit) -> Built {
        let kids = self.children(&j.children, inh, Parent::Row);
        let cells: Vec<String> = kids.into_iter().map(|k| if k.collection { k.code } else { format!("Expanded(child: {})", k.code) }).collect();
        let row = format!("Row(crossAxisAlignment: CrossAxisAlignment.center, children: <Widget>[{}])", cells.join(", "));
        let code = self.decorate(st, row, DecorOpts::default());
        Built { code, ..Default::default() }
    }

    // ---------------------------------------------------------------- inputs

    fn input_decoration(&mut self, j: &Jsx, st: &Style) -> String {
        let mut parts = Vec::new();
        match attr(j, "placeholder") {
            Some(AttrVal::Str(s)) => parts.push(format!("hintText: {}", str_lit(s))),
            Some(AttrVal::Expr(e)) => parts.push(format!("hintText: {}", self.expr(e, None))),
            _ => {}
        }
        if let Some(bg) = st.bg {
            parts.push("filled: true".to_string());
            parts.push(format!("fillColor: {}", bg.dart()));
        }
        let radius = border_radius(&st.radius).unwrap_or_else(|| "BorderRadius.circular(4)".to_string());
        let w = st.border.iter().flatten().cloned().fold(0.0, f64::max);
        let col = st.border_color.map(|c| c.dart()).unwrap_or_else(|| "const Color(0xFFD1D5DB)".to_string());
        let focus = st.ring.and_then(|r| r.1).map(|c| c.dart()).unwrap_or_else(|| "const Color(0xFF3B82F6)".to_string());
        if w > 0.0 {
            parts.push(format!("border: OutlineInputBorder(borderRadius: {}, borderSide: BorderSide(color: {}, width: {}))", radius, col, px(w)));
            parts.push(format!("enabledBorder: OutlineInputBorder(borderRadius: {}, borderSide: BorderSide(color: {}, width: {}))", radius, col, px(w)));
            parts.push(format!("focusedBorder: OutlineInputBorder(borderRadius: {}, borderSide: BorderSide(color: {}, width: 2))", radius, focus));
        } else {
            parts.push("border: InputBorder.none".to_string());
        }
        if let Some(p) = edge_insets(&st.pad) {
            parts.push(format!("contentPadding: {}", p));
        }
        parts.push("isDense: true".to_string());
        format!("InputDecoration({})", parts.join(", "))
    }

    fn input_element(&mut self, j: &Jsx, st: &Style, inh: &Inherit) -> Built {
        let ty = attr_str(j, "type").unwrap_or_else(|| "text".to_string());
        let value = match attr(j, "value") {
            Some(AttrVal::Expr(e)) => Some(self.expr(e, None)),
            Some(AttrVal::Str(s)) => Some(str_lit(s)),
            _ => None,
        };
        let onchange = match attr(j, "onChange") {
            Some(AttrVal::Expr(e)) => Some(self.change_handler(e)),
            _ => None,
        };
        let disabled = match attr(j, "disabled") {
            Some(AttrVal::Expr(e)) => Some(self.cond(e)),
            Some(AttrVal::True) => Some("true".to_string()),
            _ => None,
        };
        if ty == "checkbox" || ty == "radio" {
            let checked = match attr(j, "checked") {
                Some(AttrVal::Expr(e)) => self.cond(e),
                _ => "false".to_string(),
            };
            let on = onchange.unwrap_or_else(|| "null".to_string());
            return Built { code: format!("Checkbox(value: {}, onChanged: {})", checked, if on == "null" { on } else { format!("(v) => ({})(v)", on) }), ..Default::default() };
        }
        let mut args = Vec::new();
        if let Some(v) = value {
            args.push(format!("value: {}", v));
        }
        if let Some(c) = onchange {
            args.push(format!("onChanged: {}", c));
        }
        match ty.as_str() {
            "password" => args.push("obscureText: true".to_string()),
            "email" => args.push("keyboardType: TextInputType.emailAddress".to_string()),
            "tel" => args.push("keyboardType: TextInputType.phone".to_string()),
            "number" => args.push("keyboardType: TextInputType.number".to_string()),
            "url" => args.push("keyboardType: TextInputType.url".to_string()),
            _ => {}
        }
        if j.tag == "textarea" {
            let rows = match attr(j, "rows") {
                Some(AttrVal::Str(s)) => s.clone(),
                Some(AttrVal::Expr(Expr::Num(n))) => n.clone(),
                _ => "3".to_string(),
            };
            args.push(format!("maxLines: {}", rows));
        }
        if let Some(Ok(n)) = attr(j, "maxLength").map(|v| match v {
            AttrVal::Expr(Expr::Num(n)) => Ok(n.clone()),
            AttrVal::Str(s) => Ok(s.clone()),
            _ => Err(()),
        }) {
            args.push(format!("maxLength: {}", n));
        }
        if let Some(d) = disabled {
            args.push(format!("enabled: !({})", d));
        }
        if let Some(ts) = text_style(st, inh) {
            args.push(format!("style: {}", ts));
        }
        args.push(format!("decoration: {}", self.input_decoration(j, st)));
        let field = format!("ControlledField({})", args.join(", "));
        let code = self.decorate(&Style { pad: [None; 4], bg: None, border: [None; 4], radius: [None; 4], shadow: None, ring: None, ..st.clone() }, field, DecorOpts::default());
        Built { code, ..Default::default() }
    }

    fn select_element(&mut self, j: &Jsx, st: &Style, inh: &Inherit) -> Built {
        let mut items = Vec::new();
        let mut flat = Vec::new();
        self.flatten(&j.children, &mut flat);
        for k in flat {
            if let JsxChild::Element(o) = k {
                if o.tag == "option" {
                    let value = match attr(o, "value") {
                        Some(AttrVal::Str(s)) => str_lit(s),
                        Some(AttrVal::Expr(e)) => self.expr(e, None),
                        _ => "''".to_string(),
                    };
                    let mut f2 = Vec::new();
                    self.flatten(&o.children, &mut f2);
                    let label = self.text_template(&f2);
                    items.push(format!("DropdownMenuItem<dynamic>(value: {}, child: Text({}))", value, label));
                }
            } else if let JsxChild::Expr(Expr::Call(callee, args, _)) = k {
                // options from a list: {xs.map(x => <option value={x}>..</option>)}
                if let (Expr::Member(obj, m, _), Some(Expr::Arrow(f))) = (&**callee, args.first()) {
                    if m == "map" {
                        let list = self.expr(obj, None);
                        let item = match f.params.first().map(|p| &p.pat) {
                            Some(Pat::Ident(n)) => safe_ident(n),
                            _ => "_o".to_string(),
                        };
                        if let FnBody::Expr(body) = &f.body {
                            if let Expr::Jsx(o) = &**body {
                                self.push_scope();
                                if let (Some(Pat::Ident(n)), Some(t)) = (f.params.first().map(|p| &p.pat), self.expr_type(obj)) {
                                    if let Some(inner) = elem_type(&t) {
                                        self.declare(n, Some(inner));
                                    }
                                }
                                let value = match attr(o, "value") {
                                    Some(AttrVal::Expr(e)) => self.expr(e, None),
                                    Some(AttrVal::Str(s)) => str_lit(s),
                                    _ => item.clone(),
                                };
                                let mut f2 = Vec::new();
                                self.flatten(&o.children, &mut f2);
                                let label = self.text_template(&f2);
                                self.pop_scope();
                                items.push(format!("for (final {} in {}) DropdownMenuItem<dynamic>(value: {}, child: Text({}))", item, list, value, label));
                            }
                        }
                    }
                }
            }
        }
        let value = match attr(j, "value") {
            Some(AttrVal::Expr(e)) => self.expr(e, None),
            _ => "null".to_string(),
        };
        let onchange = match attr(j, "onChange") {
            Some(AttrVal::Expr(e)) => self.change_handler(e),
            _ => "null".to_string(),
        };
        let _ = inh;
        let dec = self.input_decoration(j, st);
        let code = format!("DropdownButtonFormField<dynamic>(value: jsOptionValue({}), items: <DropdownMenuItem<dynamic>>[{}], onChanged: {}, decoration: {}, isExpanded: true)", value, items.join(", "), onchange, dec);
        Built { code, ..Default::default() }
    }

    // ---------------------------------------------------------------- components

    fn component_element(&mut self, j: &Jsx, inh: &Inherit, parent: Parent) -> Built {
        let tag = j.tag.as_str();
        // routing
        match tag {
            "Router" | "BrowserRouter" | "HashRouter" | "MemoryRouter" => return Built { code: self.router(j), ..Default::default() },
            "Routes" | "Route" => return Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() },
            "Navigate" => {
                let to = match attr(j, "to") {
                    Some(AttrVal::Str(s)) => str_lit(s),
                    Some(AttrVal::Expr(e)) => self.expr(e, None),
                    _ => "'/'".to_string(),
                };
                let replace = if attr(j, "replace").is_some() { ", replace: true" } else { "" };
                return Built { code: format!("Navigate(to: {}{})", to, replace), ..Default::default() };
            }
            "Link" | "NavLink" => {
                let (st, _) = self.class_of(j);
                let inh2 = merge_inherit(&st, inh);
                let kids = self.children(&j.children, &inh2, Parent::Row);
                let items: Vec<String> = kids.into_iter().map(|k| k.code).collect();
                let to = match attr(j, "to") {
                    Some(AttrVal::Str(s)) => str_lit(s),
                    Some(AttrVal::Expr(e)) => self.expr(e, None),
                    _ => "'/'".to_string(),
                };
                let content = if items.len() == 1 { items[0].clone() } else { format!("Row(mainAxisSize: MainAxisSize.min, children: <Widget>[{}])", items.join(", ")) };
                let inner = self.decorate(&st, content, DecorOpts::default());
                return Built { code: format!("InkWell(onTap: () => navigate(context, {}), child: {})", to, inner), ..Default::default() };
            }
            "Outlet" => {
                self.note("<Outlet/> (nested routes) has no direct Flutter equivalent");
                return Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() };
            }
            "Fragment" | "React.Fragment" | "Suspense" | "StrictMode" | "React.StrictMode" => {
                let kids = self.children(&j.children, inh, parent);
                return Built { code: self.stack_of(kids, "Column"), ..Default::default() };
            }
            _ => {}
        }
        if let Some(ctx) = tag.strip_suffix(".Provider") {
            let value = match attr(j, "value") {
                Some(AttrVal::Expr(e)) => self.expr(e, None),
                _ => "null".to_string(),
            };
            let kids = self.children(&j.children, inh, Parent::Block);
            return Built { code: format!("{}.provider(value: {}, child: {})", ctx, value, self.stack_of(kids, "Column")), ..Default::default() };
        }
        if tag.ends_with(".Consumer") {
            self.note(format!("<{}> render-prop consumers are not converted", tag));
            return Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() };
        }
        // an icon component: lucide-react, or a prop that holds one
        // a local that holds an icon (`const Icon = icons[type]`)
        let is_icon_var = self.scope.icon_vars.contains(tag) || (self.names.contains(tag) && !self.info.comps.contains_key(tag) && !self.lucide_imports.contains(tag));
        let real = self.lucide_alias.get(tag).cloned().unwrap_or_else(|| tag.to_string());
        // an icon: imported from lucide-react, or a known icon name when the project has no component of that name
        if is_icon_var || self.lucide_imports.contains(tag) || (lucide(&real).is_some() && !self.info.comps.contains_key(tag)) {
            return Built { code: self.icon(j, &real, is_icon_var, inh), ..Default::default() };
        }
        // a component of the project
        let props = self.info.comps.get(tag).cloned();
        let Some(props) = props else {
            self.note(format!("<{}> is not a component of this project (an external library?): written as an empty box", tag));
            return Built { code: "const SizedBox.shrink()".to_string(), ..Default::default() };
        };
        let mut args: Vec<String> = Vec::new();
        for a in &j.attrs {
            match a {
                Attr::Kv(name, val) => {
                    if name == "key" || name == "ref" || name.starts_with("data-") || name.starts_with("aria-") {
                        continue;
                    }
                    let pname = safe_ident(name);
                    let fd = props.iter().find(|f| &f.name == name);
                    let exp = fd.map(|f| f.ty.clone());
                    if fd.is_none() && !props.iter().any(|f| f.name == "className") && name != "className" {
                        self.note(format!("<{}> has no prop `{}`: dropped", tag, name));
                        continue;
                    }
                    if fd.is_none() {
                        continue;
                    }
                    let code = match val {
                        AttrVal::Str(s) => str_lit(s),
                        AttrVal::True => "true".to_string(),
                        AttrVal::Expr(e) => {
                            let is_fn = matches!(exp, Some(Ty::Fn(..)));
                            let is_icon = matches!(exp, Some(Ty::Name(ref n, _)) if n == "LucideIcon");
                            match e {
                                Expr::Ident(n) if is_icon => match lucide(self.lucide_alias.get(n).map(|s| s.as_str()).unwrap_or(n)) {
                                    Some(i) => i.to_string(),
                                    None => {
                                        self.note(format!("icon `{}` has no Material mapping: Icons.circle_outlined used", n));
                                        "Icons.circle_outlined".to_string()
                                    }
                                },
                                _ if is_fn => self.fn_prop(e, exp.as_ref()),
                                Expr::Arrow(_) => self.fn_prop(e, exp.as_ref()),
                                _ => self.expr(e, exp.as_ref()),
                            }
                        }
                    };
                    args.push(format!("{}: {}", pname, code));
                }
                Attr::Spread(e) => {
                    self.note(format!("<{} {{...spread}}>: spread props are not passed through", tag));
                    let _ = e;
                }
            }
        }
        let wants_children = props.iter().find(|f| f.name == "children");
        if wants_children.is_some() {
            let kids = self.children(&j.children, inh, Parent::Block);
            if !kids.is_empty() {
                let code = self.stack_of(kids, "Column");
                args.push(format!("children: {}", code));
            }
        }
        Built { code: format!("{}({})", tag, args.join(", ")), ..Default::default() }
    }

    /// A function-valued prop: onClick -> closure; others keep their parameters.
    fn fn_prop(&mut self, e: &Expr, exp: Option<&Ty>) -> String {
        let no_params = matches!(exp, Some(Ty::Fn(p, _)) if p.is_empty());
        match e {
            Expr::Arrow(f) => {
                let names: Vec<String> = f.params.iter().filter_map(|p| match &p.pat {
                    Pat::Ident(n) => Some(n.clone()),
                    _ => None,
                }).collect();
                let before = self.scope.events.len();
                if no_params {
                    self.scope.events.extend(names);
                }
                let code = self.func(f, exp);
                self.scope.events.truncate(before);
                if no_params && !f.params.is_empty() {
                    format!("() {{ ({})({}); }}", code, vec!["null"; f.params.len()].join(", "))
                } else {
                    code
                }
            }
            Expr::Ident(n) if no_params => {
                let arity = self.scope.methods.get(n).copied().unwrap_or(0);
                let code = self.expr(e, None);
                if arity > 0 { format!("() {{ {}({}); }}", code, vec!["null"; arity].join(", ")) } else { code }
            }
            other => self.expr(other, exp),
        }
    }

    fn icon(&mut self, j: &Jsx, tag: &str, is_var: bool, inh: &Inherit) -> String {
        let (st, _) = self.class_of(j);
        let size = match (st.w, st.h) {
            (Some(Size::Px(w)), _) => w,
            (_, Some(Size::Px(h))) => h,
            _ => st.size.or(inh.size).unwrap_or(24.0),
        };
        let color = st.color.or(inh.color).map(|c| format!(", color: {}", c.dart())).unwrap_or_default();
        if st.spin || tag == "Loader2" {
            let col = st.color.or(inh.color).map(|c| c.dart()).unwrap_or_else(|| "Colors.blue".to_string());
            return format!("SizedBox(width: {}, height: {}, child: CircularProgressIndicator(strokeWidth: 2, color: {}))", px(size), px(size), col);
        }
        let data = if is_var {
            self.ident(&tag.to_string())
        } else {
            match lucide(tag) {
                Some(i) => i.to_string(),
                None => {
                    self.note(format!("icon `{}` has no Material mapping: Icons.circle_outlined used", tag));
                    "Icons.circle_outlined".to_string()
                }
            }
        };
        let mar = edge_insets(&st.mar);
        let icon = format!("Icon({}, size: {}{})", data, px(size), color);
        let icon = if is_var { format!("({} == null ? const SizedBox.shrink() : {})", data, icon) } else { icon };
        match mar {
            Some(m) => format!("Padding(padding: {}, child: {})", m, icon),
            None => icon,
        }
    }

    /// `<Router><Routes><Route path element/>...</Routes></Router>` -> a MaterialApp with a route table.
    fn router(&mut self, j: &Jsx) -> String {
        fn find_routes(kids: &[JsxChild]) -> Option<&Jsx> {
            for k in kids {
                if let JsxChild::Element(e) = k {
                    if e.tag == "Routes" || e.tag == "Switch" {
                        return Some(e);
                    }
                    if let Some(r) = find_routes(&e.children) {
                        return Some(r);
                    }
                }
            }
            None
        }
        let mut entries = Vec::new();
        let mut fallback = None;
        if let Some(routes) = find_routes(&j.children) {
            for k in &routes.children {
                let JsxChild::Element(r) = k else { continue };
                if r.tag != "Route" {
                    continue;
                }
                let path = attr_str(r, "path").unwrap_or_else(|| "/".to_string());
                let element = match attr(r, "element") {
                    Some(AttrVal::Expr(Expr::Jsx(e))) => self.element(e, &Inherit::default(), Parent::Block).code,
                    _ => "const SizedBox.shrink()".to_string(),
                };
                if path == "*" {
                    fallback = Some(element);
                } else if path.contains(':') {
                    self.note(format!("route `{}` has a parameter: Flutter named routes do not parse it; use onGenerateRoute", path));
                    entries.push(format!("    {}: (context) => {},", str_lit(&path), element));
                } else {
                    entries.push(format!("    {}: (context) => {},", str_lit(&path), element));
                }
            }
        }
        let fb = fallback.map(|f| format!(",\n  onUnknownRoute: (settings) => MaterialPageRoute(settings: settings, builder: (context) => {})", f)).unwrap_or_default();
        format!(
            "MaterialApp(\n  debugShowCheckedModeBanner: false,\n  title: {},\n  theme: ThemeData(useMaterial3: true, colorSchemeSeed: const Color(0xFF2563EB), scaffoldBackgroundColor: const Color(0xFFF9FAFB)),\n  initialRoute: '/',\n  routes: {{\n{}\n  }}{}\n)",
            str_lit(&self.app_title),
            entries.join("\n"),
            fb
        )
    }
}

#[derive(Default)]
struct DecorOpts {
    alignment: Option<String>,
    in_row: bool,
    no_scroll: bool,
}

/// Widgets that have a size of their own and must not be squeezed: icons, spinners, fixed boxes.
fn icon_like(code: &str) -> bool {
    code.starts_with("Icon(")
        || code.starts_with("const SizedBox")
        || code.starts_with("SizedBox(width")
        || code.starts_with("Divider")
        || code.starts_with("Flexible(")
        || code.starts_with("Expanded(")
        || code.starts_with("Spacer")
        || (code.starts_with("Padding(") && code.contains("child: Icon(") && code.len() < 140)
        || (code.starts_with("Padding(") && code.contains("CircularProgressIndicator") && code.len() < 220)
}

fn wrap_padding(code: &str, inset: &[Option<Size>; 4]) -> String {
    let p = |i: usize| match inset[i] {
        Some(Size::Px(v)) if v != 0.0 => v,
        _ => 0.0,
    };
    let (t, r, b, l) = (p(0), p(1), p(2), p(3));
    if t == 0.0 && r == 0.0 && b == 0.0 && l == 0.0 {
        return code.to_string();
    }
    format!("Padding(padding: EdgeInsets.fromLTRB({}, {}, {}, {}), child: {})", px(l), px(t), px(r), px(b), code)
}
