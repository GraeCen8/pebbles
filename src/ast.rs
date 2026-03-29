#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Ident(String),

    // Keywords
    Let,
    Mut,
    Fn,
    Return,
    If,
    Else,
    While,
    For,
    In,
    Match,
    Struct,
    Impl,
    As,
    True,
    False,
    None_,

    // Symbols
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Eq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
    Bang,
    Assign,
    PlusAssign,
    MinusAssign,
    Arrow,    // ->
    FatArrow, // =>
    DotDot,   // ..
    DotDotEq, // ..=
    Dot,
    Comma,
    Colon,
    Semicolon,
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Question, // ?

    EOF,
}

// ast.rs — Pebble AST node definitions
// Every node carries a `line` field for error reporting.

// ─── Types ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    I32,
    I64,
    F64,
    Bool,
    Str,
    Void,
    SelfType,
    Array(Box<Type>),
    Optional(Box<Type>),
    Named(String), // user-defined struct name
}

// ─── Top-level items ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDef),
    Struct(StructDef),
    Impl(ImplBlock),
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Type,
    pub body: Vec<Stmt>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<StructField>,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct ImplBlock {
    pub type_name: String,
    pub methods: Vec<FnDef>,
    pub line: usize,
}

// ─── Statements ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Stmt {
    /// let [mut] name [: Type] = expr
    Let {
        name: String,
        ty: Option<Type>,
        value: Expr,
        mutable: bool,
        line: usize,
    },
    /// target = expr  (also desugared from +=, -=, etc.)
    Assign {
        target: AssignTarget,
        value: Expr,
        line: usize,
    },
    /// return [expr]
    Return {
        value: Option<Expr>,
        line: usize,
    },
    /// while cond { body }
    While {
        cond: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    /// for var in iter { body }
    For {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
        line: usize,
    },
    Break    { line: usize },
    Continue { line: usize },
    /// A bare expression used as a statement (function calls, if-else, etc.)
    Expr(Expr),
}

/// What can appear on the left-hand side of an assignment.
#[derive(Debug, Clone)]
pub enum AssignTarget {
    Ident(String, usize),
    Field { obj: Box<Expr>, field: String, line: usize },
    Index { obj: Box<Expr>, index: Box<Expr>, line: usize },
}

// ─── Expressions ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum Expr {
    // Literals
    Int(i64, usize),
    Float(f64, usize),
    Bool(bool, usize),
    Str(String, usize),
    None(usize),

    // Collections
    Array(Vec<Expr>, usize),
    Tuple(Vec<Expr>, usize),

    // Range:  start..end  or  start..=end
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        inclusive: bool,
        line: usize,
    },

    // Names and access
    Ident(String, usize),
    FieldAccess { obj: Box<Expr>, field: String, line: usize },
    Index       { obj: Box<Expr>, index: Box<Expr>, line: usize },

    // Calls
    Call       { name: String, args: Vec<Expr>, line: usize },
    MethodCall { obj: Box<Expr>, method: String, args: Vec<Expr>, line: usize },

    // Struct literal:  Name { field: val, ... }
    StructLit  { name: String, fields: Vec<(String, Expr)>, line: usize },

    // Operators
    BinOp  { op: BinOp,   left: Box<Expr>, right: Box<Expr>, line: usize },
    UnaryOp { op: UnaryOp, operand: Box<Expr>, line: usize },

    // Type cast:  expr as Type
    Cast { expr: Box<Expr>, ty: Type, line: usize },

    // Control flow as expressions
    If    { cond: Box<Expr>, then: Vec<Stmt>, else_: Option<Vec<Stmt>>, line: usize },
    Match { subject: Box<Expr>, arms: Vec<MatchArm>, line: usize },
}

impl Expr {
    pub fn line(&self) -> usize {
        match self {
            Expr::Int(_, l) | Expr::Float(_, l) | Expr::Bool(_, l)
            | Expr::Str(_, l) | Expr::None(l) | Expr::Array(_, l)
            | Expr::Tuple(_, l) | Expr::Ident(_, l) => *l,
            Expr::BinOp  { line, .. } | Expr::UnaryOp { line, .. }
            | Expr::Call { line, .. } | Expr::MethodCall { line, .. }
            | Expr::FieldAccess { line, .. } | Expr::Index { line, .. }
            | Expr::StructLit { line, .. } | Expr::Cast { line, .. }
            | Expr::If { line, .. } | Expr::Match { line, .. }
            | Expr::Range { line, .. } => *line,
        }
    }
}

// ─── Binary operators ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, NotEq,
    Lt, Gt, LtEq, GtEq,
    And, Or,
}

// ─── Unary operators ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum UnaryOp {
    Neg, // -x
    Not, // !x
}

// ─── Match arms and patterns ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Pattern {
    Wildcard,               // _
    Int(i64),
    Float(f64),
    Bool(bool),
    Str(String),
    None,
    Binding(String),        // variable name — captures the value
    Tuple(Vec<Pattern>),    // (pat, pat)
    Struct {                // Name { field: pat, ... }
        name: String,
        fields: Vec<(String, Pattern)>,
    },
}
