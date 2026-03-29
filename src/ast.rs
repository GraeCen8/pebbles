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
