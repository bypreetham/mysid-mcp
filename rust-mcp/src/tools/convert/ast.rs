//! Syntax tree of the TypeScript / TSX subset the converter reads (what `parse.rs` produces).
#![allow(dead_code)]

pub type Ex = Box<Expr>;

/// `useState<Foo>(x)` reads as `As(Call, __targs<Foo>)`: the call and its type arguments.
pub fn unwrap_targs(e: &Expr) -> (&Expr, &[Ty]) {
    if let Expr::As(inner, Ty::Name(n, args)) = e {
        if n == "__targs" {
            return (inner, args);
        }
    }
    (e, &[])
}

#[derive(Clone, Debug)]
pub enum Expr {
    Ident(String),
    Num(String),
    Str(String),
    Bool(bool),
    Null,
    Undefined,
    This,
    /// `quasis.len() == exprs.len() + 1`; quasis are raw source text (escapes not decoded).
    Template(Vec<String>, Vec<Expr>),
    Array(Vec<Expr>),
    Object(Vec<Prop>),
    /// object, property, optional (`?.`)
    Member(Ex, String, bool),
    Index(Ex, Ex, bool),
    Call(Ex, Vec<Expr>, bool),
    New(Ex, Vec<Expr>),
    Unary(String, Ex),
    /// `++x` / `x--`: operator, operand, prefix
    Update(String, Ex, bool),
    Binary(String, Ex, Ex),
    Assign(String, Ex, Ex),
    Cond(Ex, Ex, Ex),
    Arrow(Box<Func>),
    Await(Ex),
    Spread(Ex),
    NonNull(Ex),
    As(Ex, Ty),
    Jsx(Box<Jsx>),
    Regex(String),
}

#[derive(Clone, Debug)]
pub enum Prop {
    /// key (already a plain name or a computed expression), value
    Kv(PropKey, Expr),
    Short(String),
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub enum PropKey {
    Name(String),
    Computed(Expr),
}

#[derive(Clone, Debug)]
pub struct Func {
    pub name: Option<String>,
    pub params: Vec<Param>,
    pub ret: Option<Ty>,
    pub body: FnBody,
    pub is_async: bool,
}

#[derive(Clone, Debug)]
pub enum FnBody {
    Expr(Box<Expr>),
    Block(Vec<Stmt>),
}

#[derive(Clone, Debug)]
pub struct Param {
    pub pat: Pat,
    pub ty: Option<Ty>,
    pub default: Option<Expr>,
    pub optional: bool,
    pub rest: bool,
}

#[derive(Clone, Debug)]
pub enum Pat {
    Ident(String),
    Object(Vec<PatProp>),
    Array(Vec<Option<Pat>>),
}

#[derive(Clone, Debug)]
pub struct PatProp {
    pub key: String,
    pub value: Pat,
    pub default: Option<Expr>,
    pub rest: bool,
}

#[derive(Clone, Debug)]
pub struct VarDecl {
    pub pat: Pat,
    pub ty: Option<Ty>,
    pub init: Option<Expr>,
}

#[derive(Clone, Debug)]
pub struct CatchClause {
    pub param: Option<Pat>,
    pub body: Vec<Stmt>,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    Var(String, Vec<VarDecl>),
    Expr(Expr),
    Return(Option<Expr>),
    If(Expr, Box<Stmt>, Option<Box<Stmt>>),
    Block(Vec<Stmt>),
    For(Option<Box<Stmt>>, Option<Expr>, Option<Expr>, Box<Stmt>),
    /// kind (const/let), pattern, iterable, body, is `for ... in`
    ForOf(String, Pat, Expr, Box<Stmt>, bool),
    While(Expr, Box<Stmt>),
    DoWhile(Box<Stmt>, Expr),
    Try(Vec<Stmt>, Option<CatchClause>, Option<Vec<Stmt>>),
    Throw(Expr),
    Switch(Expr, Vec<(Option<Expr>, Vec<Stmt>)>),
    Break,
    Continue,
    Func(Func),
    Empty,
}

#[derive(Clone, Debug)]
pub enum Ty {
    Name(String, Vec<Ty>),
    Union(Vec<Ty>),
    Inter(Vec<Ty>),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    /// a literal type: the raw text (`'admin'`, `3`, `true`)
    Lit(String),
    Obj(Vec<Field>),
    Fn(Vec<Param>, Box<Ty>),
    Any,
}

#[derive(Clone, Debug)]
pub struct Field {
    pub name: String,
    pub ty: Ty,
    pub optional: bool,
}

#[derive(Clone, Debug)]
pub struct Jsx {
    /// "" for a fragment
    pub tag: String,
    pub attrs: Vec<Attr>,
    pub children: Vec<JsxChild>,
}

#[derive(Clone, Debug)]
pub enum Attr {
    Kv(String, AttrVal),
    Spread(Expr),
}

#[derive(Clone, Debug)]
pub enum AttrVal {
    Str(String),
    Expr(Expr),
    True,
}

#[derive(Clone, Debug)]
pub enum JsxChild {
    Text(String),
    Expr(Expr),
    Element(Jsx),
}

#[derive(Clone, Debug)]
pub struct ClassMember {
    pub name: String,
    pub is_static: bool,
    pub kind: MemberKind,
}

#[derive(Clone, Debug)]
pub enum MemberKind {
    Method(Func),
    Field(Option<Ty>, Option<Expr>),
}

#[derive(Clone, Debug)]
pub struct Import {
    pub default: Option<String>,
    pub named: Vec<(String, String)>,
    pub namespace: Option<String>,
    pub from: String,
}

#[derive(Clone, Debug)]
pub enum Item {
    Import(Import),
    Interface { name: String, extends: Vec<Ty>, fields: Vec<Field>, exported: bool },
    TypeAlias { name: String, ty: Ty, exported: bool },
    Enum { name: String, members: Vec<(String, Option<Expr>)>, exported: bool },
    Var { kind: String, decls: Vec<VarDecl>, exported: bool },
    Func { func: Func, exported: bool, default: bool },
    Class { name: String, extends: Option<Ty>, members: Vec<ClassMember>, exported: bool, default: bool },
    ExportDefault(Expr),
    /// `export { a, b as c }` or `export * from`
    ExportNames(Vec<(String, String)>),
    Stmt(Stmt),
}

#[derive(Clone, Debug, Default)]
pub struct Module {
    pub items: Vec<Item>,
    /// problems met while reading: (line, message); the items before the problem are still here
    pub problems: Vec<(usize, String)>,
}
