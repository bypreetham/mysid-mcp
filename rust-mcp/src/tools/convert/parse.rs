//! A recursive-descent reader for TypeScript and TSX: expressions, statements, types and JSX, straight from the characters
//! (JSX needs the parser to decide how to read text, so there is no separate token pass).
//!
//! It reads the language the converter needs, not all of it: no decorators, namespaces or conditional types (those read as
//! `any`). A construct it cannot read ends the module with a problem (line + message); everything before it is kept.

use super::ast::*;

type R<T> = Result<T, String>;

pub struct Parser {
    c: Vec<char>,
    p: usize,
    /// a line break was skipped by the last `ws()`
    nl: bool,
}

pub fn parse_module(src: &str) -> Module {
    let mut ps = Parser { c: src.trim_start_matches('\u{feff}').chars().collect(), p: 0, nl: false };
    let mut m = Module::default();
    loop {
        ps.ws();
        if ps.p >= ps.c.len() {
            break;
        }
        let start = ps.p;
        match ps.item() {
            Ok(Some(i)) => m.items.push(i),
            Ok(None) => {}
            Err(e) => {
                m.problems.push((ps.line_of(start.max(ps.p.min(start + 1))), e));
                break;
            }
        }
        if ps.p == start {
            ps.p += 1;
        }
    }
    m
}

const KEYWORDS: &[&str] = &["if", "else", "for", "while", "do", "return", "throw", "try", "catch", "finally", "switch", "case", "default", "break", "continue", "new", "delete", "typeof", "void", "instanceof", "in", "of", "function", "class", "const", "let", "var", "import", "export", "this", "true", "false", "null", "await", "async", "yield"];

impl Parser {
    // ---------------------------------------------------------------- characters

    fn line_of(&self, at: usize) -> usize {
        self.c[..at.min(self.c.len())].iter().filter(|&&c| c == '\n').count() + 1
    }

    fn err<T>(&self, msg: &str) -> R<T> {
        let near: String = self.c[self.p.min(self.c.len())..].iter().take(30).collect::<String>().replace('\n', " ");
        Err(format!("{} near `{}`", msg, near))
    }

    fn at(&self, i: usize) -> char {
        *self.c.get(self.p + i).unwrap_or(&'\0')
    }

    fn starts(&self, s: &str) -> bool {
        s.chars().enumerate().all(|(i, ch)| self.at(i) == ch)
    }

    /// Skips whitespace and comments; remembers whether a line break was crossed.
    fn ws(&mut self) {
        self.nl = false;
        loop {
            let ch = self.at(0);
            if ch == '\n' {
                self.nl = true;
                self.p += 1;
            } else if ch.is_whitespace() {
                self.p += 1;
            } else if self.starts("//") {
                while self.p < self.c.len() && self.at(0) != '\n' {
                    self.p += 1;
                }
            } else if self.starts("/*") {
                self.p += 2;
                while self.p < self.c.len() && !self.starts("*/") {
                    if self.at(0) == '\n' {
                        self.nl = true;
                    }
                    self.p += 1;
                }
                self.p += 2;
            } else {
                break;
            }
        }
    }

    /// Eats `s` when it comes next (after whitespace). Operators that are prefixes of longer ones are the caller's business.
    fn eat(&mut self, s: &str) -> bool {
        self.ws();
        if self.starts(s) {
            self.p += s.chars().count();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, s: &str) -> R<()> {
        if self.eat(s) { Ok(()) } else { self.err(&format!("expected `{}`", s)) }
    }

    fn peek(&mut self, s: &str) -> bool {
        self.ws();
        self.starts(s)
    }

    fn is_id_start(ch: char) -> bool {
        ch.is_alphabetic() || ch == '_' || ch == '$'
    }

    fn is_id(ch: char) -> bool {
        ch.is_alphanumeric() || ch == '_' || ch == '$'
    }

    fn peek_ident(&mut self) -> Option<String> {
        self.ws();
        if !Self::is_id_start(self.at(0)) {
            return None;
        }
        let mut i = 0;
        while Self::is_id(self.at(i)) {
            i += 1;
        }
        Some(self.c[self.p..self.p + i].iter().collect())
    }

    fn ident(&mut self) -> R<String> {
        match self.peek_ident() {
            Some(s) => {
                self.p += s.chars().count();
                Ok(s)
            }
            None => self.err("expected a name"),
        }
    }

    fn is_kw(&mut self, k: &str) -> bool {
        self.peek_ident().map_or(false, |s| s == k)
    }

    fn eat_kw(&mut self, k: &str) -> bool {
        if self.is_kw(k) {
            self.p += k.chars().count();
            true
        } else {
            false
        }
    }

    /// Runs `f`; when it fails the position goes back (for the places where two readings are possible).
    fn attempt<T>(&mut self, f: impl FnOnce(&mut Self) -> R<T>) -> Option<T> {
        let (p, nl) = (self.p, self.nl);
        match f(self) {
            Ok(v) => Some(v),
            Err(_) => {
                self.p = p;
                self.nl = nl;
                None
            }
        }
    }

    // ---------------------------------------------------------------- items

    fn item(&mut self) -> R<Option<Item>> {
        self.ws();
        if self.eat_kw("import") {
            // `import type { X } from` and `import('x')` as a statement are both fine to skip as an import line
            if self.peek("(") {
                self.p -= 6;
                return Ok(Some(Item::Stmt(self.stmt()?)));
            }
            return self.import().map(Some);
        }
        if self.is_kw("export") {
            self.p += 6;
            return self.export();
        }
        self.decl_or_stmt(false, false)
    }

    fn import(&mut self) -> R<Item> {
        let mut imp = Import { default: None, named: Vec::new(), namespace: None, from: String::new() };
        if self.is_kw("type") {
            // `import type { X }`: the word `type` is a modifier unless it is the default name (`import type from`)
            let save = self.p;
            self.p += 4;
            if self.peek("{") || self.peek("*") || self.peek_ident().map_or(false, |s| s != "from") {
            } else {
                self.p = save;
            }
        }
        if self.peek("'") || self.peek("\"") {
            imp.from = self.string()?;
            self.eat(";");
            return Ok(Item::Import(imp));
        }
        if let Some(id) = self.peek_ident().filter(|s| s != "from") {
            self.p += id.chars().count();
            imp.default = Some(id);
            self.eat(",");
        }
        if self.eat("*") {
            if !self.eat_kw("as") {
                return self.err("expected `as`");
            }
            imp.namespace = Some(self.ident()?);
        } else if self.eat("{") {
            loop {
                if self.eat("}") {
                    break;
                }
                self.eat_kw("type");
                let name = self.ident()?;
                let local = if self.eat_kw("as") { self.ident()? } else { name.clone() };
                imp.named.push((name, local));
                if !self.eat(",") {
                    self.expect("}")?;
                    break;
                }
            }
        }
        if !self.eat_kw("from") {
            return self.err("expected `from`");
        }
        imp.from = self.string()?;
        self.eat(";");
        Ok(Item::Import(imp))
    }

    fn export(&mut self) -> R<Option<Item>> {
        if self.eat_kw("default") {
            if self.is_kw("function") || self.is_kw("async") || self.is_kw("class") {
                return self.decl_or_stmt(true, true);
            }
            let e = self.expr()?;
            self.eat(";");
            return Ok(Some(Item::ExportDefault(e)));
        }
        if self.peek("{") {
            self.p += 1;
            let mut names = Vec::new();
            loop {
                if self.eat("}") {
                    break;
                }
                let n = self.ident()?;
                let alias = if self.eat_kw("as") { self.ident()? } else { n.clone() };
                names.push((n, alias));
                if !self.eat(",") {
                    self.expect("}")?;
                    break;
                }
            }
            if self.eat_kw("from") {
                self.string()?;
            }
            self.eat(";");
            return Ok(Some(Item::ExportNames(names)));
        }
        if self.eat("*") {
            if self.eat_kw("as") {
                self.ident()?;
            }
            if self.eat_kw("from") {
                self.string()?;
            }
            self.eat(";");
            return Ok(None);
        }
        self.decl_or_stmt(true, false)
    }

    fn decl_or_stmt(&mut self, exported: bool, default: bool) -> R<Option<Item>> {
        self.ws();
        let word = self.peek_ident().unwrap_or_default();
        match word.as_str() {
            "interface" => {
                self.p += 9;
                self.interface(exported).map(Some)
            }
            "type" if { let save = self.p; self.p += 4; let ok = self.peek_ident().is_some(); self.p = save; ok } => {
                self.p += 4;
                let name = self.ident()?;
                self.type_params()?;
                self.expect("=")?;
                let ty = self.ty()?;
                self.eat(";");
                Ok(Some(Item::TypeAlias { name, ty, exported }))
            }
            "enum" | "const" if word == "enum" || { let save = self.p; self.p += 5; let ok = self.is_kw("enum"); self.p = save; ok } => {
                if word == "const" {
                    self.p += 5;
                }
                self.eat_kw("enum");
                let name = self.ident()?;
                self.expect("{")?;
                let mut members = Vec::new();
                loop {
                    if self.eat("}") {
                        break;
                    }
                    let n = if self.peek("'") || self.peek("\"") { self.string()? } else { self.ident()? };
                    let v = if self.eat("=") { Some(self.assign()?) } else { None };
                    members.push((n, v));
                    if !self.eat(",") {
                        self.expect("}")?;
                        break;
                    }
                }
                Ok(Some(Item::Enum { name, members, exported }))
            }
            "declare" => {
                // ambient declarations carry no code
                self.p += 7;
                self.decl_or_stmt(exported, default)?;
                Ok(None)
            }
            "abstract" => {
                self.p += 8;
                self.decl_or_stmt(exported, default)
            }
            "class" => {
                self.p += 5;
                self.class(exported, default).map(Some)
            }
            "function" => {
                self.p += 8;
                let f = self.function(false)?;
                Ok(Some(Item::Func { func: f, exported, default }))
            }
            "async" if { let save = self.p; self.p += 5; let ok = self.is_kw("function"); self.p = save; ok } => {
                self.p += 5;
                self.p += { self.ws(); 8 };
                let f = self.function(true)?;
                Ok(Some(Item::Func { func: f, exported, default }))
            }
            "const" | "let" | "var" => {
                self.p += word.chars().count();
                let decls = self.var_decls()?;
                self.eat(";");
                Ok(Some(Item::Var { kind: word, decls, exported }))
            }
            _ => Ok(Some(Item::Stmt(self.stmt()?))),
        }
    }

    fn interface(&mut self, exported: bool) -> R<Item> {
        let name = self.ident()?;
        self.type_params()?;
        let mut extends = Vec::new();
        if self.eat_kw("extends") {
            loop {
                extends.push(self.ty_primary()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        let fields = match self.ty_object()? {
            Ty::Obj(f) => f,
            _ => Vec::new(),
        };
        Ok(Item::Interface { name, extends, fields, exported })
    }

    fn class(&mut self, exported: bool, default: bool) -> R<Item> {
        let name = self.peek_ident().filter(|s| s != "extends").map(|s| {
            self.p += s.chars().count();
            s
        }).unwrap_or_default();
        self.type_params()?;
        let extends = if self.eat_kw("extends") { Some(self.ty_primary()?) } else { None };
        if self.eat_kw("implements") {
            loop {
                self.ty_primary()?;
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect("{")?;
        let mut members = Vec::new();
        loop {
            if self.eat("}") {
                break;
            }
            if self.eat(";") {
                continue;
            }
            let mut is_static = false;
            let mut is_async = false;
            loop {
                if self.eat_kw("static") {
                    is_static = true;
                } else if self.eat_kw("async") {
                    is_async = true;
                } else if self.eat_kw("public") || self.eat_kw("private") || self.eat_kw("protected") || self.eat_kw("readonly") || self.eat_kw("override") {
                } else {
                    break;
                }
            }
            let mname = if self.eat("#") { format!("#{}", self.ident()?) } else { self.ident()? };
            self.eat("?");
            if self.peek("(") || self.peek("<") {
                self.type_params()?;
                let params = self.params()?;
                let ret = if self.eat(":") { Some(self.ty()?) } else { None };
                let body = self.block()?;
                members.push(ClassMember { name: mname.clone(), is_static, kind: MemberKind::Method(Func { name: Some(mname), params, ret, body: FnBody::Block(body), is_async }) });
            } else {
                let ty = if self.eat(":") { Some(self.ty()?) } else { None };
                let init = if self.eat("=") { Some(self.assign()?) } else { None };
                self.eat(";");
                members.push(ClassMember { name: mname, is_static, kind: MemberKind::Field(ty, init) });
            }
        }
        Ok(Item::Class { name, extends, members, exported, default })
    }

    fn function(&mut self, is_async: bool) -> R<Func> {
        self.eat("*");
        let name = self.peek_ident().map(|s| {
            self.p += s.chars().count();
            s
        });
        self.type_params()?;
        let params = self.params()?;
        let ret = if self.eat(":") { Some(self.ty()?) } else { None };
        let body = self.block()?;
        Ok(Func { name, params, ret, body: FnBody::Block(body), is_async })
    }

    /// `<T, U extends X = Y>`: skipped; the names do not matter to Dart output.
    fn type_params(&mut self) -> R<()> {
        if !self.peek("<") {
            return Ok(());
        }
        self.p += 1;
        let mut depth = 1;
        while depth > 0 {
            if self.p >= self.c.len() {
                return self.err("unterminated `<`");
            }
            let prev = if self.p > 0 { self.c[self.p - 1] } else { ' ' };
            match self.at(0) {
                '<' => depth += 1,
                '>' if prev != '=' => depth -= 1,
                _ => {}
            }
            self.p += 1;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- statements

    fn block(&mut self) -> R<Vec<Stmt>> {
        self.expect("{")?;
        let mut out = Vec::new();
        loop {
            if self.eat("}") {
                return Ok(out);
            }
            if self.p >= self.c.len() {
                return self.err("unterminated block");
            }
            out.push(self.stmt()?);
        }
    }

    fn var_decls(&mut self) -> R<Vec<VarDecl>> {
        let mut out = Vec::new();
        loop {
            let pat = self.pattern()?;
            self.eat("!");
            let ty = if self.eat(":") { Some(self.ty()?) } else { None };
            let init = if self.peek("=") && !self.starts("==") && !self.starts("=>") {
                self.p += 1;
                Some(self.assign()?)
            } else {
                None
            };
            out.push(VarDecl { pat, ty, init });
            if !self.eat(",") {
                return Ok(out);
            }
        }
    }

    fn pattern(&mut self) -> R<Pat> {
        self.ws();
        if self.eat("{") {
            let mut props = Vec::new();
            loop {
                if self.eat("}") {
                    break;
                }
                let rest = self.eat("...");
                let key = if self.peek("'") || self.peek("\"") { self.string()? } else { self.ident()? };
                let value = if self.eat(":") { self.pattern()? } else { Pat::Ident(key.clone()) };
                let default = if self.peek("=") && !self.starts("==") && !self.starts("=>") {
                    self.p += 1;
                    Some(self.assign()?)
                } else {
                    None
                };
                props.push(PatProp { key, value, default, rest });
                if !self.eat(",") {
                    self.expect("}")?;
                    break;
                }
            }
            return Ok(Pat::Object(props));
        }
        if self.eat("[") {
            let mut items = Vec::new();
            loop {
                if self.eat("]") {
                    break;
                }
                if self.eat(",") {
                    items.push(None);
                    continue;
                }
                self.eat("...");
                items.push(Some(self.pattern()?));
                if !self.eat(",") {
                    self.expect("]")?;
                    break;
                }
            }
            return Ok(Pat::Array(items));
        }
        Ok(Pat::Ident(self.ident()?))
    }

    fn params(&mut self) -> R<Vec<Param>> {
        self.expect("(")?;
        let mut out = Vec::new();
        loop {
            if self.eat(")") {
                return Ok(out);
            }
            while self.eat_kw("public") || self.eat_kw("private") || self.eat_kw("protected") || self.eat_kw("readonly") {}
            let rest = self.eat("...");
            let pat = self.pattern()?;
            let optional = self.eat("?");
            let ty = if self.eat(":") { Some(self.ty()?) } else { None };
            let default = if self.peek("=") && !self.starts("==") && !self.starts("=>") {
                self.p += 1;
                Some(self.assign()?)
            } else {
                None
            };
            out.push(Param { pat, ty, default, optional, rest });
            if !self.eat(",") {
                self.expect(")")?;
                return Ok(out);
            }
        }
    }

    fn stmt(&mut self) -> R<Stmt> {
        self.ws();
        if self.peek("{") {
            return Ok(Stmt::Block(self.block()?));
        }
        if self.eat(";") {
            return Ok(Stmt::Empty);
        }
        let word = self.peek_ident().unwrap_or_default();
        match word.as_str() {
            "const" | "let" | "var" => {
                self.p += word.chars().count();
                let d = self.var_decls()?;
                self.eat(";");
                Ok(Stmt::Var(word, d))
            }
            "if" => {
                self.p += 2;
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                let t = self.stmt()?;
                let e = if self.eat_kw("else") { Some(Box::new(self.stmt()?)) } else { None };
                Ok(Stmt::If(c, Box::new(t), e))
            }
            "return" => {
                self.p += 6;
                let line_break_before = {
                    let save = self.p;
                    self.ws();
                    let nl = self.nl;
                    self.p = save;
                    nl
                };
                if line_break_before || self.peek(";") || self.peek("}") {
                    self.eat(";");
                    return Ok(Stmt::Return(None));
                }
                let e = self.expr()?;
                self.eat(";");
                Ok(Stmt::Return(Some(e)))
            }
            "throw" => {
                self.p += 5;
                let e = self.expr()?;
                self.eat(";");
                Ok(Stmt::Throw(e))
            }
            "break" | "continue" => {
                self.p += word.chars().count();
                if self.peek_ident().is_some() && !self.nl {
                    self.ident()?;
                }
                self.eat(";");
                Ok(if word == "break" { Stmt::Break } else { Stmt::Continue })
            }
            "for" => {
                self.p += 3;
                self.eat_kw("await");
                self.expect("(")?;
                let decl_kind = ["const", "let", "var"].iter().find(|k| self.is_kw(k)).map(|k| k.to_string());
                if let Some(kind) = &decl_kind {
                    let save = self.p;
                    self.p += kind.len();
                    let pat = self.pattern()?;
                    if self.is_kw("of") || self.is_kw("in") {
                        let is_in = self.is_kw("in");
                        self.p += 2;
                        let it = self.expr()?;
                        self.expect(")")?;
                        let body = self.stmt()?;
                        return Ok(Stmt::ForOf(kind.clone(), pat, it, Box::new(body), is_in));
                    }
                    self.p = save;
                }
                let init = if self.eat(";") {
                    None
                } else if decl_kind.is_some() {
                    let kind = decl_kind.unwrap();
                    self.p += kind.len();
                    let d = self.var_decls()?;
                    self.expect(";")?;
                    Some(Box::new(Stmt::Var(kind, d)))
                } else {
                    let e = self.expr()?;
                    self.expect(";")?;
                    Some(Box::new(Stmt::Expr(e)))
                };
                let test = if self.peek(";") { None } else { Some(self.expr()?) };
                self.expect(";")?;
                let update = if self.peek(")") { None } else { Some(self.expr()?) };
                self.expect(")")?;
                let body = self.stmt()?;
                Ok(Stmt::For(init, test, update, Box::new(body)))
            }
            "while" => {
                self.p += 5;
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                Ok(Stmt::While(c, Box::new(self.stmt()?)))
            }
            "do" => {
                self.p += 2;
                let body = self.stmt()?;
                if !self.eat_kw("while") {
                    return self.err("expected `while`");
                }
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                self.eat(";");
                Ok(Stmt::DoWhile(Box::new(body), c))
            }
            "try" => {
                self.p += 3;
                let body = self.block()?;
                let catch = if self.eat_kw("catch") {
                    let param = if self.eat("(") {
                        let p = self.pattern()?;
                        if self.eat(":") {
                            self.ty()?;
                        }
                        self.expect(")")?;
                        Some(p)
                    } else {
                        None
                    };
                    Some(CatchClause { param, body: self.block()? })
                } else {
                    None
                };
                let fin = if self.eat_kw("finally") { Some(self.block()?) } else { None };
                Ok(Stmt::Try(body, catch, fin))
            }
            "switch" => {
                self.p += 6;
                self.expect("(")?;
                let d = self.expr()?;
                self.expect(")")?;
                self.expect("{")?;
                let mut cases = Vec::new();
                loop {
                    if self.eat("}") {
                        break;
                    }
                    let test = if self.eat_kw("case") {
                        let t = self.expr()?;
                        Some(t)
                    } else if self.eat_kw("default") {
                        None
                    } else {
                        return self.err("expected `case` or `default`");
                    };
                    self.expect(":")?;
                    let mut body = Vec::new();
                    while !self.is_kw("case") && !self.is_kw("default") && !self.peek("}") {
                        body.push(self.stmt()?);
                    }
                    cases.push((test, body));
                }
                Ok(Stmt::Switch(d, cases))
            }
            "function" => {
                self.p += 8;
                Ok(Stmt::Func(self.function(false)?))
            }
            "async" if { let save = self.p; self.p += 5; let ok = self.is_kw("function"); self.p = save; ok } => {
                self.p += 5;
                self.ws();
                self.p += 8;
                Ok(Stmt::Func(self.function(true)?))
            }
            "interface" | "type" | "enum" | "class" => {
                // a declaration inside a block: keep the statement shape by reading and dropping it
                match self.decl_or_stmt(false, false)? {
                    Some(Item::Stmt(s)) => Ok(s),
                    _ => Ok(Stmt::Empty),
                }
            }
            _ => {
                // a label: `name:` before a loop
                if let Some(id) = self.peek_ident() {
                    let save = self.p;
                    self.p += id.chars().count();
                    if self.peek(":") && !self.starts("::") && !KEYWORDS.contains(&id.as_str()) {
                        self.p += 1;
                        return self.stmt();
                    }
                    self.p = save;
                }
                let e = self.expr()?;
                self.eat(";");
                Ok(Stmt::Expr(e))
            }
        }
    }

    // ---------------------------------------------------------------- expressions

    pub fn expr(&mut self) -> R<Expr> {
        let first = self.assign()?;
        if self.peek(",") && !self.starts(",)") {
            // a comma sequence keeps only its last value (the earlier ones are evaluated for effect, which Dart cannot express inline)
            let mut last = first;
            while self.eat(",") {
                last = self.assign()?;
            }
            return Ok(last);
        }
        Ok(first)
    }

    fn assign(&mut self) -> R<Expr> {
        self.ws();
        if let Some(f) = self.attempt(|s| s.arrow()) {
            return Ok(Expr::Arrow(Box::new(f)));
        }
        let lhs = self.cond()?;
        self.ws();
        for op in ["**=", "<<=", ">>=", ">>>=", "&&=", "||=", "??=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "="] {
            if self.starts(op) {
                if op == "=" && (self.starts("==") || self.starts("=>")) {
                    break;
                }
                self.p += op.len();
                let rhs = self.assign()?;
                return Ok(Expr::Assign(op.to_string(), Box::new(lhs), Box::new(rhs)));
            }
        }
        Ok(lhs)
    }

    fn arrow(&mut self) -> R<Func> {
        let is_async = {
            let save = self.p;
            if self.eat_kw("async") {
                self.ws();
                if self.at(0) == '(' || Self::is_id_start(self.at(0)) { true } else { self.p = save; false }
            } else {
                false
            }
        };
        self.type_params()?;
        self.ws();
        let params = if self.at(0) == '(' {
            self.params()?
        } else {
            let n = self.ident()?;
            vec![Param { pat: Pat::Ident(n), ty: None, default: None, optional: false, rest: false }]
        };
        let ret = if self.eat(":") { Some(self.ty()?) } else { None };
        self.ws();
        if !self.starts("=>") {
            return self.err("not an arrow function");
        }
        self.p += 2;
        self.ws();
        let body = if self.at(0) == '{' { FnBody::Block(self.block()?) } else { FnBody::Expr(Box::new(self.assign()?)) };
        Ok(Func { name: None, params, ret, body, is_async })
    }

    fn cond(&mut self) -> R<Expr> {
        let c = self.binary(0)?;
        self.ws();
        if self.at(0) == '?' && self.at(1) != '.' && self.at(1) != '?' {
            self.p += 1;
            let t = self.assign()?;
            self.expect(":")?;
            let f = self.assign()?;
            return Ok(Expr::Cond(Box::new(c), Box::new(t), Box::new(f)));
        }
        Ok(c)
    }

    fn binop(&mut self) -> Option<(&'static str, u8)> {
        self.ws();
        const OPS: &[(&str, u8)] = &[
            ("??", 1), ("||", 2), ("&&", 3), ("===", 7), ("!==", 7), ("==", 7), ("!=", 7), (">>>", 9), ("<<", 9), (">>", 9), ("<=", 8), (">=", 8),
            ("**", 12), ("|", 4), ("^", 5), ("&", 6), ("<", 8), (">", 8), ("+", 10), ("-", 10), ("*", 11), ("/", 11), ("%", 11),
        ];
        for (op, prec) in OPS {
            if self.starts(op) {
                let next = self.at(op.len());
                // an assignment operator or arrow, not a binary one
                if next == '=' && !matches!(*op, "===" | "!==" | "==" | "!=" | "<=" | ">=") {
                    return None;
                }
                if *op == "|" && next == '|' || *op == "&" && next == '&' {
                    continue;
                }
                return Some((op, *prec));
            }
        }
        for (kw, prec) in [("instanceof", 8u8), ("in", 8u8)] {
            if self.is_kw(kw) {
                return Some((if kw == "in" { "in" } else { "instanceof" }, prec));
            }
        }
        None
    }

    fn binary(&mut self, min: u8) -> R<Expr> {
        let mut left = self.unary()?;
        loop {
            self.ws();
            if (self.is_kw("as") || self.is_kw("satisfies")) && 8 >= min {
                let was_as = self.is_kw("as");
                self.p += if was_as { 2 } else { 9 };
                if self.eat_kw("const") {
                    continue;
                }
                let t = self.ty()?;
                left = Expr::As(Box::new(left), t);
                continue;
            }
            let Some((op, prec)) = self.binop() else { break };
            if prec < min {
                break;
            }
            self.p += op.len();
            let right = if op == "**" { self.binary(prec)? } else { self.binary(prec + 1)? };
            left = Expr::Binary(op.to_string(), Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> R<Expr> {
        self.ws();
        for op in ["++", "--"] {
            if self.starts(op) {
                self.p += 2;
                let e = self.unary()?;
                return Ok(Expr::Update(op.to_string(), Box::new(e), true));
            }
        }
        for op in ["!", "-", "+", "~"] {
            if self.starts(op) {
                self.p += 1;
                let e = self.unary()?;
                return Ok(Expr::Unary(op.to_string(), Box::new(e)));
            }
        }
        for kw in ["typeof", "void", "delete"] {
            if self.is_kw(kw) {
                self.p += kw.len();
                let e = self.unary()?;
                return Ok(Expr::Unary(kw.to_string(), Box::new(e)));
            }
        }
        if self.is_kw("await") {
            self.p += 5;
            let e = self.unary()?;
            return Ok(Expr::Await(Box::new(e)));
        }
        self.postfix()
    }

    fn args(&mut self) -> R<Vec<Expr>> {
        self.expect("(")?;
        let mut out = Vec::new();
        loop {
            if self.eat(")") {
                return Ok(out);
            }
            if self.eat("...") {
                out.push(Expr::Spread(Box::new(self.assign()?)));
            } else {
                out.push(self.assign()?);
            }
            if !self.eat(",") {
                self.expect(")")?;
                return Ok(out);
            }
        }
    }

    fn postfix(&mut self) -> R<Expr> {
        let mut e = self.primary()?;
        // type arguments seen before a call: `useState<Foo>(...)` keeps `Foo` as `As(call, __targs<Foo>)`
        let mut targs: Option<Vec<Ty>> = None;
        loop {
            self.ws();
            if self.starts("?.") && self.at(2) != '(' && self.at(2) != '[' && !self.at(2).is_ascii_digit() {
                self.p += 2;
                let name = self.member_name()?;
                e = Expr::Member(Box::new(e), name, true);
            } else if self.starts("?.(") {
                self.p += 2;
                let a = self.args()?;
                e = Expr::Call(Box::new(e), a, true);
            } else if self.starts("?.[") {
                self.p += 3;
                let i = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(i), true);
            } else if self.at(0) == '.' && !self.starts("...") {
                self.p += 1;
                let name = self.member_name()?;
                e = Expr::Member(Box::new(e), name, false);
            } else if self.at(0) == '[' && !self.nl_before_bracket() {
                self.p += 1;
                let i = self.expr()?;
                self.expect("]")?;
                e = Expr::Index(Box::new(e), Box::new(i), false);
            } else if self.at(0) == '(' {
                let a = self.args()?;
                e = Expr::Call(Box::new(e), a, false);
                if let Some(t) = targs.take() {
                    e = Expr::As(Box::new(e), Ty::Name("__targs".to_string(), t));
                }
            } else if self.at(0) == '!' && self.at(1) != '=' && !self.nl {
                self.p += 1;
                e = Expr::NonNull(Box::new(e));
            } else if (self.starts("++") || self.starts("--")) && !self.nl {
                let op = if self.starts("++") { "++" } else { "--" };
                self.p += 2;
                e = Expr::Update(op.to_string(), Box::new(e), false);
            } else if self.at(0) == '<' {
                // `useState<Foo>(...)`: type arguments, only when a call follows
                let ok = self.attempt(|s| {
                    let args = s.ty_args()?;
                    s.ws();
                    if s.at(0) == '(' { Ok(args) } else { s.err("not type arguments") }
                });
                match ok {
                    Some(a) => targs = Some(a),
                    None => break,
                }
            } else if self.at(0) == '`' {
                // a tagged template: keep the call shape
                let t = self.template()?;
                e = Expr::Call(Box::new(e), vec![t], false);
            } else {
                break;
            }
        }
        Ok(e)
    }

    fn nl_before_bracket(&self) -> bool {
        self.nl
    }

    fn member_name(&mut self) -> R<String> {
        self.ws();
        if self.at(0) == '#' {
            self.p += 1;
            return Ok(format!("#{}", self.ident()?));
        }
        self.ident()
    }

    fn primary(&mut self) -> R<Expr> {
        self.ws();
        let ch = self.at(0);
        if ch.is_ascii_digit() || (ch == '.' && self.at(1).is_ascii_digit()) {
            return Ok(Expr::Num(self.number()));
        }
        match ch {
            '\'' | '"' => return Ok(Expr::Str(self.string()?)),
            '`' => return self.template(),
            '(' => {
                self.p += 1;
                let e = self.expr()?;
                self.expect(")")?;
                return Ok(e);
            }
            '[' => {
                self.p += 1;
                let mut items = Vec::new();
                loop {
                    if self.eat("]") {
                        break;
                    }
                    if self.eat(",") {
                        continue;
                    }
                    if self.eat("...") {
                        items.push(Expr::Spread(Box::new(self.assign()?)));
                    } else {
                        items.push(self.assign()?);
                    }
                    if !self.eat(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                return Ok(Expr::Array(items));
            }
            '{' => return self.object(),
            '<' => {
                let j = self.jsx()?;
                return Ok(Expr::Jsx(Box::new(j)));
            }
            '/' => return self.regex(),
            _ => {}
        }
        let word = self.peek_ident().unwrap_or_default();
        match word.as_str() {
            "function" => {
                self.p += 8;
                let f = self.function(false)?;
                Ok(Expr::Arrow(Box::new(f)))
            }
            "async" if { let save = self.p; self.p += 5; let ok = self.is_kw("function"); self.p = save; ok } => {
                self.p += 5;
                self.ws();
                self.p += 8;
                let f = self.function(true)?;
                Ok(Expr::Arrow(Box::new(f)))
            }
            "new" => {
                self.p += 3;
                let mut callee = self.primary_for_new()?;
                self.ws();
                if self.at(0) == '<' {
                    self.attempt(|s| {
                        s.type_params()?;
                        s.ws();
                        if s.at(0) == '(' { Ok(()) } else { s.err("not type arguments") }
                    });
                }
                let args = if self.peek("(") { self.args()? } else { Vec::new() };
                if let Expr::Ident(_) | Expr::Member(..) = callee {
                } else {
                    callee = Expr::Ident("Object".to_string());
                }
                Ok(Expr::New(Box::new(callee), args))
            }
            "this" => {
                self.p += 4;
                Ok(Expr::This)
            }
            "true" => {
                self.p += 4;
                Ok(Expr::Bool(true))
            }
            "false" => {
                self.p += 5;
                Ok(Expr::Bool(false))
            }
            "null" => {
                self.p += 4;
                Ok(Expr::Null)
            }
            "undefined" => {
                self.p += 9;
                Ok(Expr::Undefined)
            }
            "" => self.err("expected an expression"),
            _ => {
                self.p += word.chars().count();
                Ok(Expr::Ident(word))
            }
        }
    }

    /// The thing after `new`: a name or a dotted name, without calls.
    fn primary_for_new(&mut self) -> R<Expr> {
        let mut e = Expr::Ident(self.ident()?);
        while self.peek(".") && !self.starts("...") {
            self.p += 1;
            let n = self.ident()?;
            e = Expr::Member(Box::new(e), n, false);
        }
        Ok(e)
    }

    fn number(&mut self) -> String {
        let start = self.p;
        if self.at(0) == '0' && matches!(self.at(1), 'x' | 'X' | 'b' | 'B' | 'o' | 'O') {
            self.p += 2;
            while self.at(0).is_ascii_hexdigit() || self.at(0) == '_' {
                self.p += 1;
            }
        } else {
            while self.at(0).is_ascii_digit() || self.at(0) == '_' || self.at(0) == '.' && self.at(1).is_ascii_digit() || self.at(0) == '.' && !Self::is_id_start(self.at(1)) && self.at(1) != '.' {
                self.p += 1;
            }
            if matches!(self.at(0), 'e' | 'E') && (self.at(1).is_ascii_digit() || (matches!(self.at(1), '+' | '-') && self.at(2).is_ascii_digit())) {
                self.p += 2;
                while self.at(0).is_ascii_digit() {
                    self.p += 1;
                }
            }
        }
        let n: String = self.c[start..self.p].iter().filter(|&&c| c != '_').collect();
        if self.at(0) == 'n' && !Self::is_id(self.at(1)) {
            self.p += 1;
        }
        n
    }

    /// The decoded value of a quoted string (JS escapes resolved).
    fn string(&mut self) -> R<String> {
        self.ws();
        let q = self.at(0);
        if q != '\'' && q != '"' {
            return self.err("expected a string");
        }
        self.p += 1;
        let mut out = String::new();
        loop {
            if self.p >= self.c.len() {
                return self.err("unterminated string");
            }
            let ch = self.at(0);
            self.p += 1;
            if ch == q {
                return Ok(out);
            }
            if ch == '\\' {
                let e = self.at(0);
                self.p += 1;
                match e {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    'b' => out.push('\u{8}'),
                    'f' => out.push('\u{c}'),
                    'v' => out.push('\u{b}'),
                    '0' => out.push('\0'),
                    'x' => {
                        let h: String = (0..2).map(|i| self.at(i)).collect();
                        self.p += 2;
                        out.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                    }
                    'u' => {
                        if self.at(0) == '{' {
                            let mut h = String::new();
                            self.p += 1;
                            while self.at(0) != '}' && self.p < self.c.len() {
                                h.push(self.at(0));
                                self.p += 1;
                            }
                            self.p += 1;
                            out.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                        } else {
                            let h: String = (0..4).map(|i| self.at(i)).collect();
                            self.p += 4;
                            out.push(u32::from_str_radix(&h, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                        }
                    }
                    '\n' => {}
                    other => out.push(other),
                }
            } else {
                out.push(ch);
            }
        }
    }

    fn template(&mut self) -> R<Expr> {
        self.ws();
        self.p += 1; // `
        let mut quasis = Vec::new();
        let mut exprs = Vec::new();
        let mut cur = String::new();
        loop {
            if self.p >= self.c.len() {
                return self.err("unterminated template");
            }
            let ch = self.at(0);
            if ch == '`' {
                self.p += 1;
                quasis.push(cur);
                return Ok(Expr::Template(quasis, exprs));
            }
            if ch == '\\' {
                cur.push(ch);
                cur.push(self.at(1));
                self.p += 2;
                continue;
            }
            if ch == '$' && self.at(1) == '{' {
                self.p += 2;
                quasis.push(std::mem::take(&mut cur));
                let e = self.expr()?;
                self.expect("}")?;
                exprs.push(e);
                continue;
            }
            cur.push(ch);
            self.p += 1;
        }
    }

    fn regex(&mut self) -> R<Expr> {
        let start = self.p;
        self.p += 1;
        let mut in_class = false;
        loop {
            if self.p >= self.c.len() || self.at(0) == '\n' {
                return self.err("unterminated regular expression");
            }
            let ch = self.at(0);
            self.p += 1;
            match ch {
                '\\' => self.p += 1,
                '[' => in_class = true,
                ']' => in_class = false,
                '/' if !in_class => break,
                _ => {}
            }
        }
        while Self::is_id(self.at(0)) {
            self.p += 1;
        }
        Ok(Expr::Regex(self.c[start..self.p].iter().collect()))
    }

    fn object(&mut self) -> R<Expr> {
        self.expect("{")?;
        let mut props = Vec::new();
        loop {
            if self.eat("}") {
                break;
            }
            if self.eat("...") {
                props.push(Prop::Spread(self.assign()?));
            } else {
                self.ws();
                let is_async = {
                    let save = self.p;
                    if self.eat_kw("async") {
                        self.ws();
                        if matches!(self.at(0), ',' | ':' | '(' | '}') { self.p = save; false } else { true }
                    } else {
                        false
                    }
                };
                // `get x() {}` / `set x(v) {}` read as methods
                let key = if self.at(0) == '[' {
                    self.p += 1;
                    let e = self.assign()?;
                    self.expect("]")?;
                    PropKey::Computed(e)
                } else if self.at(0) == '\'' || self.at(0) == '"' {
                    PropKey::Name(self.string()?)
                } else if self.at(0).is_ascii_digit() {
                    PropKey::Name(self.number())
                } else {
                    PropKey::Name(self.ident()?)
                };
                self.ws();
                if self.at(0) == ':' {
                    self.p += 1;
                    let v = self.assign()?;
                    props.push(Prop::Kv(key, v));
                } else if self.at(0) == '(' || self.at(0) == '<' {
                    self.type_params()?;
                    let params = self.params()?;
                    let ret = if self.eat(":") { Some(self.ty()?) } else { None };
                    let body = self.block()?;
                    props.push(Prop::Kv(key, Expr::Arrow(Box::new(Func { name: None, params, ret, body: FnBody::Block(body), is_async }))));
                } else if let PropKey::Name(n) = key {
                    // shorthand, possibly with a default (`{ a = 1 }` only in patterns; read and keep the name)
                    if self.peek("=") && !self.starts("==") {
                        self.p += 1;
                        self.assign()?;
                    }
                    props.push(Prop::Short(n));
                } else {
                    return self.err("expected `:` in an object");
                }
            }
            if !self.eat(",") {
                self.expect("}")?;
                break;
            }
        }
        Ok(Expr::Object(props))
    }

    // ---------------------------------------------------------------- JSX

    fn jsx_name(&mut self) -> R<String> {
        self.ws();
        let mut s = String::new();
        while Self::is_id(self.at(0)) || matches!(self.at(0), '.' | '-' | ':') {
            s.push(self.at(0));
            self.p += 1;
        }
        Ok(s)
    }

    fn jsx(&mut self) -> R<Jsx> {
        self.expect("<")?;
        if self.eat(">") {
            let children = self.jsx_children("")?;
            return Ok(Jsx { tag: String::new(), attrs: Vec::new(), children });
        }
        let tag = self.jsx_name()?;
        if tag.is_empty() {
            return self.err("expected a tag name");
        }
        // generic component `<Foo<T> ...>`
        if self.peek("<") {
            self.type_params()?;
        }
        let mut attrs = Vec::new();
        loop {
            self.ws();
            if self.starts("/>") {
                self.p += 2;
                return Ok(Jsx { tag, attrs, children: Vec::new() });
            }
            if self.at(0) == '>' {
                self.p += 1;
                let children = self.jsx_children(&tag)?;
                return Ok(Jsx { tag, attrs, children });
            }
            if self.at(0) == '{' {
                self.p += 1;
                self.expect("...")?;
                let e = self.assign()?;
                self.expect("}")?;
                attrs.push(Attr::Spread(e));
                continue;
            }
            let name = self.jsx_name()?;
            if name.is_empty() {
                return self.err("expected an attribute name");
            }
            self.ws();
            if self.at(0) == '=' && self.at(1) != '=' {
                self.p += 1;
                self.ws();
                let val = match self.at(0) {
                    '"' | '\'' => {
                        let q = self.at(0);
                        self.p += 1;
                        let start = self.p;
                        while self.p < self.c.len() && self.at(0) != q {
                            self.p += 1;
                        }
                        let raw: String = self.c[start..self.p].iter().collect();
                        self.p += 1;
                        AttrVal::Str(raw)
                    }
                    '{' => {
                        self.p += 1;
                        let e = self.expr()?;
                        self.expect("}")?;
                        AttrVal::Expr(e)
                    }
                    '<' => AttrVal::Expr(Expr::Jsx(Box::new(self.jsx()?))),
                    _ => return self.err("expected an attribute value"),
                };
                attrs.push(Attr::Kv(name, val));
            } else {
                attrs.push(Attr::Kv(name, AttrVal::True));
            }
        }
    }

    fn jsx_children(&mut self, tag: &str) -> R<Vec<JsxChild>> {
        let mut out = Vec::new();
        let mut text = String::new();
        loop {
            if self.p >= self.c.len() {
                return self.err(&format!("unclosed <{}>", tag));
            }
            match self.at(0) {
                '<' => {
                    flush_text(&mut text, &mut out);
                    if self.at(1) == '/' {
                        self.p += 2;
                        let name = self.jsx_name()?;
                        self.expect(">")?;
                        if name != tag {
                            return self.err(&format!("</{}> closes <{}>", name, tag));
                        }
                        return Ok(out);
                    }
                    out.push(JsxChild::Element(self.jsx()?));
                }
                '{' => {
                    flush_text(&mut text, &mut out);
                    self.p += 1;
                    self.ws();
                    if self.at(0) == '}' {
                        self.p += 1;
                        continue;
                    }
                    let e = if self.eat("...") { Expr::Spread(Box::new(self.assign()?)) } else { self.expr()? };
                    self.expect("}")?;
                    out.push(JsxChild::Expr(e));
                }
                ch => {
                    text.push(ch);
                    self.p += 1;
                }
            }
        }
    }

    // ---------------------------------------------------------------- types

    pub fn ty(&mut self) -> R<Ty> {
        self.ws();
        self.eat("|");
        let first = self.ty_inter()?;
        let mut alts = vec![first];
        while self.peek("|") && !self.starts("||") {
            self.p += 1;
            alts.push(self.ty_inter()?);
        }
        let t = if alts.len() == 1 { alts.pop().unwrap() } else { Ty::Union(alts) };
        // conditional types are read and dropped
        if self.is_kw("extends") {
            self.p += 7;
            self.ty_inter()?;
            if self.eat("?") {
                self.ty()?;
                self.expect(":")?;
                self.ty()?;
            }
            return Ok(Ty::Any);
        }
        Ok(t)
    }

    fn ty_inter(&mut self) -> R<Ty> {
        let first = self.ty_postfix()?;
        let mut parts = vec![first];
        while self.peek("&") && !self.starts("&&") {
            self.p += 1;
            parts.push(self.ty_postfix()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Ty::Inter(parts) })
    }

    fn ty_postfix(&mut self) -> R<Ty> {
        let mut t = self.ty_primary()?;
        loop {
            self.ws();
            if self.nl {
                break;
            }
            if self.starts("[]") {
                self.p += 2;
                t = Ty::Array(Box::new(t));
            } else if self.at(0) == '[' {
                // indexed access `T['k']`
                self.p += 1;
                self.ty()?;
                self.expect("]")?;
                t = Ty::Any;
            } else {
                break;
            }
        }
        Ok(t)
    }

    fn ty_primary(&mut self) -> R<Ty> {
        self.ws();
        let ch = self.at(0);
        match ch {
            '(' => {
                if let Some(t) = self.attempt(|s| s.ty_function()) {
                    return Ok(t);
                }
                self.p += 1;
                let t = self.ty()?;
                self.expect(")")?;
                Ok(t)
            }
            '{' => self.ty_object(),
            '[' => {
                self.p += 1;
                let mut items = Vec::new();
                loop {
                    if self.eat("]") {
                        break;
                    }
                    self.eat("...");
                    items.push(self.ty()?);
                    self.eat("?");
                    if !self.eat(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                Ok(Ty::Tuple(items))
            }
            '\'' | '"' => {
                let s = self.string()?;
                Ok(Ty::Lit(format!("'{}'", s)))
            }
            '`' => {
                self.template()?;
                Ok(Ty::Name("string".to_string(), Vec::new()))
            }
            '-' | '0'..='9' => {
                if ch == '-' {
                    self.p += 1;
                }
                Ok(Ty::Lit(self.number()))
            }
            '<' => {
                // generic function type `<T>(a: T) => T`
                self.type_params()?;
                self.ty_function()
            }
            _ => {
                let word = self.ident()?;
                match word.as_str() {
                    "keyof" => {
                        self.ty_postfix()?;
                        Ok(Ty::Name("string".to_string(), Vec::new()))
                    }
                    "typeof" => {
                        let mut n = self.ident()?;
                        while self.peek(".") {
                            self.p += 1;
                            n.push('.');
                            n.push_str(&self.ident()?);
                        }
                        Ok(Ty::Any)
                    }
                    "readonly" | "unique" => self.ty_postfix(),
                    "new" => {
                        self.ty_function()?;
                        Ok(Ty::Any)
                    }
                    "import" => {
                        self.expect("(")?;
                        self.string()?;
                        self.expect(")")?;
                        let mut n = String::new();
                        while self.peek(".") {
                            self.p += 1;
                            n = self.ident()?;
                        }
                        let args = self.ty_args()?;
                        Ok(Ty::Name(n, args))
                    }
                    "true" | "false" => Ok(Ty::Lit(word)),
                    "asserts" | "infer" => {
                        self.ident()?;
                        Ok(Ty::Any)
                    }
                    _ => {
                        let mut name = word;
                        while self.at(0) == '.' && Self::is_id_start(self.at(1)) {
                            self.p += 1;
                            name.push('.');
                            name.push_str(&self.ident()?);
                        }
                        let args = self.ty_args()?;
                        // `x is Foo` predicates read as boolean
                        if self.is_kw("is") && !self.nl {
                            self.p += 2;
                            self.ty()?;
                            return Ok(Ty::Name("boolean".to_string(), Vec::new()));
                        }
                        Ok(Ty::Name(name, args))
                    }
                }
            }
        }
    }

    fn ty_args(&mut self) -> R<Vec<Ty>> {
        if !(self.at(0) == '<') {
            return Ok(Vec::new());
        }
        self.p += 1;
        let mut out = Vec::new();
        loop {
            if self.eat(">") {
                return Ok(out);
            }
            out.push(self.ty()?);
            if !self.eat(",") {
                self.expect(">")?;
                return Ok(out);
            }
        }
    }

    fn ty_function(&mut self) -> R<Ty> {
        let params = self.params()?;
        self.ws();
        if !self.starts("=>") {
            return self.err("not a function type");
        }
        self.p += 2;
        let ret = self.ty()?;
        Ok(Ty::Fn(params, Box::new(ret)))
    }

    fn ty_object(&mut self) -> R<Ty> {
        self.expect("{")?;
        let mut fields = Vec::new();
        loop {
            self.ws();
            while self.at(0) == ';' || self.at(0) == ',' {
                self.p += 1;
                self.ws();
            }
            if self.eat("}") {
                return Ok(Ty::Obj(fields));
            }
            while self.eat_kw("readonly") {}
            if self.at(0) == '[' {
                // index signature `[key: string]: T`
                self.p += 1;
                self.ident()?;
                self.expect(":")?;
                let k = self.ty()?;
                self.expect("]")?;
                self.expect(":")?;
                let v = self.ty()?;
                fields.push(Field { name: "[index]".to_string(), ty: Ty::Name("Map".to_string(), vec![k, v]), optional: false });
                continue;
            }
            let name = if self.at(0) == '\'' || self.at(0) == '"' { self.string()? } else { self.ident()? };
            let optional = self.eat("?");
            if self.peek("(") || self.peek("<") {
                // method signature
                self.type_params()?;
                let params = self.params()?;
                let ret = if self.eat(":") { self.ty()? } else { Ty::Any };
                fields.push(Field { name, ty: Ty::Fn(params, Box::new(ret)), optional });
            } else {
                let ty = if self.eat(":") { self.ty()? } else { Ty::Any };
                fields.push(Field { name, ty, optional });
            }
        }
    }
}

/// JSX text rules: lines are trimmed, blank lines dropped, the rest joined with one space.
fn flush_text(text: &mut String, out: &mut Vec<JsxChild>) {
    if text.is_empty() {
        return;
    }
    let raw = std::mem::take(text);
    let lines: Vec<&str> = raw.split('\n').collect();
    let mut parts: Vec<String> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let mut l: &str = line;
        if i > 0 {
            l = l.trim_start();
        }
        if i < lines.len() - 1 {
            l = l.trim_end();
        }
        if !l.is_empty() {
            parts.push(l.to_string());
        }
    }
    // HTML shows any run of spaces as one
    let mut joined = String::new();
    for ch in parts.join(" ").chars() {
        if ch.is_whitespace() && ch != '\u{a0}' {
            if !joined.ends_with(' ') {
                joined.push(' ');
            }
        } else {
            joined.push(ch);
        }
    }
    if !joined.is_empty() {
        out.push(JsxChild::Text(decode_entities(&joined)));
    }
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", "\u{a0}").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(src: &str) -> Module {
        let m = parse_module(src);
        assert!(m.problems.is_empty(), "{:?}", m.problems);
        m
    }

    #[test]
    fn imports_and_interfaces_read() {
        let m = items("import React, { useState, type FC as F } from 'react';\nimport * as x from \"y\";\nexport interface A extends B { a: string; b?: number[]; c: 'x' | 'y' | null; f: (id: string) => void }\n");
        assert_eq!(m.items.len(), 3);
        match &m.items[0] {
            Item::Import(i) => assert_eq!((i.default.as_deref(), i.named.len(), i.from.as_str()), (Some("React"), 2, "react")),
            other => panic!("{:?}", other),
        }
        match &m.items[2] {
            Item::Interface { name, fields, extends, .. } => assert_eq!((name.as_str(), fields.len(), extends.len()), ("A", 4, 1)),
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn a_component_with_jsx_conditionals_and_maps_reads() {
        let m = items(
            "export const Home: React.FC = () => {\n  const [n, setN] = useState<number>(0);\n  return (\n    <div className=\"a b\" onClick={() => setN(n + 1)}>\n      {/* note */}\n      Hello {n > 0 && <b>{n}</b>}\n      {items.map(i => <li key={i}>{i}</li>)}\n    </div>\n  );\n};\n",
        );
        let Item::Var { decls, .. } = &m.items[0] else { panic!() };
        let Some(Expr::Arrow(f)) = &decls[0].init else { panic!() };
        let FnBody::Block(body) = &f.body else { panic!() };
        assert_eq!(body.len(), 2);
        let Stmt::Return(Some(Expr::Jsx(j))) = &body[1] else { panic!() };
        assert_eq!((j.tag.as_str(), j.attrs.len()), ("div", 2));
        // the comment is dropped; text, expression, expression remain
        assert_eq!(j.children.len(), 3, "{:?}", j.children);
        assert!(matches!(&j.children[0], JsxChild::Text(t) if t == "Hello "), "{:?}", j.children);
    }

    #[test]
    fn statements_templates_and_optional_chaining_read() {
        let m = items("async function f(a: string, b = 2): Promise<void> {\n  try { const x = await g(`${a}-${b}`); if (x?.y ?? false) { return; } } catch (e: any) { console.error(e.message); } finally { done(); }\n  for (const k of ks) { total += k.n * 2; }\n  switch (a) { case 'x': break; default: return; }\n}\n");
        let Item::Func { func, .. } = &m.items[0] else { panic!() };
        assert!(func.is_async);
        assert_eq!(func.params.len(), 2);
    }

    #[test]
    fn a_class_component_reads() {
        let m = items("export class Boundary extends Component<Props, State> {\n  state: State = { hasError: false };\n  static getDerivedStateFromError(error: Error): State { return { hasError: true, error }; }\n  handleRetry = () => { this.setState({ hasError: false }); };\n  render() { return <div>{this.props.children}</div>; }\n}\n");
        let Item::Class { name, members, .. } = &m.items[0] else { panic!() };
        assert_eq!((name.as_str(), members.len()), ("Boundary", 4));
    }

    #[test]
    fn jsx_text_whitespace_follows_react() {
        let m = items("const a = <p>\n   one\n   two   <b>x</b>\n</p>;");
        let Item::Var { decls, .. } = &m.items[0] else { panic!() };
        let Some(Expr::Jsx(j)) = &decls[0].init else { panic!() };
        assert!(matches!(&j.children[0], JsxChild::Text(t) if t == "one two "), "{:?}", j.children);
    }
}
