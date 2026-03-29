// parser.rs — Complete Pebble Language Parser
// Turns a flat Vec<Token> into a typed AST.
// Entry point: Parser::new(tokens).parse() -> Vec<Item>

use crate::ast::*;
use crate::lexer::Token;

// ─── Error type ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct ParseError {
    pub message: String,
    pub line: usize,
}

impl ParseError {
    fn new(msg: impl Into<String>, line: usize) -> Self {
        Self { message: msg.into(), line }
    }
}

pub type ParseResult<T> = Result<T, ParseError>;

// ─── Parser struct ────────────────────────────────────────────────────────────

pub struct Parser {
    tokens: Vec<(Token, usize)>, // (token, line_number)
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<(Token, usize)>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ── Core helpers ──────────────────────────────────────────────────────────

    fn peek(&self) -> &Token {
        &self.tokens[self.pos].0
    }

    fn peek2(&self) -> &Token {
        if self.pos + 1 < self.tokens.len() {
            &self.tokens[self.pos + 1].0
        } else {
            &Token::EOF
        }
    }

    fn line(&self) -> usize {
        self.tokens[self.pos].1
    }

    fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos].0;
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        tok
    }

    fn expect(&mut self, expected: &Token) -> ParseResult<()> {
        if self.peek() == expected {
            self.advance();
            Ok(())
        } else {
            Err(ParseError::new(
                format!("expected {:?}, found {:?}", expected, self.peek()),
                self.line(),
            ))
        }
    }

    fn expect_ident(&mut self) -> ParseResult<String> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.advance();
                Ok(name)
            }
            other => Err(ParseError::new(
                format!("expected identifier, found {:?}", other),
                self.line(),
            )),
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.peek(), Token::EOF)
    }

    // Skip optional semicolons/newlines used as statement terminators
    fn skip_semis(&mut self) {
        while matches!(self.peek(), Token::Semicolon) {
            self.advance();
        }
    }

    // ── Top-level parse ───────────────────────────────────────────────────────

    /// Parse a whole file. Returns a list of top-level items.
    pub fn parse(&mut self) -> ParseResult<Vec<Item>> {
        let mut items = vec![];
        self.skip_semis();
        while !self.at_end() {
            items.push(self.parse_item()?);
            self.skip_semis();
        }
        Ok(items)
    }

    fn parse_item(&mut self) -> ParseResult<Item> {
        match self.peek() {
            Token::Fn     => Ok(Item::Fn(self.parse_fn()?)),
            Token::Struct => Ok(Item::Struct(self.parse_struct()?)),
            Token::Impl   => Ok(Item::Impl(self.parse_impl()?)),
            other => Err(ParseError::new(
                format!("expected top-level item (fn, struct, impl), found {:?}", other),
                self.line(),
            )),
        }
    }

    // ── Function definition ───────────────────────────────────────────────────

    fn parse_fn(&mut self) -> ParseResult<FnDef> {
        let line = self.line();
        self.expect(&Token::Fn)?;
        let name = self.expect_ident()?;

        self.expect(&Token::LParen)?;
        let params = self.parse_param_list()?;
        self.expect(&Token::RParen)?;

        let ret = if matches!(self.peek(), Token::Arrow) {
            self.advance();
            self.parse_type()?
        } else {
            Type::Void
        };

        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;

        Ok(FnDef { name, params, ret, body, line })
    }

    fn parse_param_list(&mut self) -> ParseResult<Vec<Param>> {
        let mut params = vec![];

        // handle 'self' as first param for methods
        if matches!(self.peek(), Token::Self_) {
            params.push(Param { name: "self".into(), ty: Type::SelfType, mutable: false });
            self.advance();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        } else if matches!(self.peek(), Token::Mut) && matches!(self.peek2(), Token::Self_) {
            self.advance(); // mut
            self.advance(); // self
            params.push(Param { name: "self".into(), ty: Type::SelfType, mutable: true });
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }

        while !matches!(self.peek(), Token::RParen | Token::EOF) {
            let mutable = if matches!(self.peek(), Token::Mut) {
                self.advance();
                true
            } else {
                false
            };
            let name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type()?;
            params.push(Param { name, ty, mutable });

            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }

        Ok(params)
    }

    // ── Struct definition ─────────────────────────────────────────────────────

    fn parse_struct(&mut self) -> ParseResult<StructDef> {
        let line = self.line();
        self.expect(&Token::Struct)?;
        let name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;

        let mut fields = vec![];
        while !matches!(self.peek(), Token::RBrace | Token::EOF) {
            self.skip_semis();
            if matches!(self.peek(), Token::RBrace) { break; }
            let field_name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let ty = self.parse_type()?;
            fields.push(StructField { name: field_name, ty });
            self.skip_semis();
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
        }

        self.expect(&Token::RBrace)?;
        Ok(StructDef { name, fields, line })
    }

    // ── Impl block ────────────────────────────────────────────────────────────

    fn parse_impl(&mut self) -> ParseResult<ImplBlock> {
        let line = self.line();
        self.expect(&Token::Impl)?;
        let type_name = self.expect_ident()?;
        self.expect(&Token::LBrace)?;

        let mut methods = vec![];
        self.skip_semis();
        while !matches!(self.peek(), Token::RBrace | Token::EOF) {
            methods.push(self.parse_fn()?);
            self.skip_semis();
        }

        self.expect(&Token::RBrace)?;
        Ok(ImplBlock { type_name, methods, line })
    }

    // ── Types ─────────────────────────────────────────────────────────────────

    fn parse_type(&mut self) -> ParseResult<Type> {
        // Optional type: ?Type
        if matches!(self.peek(), Token::Question) {
            self.advance();
            let inner = self.parse_type()?;
            return Ok(Type::Optional(Box::new(inner)));
        }

        // Array type: [Type]
        if matches!(self.peek(), Token::LBracket) {
            self.advance();
            let inner = self.parse_type()?;
            self.expect(&Token::RBracket)?;
            return Ok(Type::Array(Box::new(inner)));
        }

        let ty = match self.peek().clone() {
            Token::Ident(name) => match name.as_str() {
                "i32"  => Type::I32,
                "i64"  => Type::I64,
                "f64"  => Type::F64,
                "bool" => Type::Bool,
                "str"  => Type::Str,
                "void" => Type::Void,
                _      => Type::Named(name),
            },
            other => {
                return Err(ParseError::new(
                    format!("expected type, found {:?}", other),
                    self.line(),
                ))
            }
        };
        self.advance();
        Ok(ty)
    }

    // ── Block (list of statements) ────────────────────────────────────────────

    fn parse_block(&mut self) -> ParseResult<Vec<Stmt>> {
        let mut stmts = vec![];
        self.skip_semis();
        while !matches!(self.peek(), Token::RBrace | Token::EOF) {
            stmts.push(self.parse_stmt()?);
            self.skip_semis();
        }
        Ok(stmts)
    }

    // ── Statements ────────────────────────────────────────────────────────────

    fn parse_stmt(&mut self) -> ParseResult<Stmt> {
        let line = self.line();
        match self.peek().clone() {
            Token::Let    => self.parse_let(),
            Token::Return => self.parse_return(),
            Token::If     => Ok(Stmt::Expr(self.parse_if()?)),
            Token::While  => self.parse_while(),
            Token::For    => self.parse_for(),
            Token::Break  => { self.advance(); Ok(Stmt::Break { line }) }
            Token::Continue => { self.advance(); Ok(Stmt::Continue { line }) }

            // Could be: assignment (x = ...), augmented assignment (x += ...),
            // or an expression statement (fn call, field access, etc.)
            Token::Ident(_) => {
                let expr = self.parse_expr()?;

                // Check for assignment or augmented assignment
                match self.peek().clone() {
                    Token::Assign => {
                        self.advance();
                        let value = self.parse_expr()?;
                        let target = expr_to_assign_target(expr)
                            .ok_or_else(|| ParseError::new("invalid assignment target", line))?;
                        Ok(Stmt::Assign { target, value, line })
                    }
                    Token::PlusAssign  => self.parse_aug_assign(expr, BinOp::Add, line),
                    Token::MinusAssign => self.parse_aug_assign(expr, BinOp::Sub, line),
                    Token::StarAssign  => self.parse_aug_assign(expr, BinOp::Mul, line),
                    Token::SlashAssign => self.parse_aug_assign(expr, BinOp::Div, line),
                    _ => Ok(Stmt::Expr(expr)),
                }
            }

            _ => Ok(Stmt::Expr(self.parse_expr()?)),
        }
    }

    fn parse_let(&mut self) -> ParseResult<Stmt> {
        let line = self.line();
        self.expect(&Token::Let)?;

        let mutable = if matches!(self.peek(), Token::Mut) {
            self.advance();
            true
        } else {
            false
        };

        let name = self.expect_ident()?;

        let ty = if matches!(self.peek(), Token::Colon) {
            self.advance();
            Some(self.parse_type()?)
        } else {
            None
        };

        self.expect(&Token::Assign)?;
        let value = self.parse_expr()?;

        Ok(Stmt::Let { name, ty, value, mutable, line })
    }

    fn parse_return(&mut self) -> ParseResult<Stmt> {
        let line = self.line();
        self.expect(&Token::Return)?;

        // return with no value (next token is } or ;)
        let value = if matches!(self.peek(), Token::RBrace | Token::Semicolon | Token::EOF) {
            None
        } else {
            Some(self.parse_expr()?)
        };

        Ok(Stmt::Return { value, line })
    }

    fn parse_while(&mut self) -> ParseResult<Stmt> {
        let line = self.line();
        self.expect(&Token::While)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::While { cond, body, line })
    }

    fn parse_for(&mut self) -> ParseResult<Stmt> {
        let line = self.line();
        self.expect(&Token::For)?;
        let var = self.expect_ident()?;
        self.expect(&Token::In)?;
        let iter = self.parse_expr()?;
        self.expect(&Token::LBrace)?;
        let body = self.parse_block()?;
        self.expect(&Token::RBrace)?;
        Ok(Stmt::For { var, iter, body, line })
    }

    fn parse_aug_assign(&mut self, target_expr: Expr, op: BinOp, line: usize) -> ParseResult<Stmt> {
        self.advance(); // consume +=, -=, etc.
        let rhs = self.parse_expr()?;
        let target = expr_to_assign_target(target_expr.clone())
            .ok_or_else(|| ParseError::new("invalid assignment target", line))?;
        // Desugar: x += y  =>  x = x + y
        let value = Expr::BinOp {
            op,
            left: Box::new(target_expr),
            right: Box::new(rhs),
            line,
        };
        Ok(Stmt::Assign { target, value, line })
    }

    // ── Expressions (Pratt-style precedence climbing) ─────────────────────────
    //
    // Precedence levels (low → high):
    //   1. ||
    //   2. &&
    //   3. == !=
    //   4. < > <= >=
    //   5. + -
    //   6. * / %
    //   7. unary: ! -
    //   8. postfix: call, field access, index, as-cast

    fn parse_expr(&mut self) -> ParseResult<Expr> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinOp { op: BinOp::Or, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_equality()?;
        while matches!(self.peek(), Token::And) {
            self.advance();
            let right = self.parse_equality()?;
            left = Expr::BinOp { op: BinOp::And, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_equality(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_comparison()?;
        loop {
            let op = match self.peek() {
                Token::Eq    => BinOp::Eq,
                Token::NotEq => BinOp::NotEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_comparison(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_additive()?;
        loop {
            let op = match self.peek() {
                Token::Lt   => BinOp::Lt,
                Token::Gt   => BinOp::Gt,
                Token::LtEq => BinOp::LtEq,
                Token::GtEq => BinOp::GtEq,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_additive(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_multiplicative()?;
        loop {
            let op = match self.peek() {
                Token::Plus  => BinOp::Add,
                Token::Minus => BinOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_multiplicative(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut left = self.parse_unary()?;
        loop {
            let op = match self.peek() {
                Token::Star    => BinOp::Mul,
                Token::Slash   => BinOp::Div,
                Token::Percent => BinOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_unary()?;
            left = Expr::BinOp { op, left: Box::new(left), right: Box::new(right), line };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        match self.peek().clone() {
            Token::Bang => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::Not, operand: Box::new(operand), line })
            }
            Token::Minus => {
                self.advance();
                let operand = self.parse_unary()?;
                Ok(Expr::UnaryOp { op: UnaryOp::Neg, operand: Box::new(operand), line })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        let mut expr = self.parse_primary()?;

        loop {
            match self.peek().clone() {
                // Field access: expr.field  OR  method call: expr.method(args)
                Token::Dot => {
                    self.advance();
                    let field = self.expect_ident()?;
                    if matches!(self.peek(), Token::LParen) {
                        self.advance();
                        let args = self.parse_arg_list()?;
                        self.expect(&Token::RParen)?;
                        expr = Expr::MethodCall {
                            obj: Box::new(expr),
                            method: field,
                            args,
                            line,
                        };
                    } else {
                        expr = Expr::FieldAccess {
                            obj: Box::new(expr),
                            field,
                            line,
                        };
                    }
                }

                // Index access: expr[index]
                Token::LBracket => {
                    self.advance();
                    let index = self.parse_expr()?;
                    self.expect(&Token::RBracket)?;
                    expr = Expr::Index { obj: Box::new(expr), index: Box::new(index), line };
                }

                // Type cast: expr as Type
                Token::As => {
                    self.advance();
                    let ty = self.parse_type()?;
                    expr = Expr::Cast { expr: Box::new(expr), ty, line };
                }

                _ => break,
            }
        }

        Ok(expr)
    }

    // ── Primary expressions ───────────────────────────────────────────────────

    fn parse_primary(&mut self) -> ParseResult<Expr> {
        let line = self.line();

        match self.peek().clone() {
            // Literals
            Token::Int(n) => {
                self.advance();
                match self.peek().clone() {
                    Token::DotDot => {
                        self.advance();
                        let end = self.parse_expr()?;
                        Ok(Expr::Range {
                            start: Box::new(Expr::Int(n, line)),
                            end: Box::new(end),
                            inclusive: false,
                            line,
                        })
                    }
                    Token::DotDotEq => {
                        self.advance();
                        let end = self.parse_expr()?;
                        Ok(Expr::Range {
                            start: Box::new(Expr::Int(n, line)),
                            end: Box::new(end),
                            inclusive: true,
                            line,
                        })
                    }
                    _ => Ok(Expr::Int(n, line)),
                }
            }
            Token::Float(f) => {
                self.advance();
                Ok(Expr::Float(f, line))
            }
            Token::Bool(b) => {
                self.advance();
                Ok(Expr::Bool(b, line))
            }
            Token::StringLit(s) => {
                self.advance();
                Ok(Expr::Str(s, line))
            }
            Token::None_ => {
                self.advance();
                Ok(Expr::None(line))
            }

            // Grouped expression: (expr)
            Token::LParen => {
                self.advance();
                // Tuple match support: (a, b) used in match
                if matches!(self.peek(), Token::RParen) {
                    self.advance();
                    return Ok(Expr::Tuple(vec![], line));
                }
                let first = self.parse_expr()?;
                if matches!(self.peek(), Token::Comma) {
                    // it's a tuple
                    let mut elems = vec![first];
                    while matches!(self.peek(), Token::Comma) {
                        self.advance();
                        if matches!(self.peek(), Token::RParen) { break; }
                        elems.push(self.parse_expr()?);
                    }
                    self.expect(&Token::RParen)?;
                    Ok(Expr::Tuple(elems, line))
                } else {
                    self.expect(&Token::RParen)?;
                    Ok(first)
                }
            }

            // Array literal: [expr, expr, ...]
            Token::LBracket => {
                self.advance();
                let mut elems = vec![];
                while !matches!(self.peek(), Token::RBracket | Token::EOF) {
                    elems.push(self.parse_expr()?);
                    if matches!(self.peek(), Token::Comma) {
                        self.advance();
                    } else {
                        break;
                    }
                }
                self.expect(&Token::RBracket)?;
                Ok(Expr::Array(elems, line))
            }

            // If expression (used inline, e.g. let x = if cond { 1 } else { 2 })
            Token::If => self.parse_if(),

            // Match expression
            Token::Match => self.parse_match(),

            // Identifier: variable, function call, or struct literal
            Token::Ident(name) => {
                self.advance();

                match self.peek().clone() {
                    // Function call: name(args)
                    Token::LParen => {
                        self.advance();
                        let args = self.parse_arg_list()?;
                        self.expect(&Token::RParen)?;
                        Ok(Expr::Call { name, args, line })
                    }

                    // Struct literal: Name { field: val, ... }
                    // Only when the next two tokens are: { ident :
                    // (to avoid confusion with block statements)
                    Token::LBrace if self.is_struct_literal() => {
                        self.advance();
                        let fields = self.parse_struct_literal_fields()?;
                        self.expect(&Token::RBrace)?;
                        Ok(Expr::StructLit { name, fields, line })
                    }

                    // Range: name..expr or name..=expr
                    Token::DotDot => {
                        self.advance();
                        let end = self.parse_expr()?;
                        Ok(Expr::Range {
                            start: Box::new(Expr::Ident(name, line)),
                            end: Box::new(end),
                            inclusive: false,
                            line,
                        })
                    }
                    Token::DotDotEq => {
                        self.advance();
                        let end = self.parse_expr()?;
                        Ok(Expr::Range {
                            start: Box::new(Expr::Ident(name, line)),
                            end: Box::new(end),
                            inclusive: true,
                            line,
                        })
                    }

                    _ => Ok(Expr::Ident(name, line)),
                }
            }

            other => Err(ParseError::new(
                format!("unexpected token in expression: {:?}", other),
                line,
            )),
        }
    }

    // ── If expression ─────────────────────────────────────────────────────────

    fn parse_if(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        self.expect(&Token::If)?;
        let cond = self.parse_expr()?;
        self.expect(&Token::LBrace)?;
        let then = self.parse_block()?;
        self.expect(&Token::RBrace)?;

        let else_ = if matches!(self.peek(), Token::Else) {
            self.advance();
            if matches!(self.peek(), Token::If) {
                // else if chain
                Some(vec![Stmt::Expr(self.parse_if()?)])
            } else {
                self.expect(&Token::LBrace)?;
                let block = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                Some(block)
            }
        } else {
            None
        };

        Ok(Expr::If { cond: Box::new(cond), then, else_, line })
    }

    // ── Match expression ──────────────────────────────────────────────────────

    fn parse_match(&mut self) -> ParseResult<Expr> {
        let line = self.line();
        self.expect(&Token::Match)?;
        let subject = self.parse_expr()?;
        self.expect(&Token::LBrace)?;

        let mut arms = vec![];
        self.skip_semis();
        while !matches!(self.peek(), Token::RBrace | Token::EOF) {
            let pattern = self.parse_pattern()?;
            self.expect(&Token::FatArrow)?;

            // Arm body: either a block or a single expression
            let body = if matches!(self.peek(), Token::LBrace) {
                self.advance();
                let stmts = self.parse_block()?;
                self.expect(&Token::RBrace)?;
                stmts
            } else {
                vec![Stmt::Expr(self.parse_expr()?)]
            };

            arms.push(MatchArm { pattern, body });

            // optional comma between arms
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            }
            self.skip_semis();
        }

        self.expect(&Token::RBrace)?;
        Ok(Expr::Match { subject: Box::new(subject), arms, line })
    }

    // ── Patterns (used in match arms) ────────────────────────────────────────

    fn parse_pattern(&mut self) -> ParseResult<Pattern> {
        let line = self.line();
        match self.peek().clone() {
            // Wildcard _
            Token::Underscore => {
                self.advance();
                Ok(Pattern::Wildcard)
            }

            // Literal patterns
            Token::Int(n) => { self.advance(); Ok(Pattern::Int(n)) }
            Token::Float(f) => { self.advance(); Ok(Pattern::Float(f)) }
            Token::Bool(b) => { self.advance(); Ok(Pattern::Bool(b)) }
            Token::StringLit(s) => { self.advance(); Ok(Pattern::Str(s)) }
            Token::None_ => { self.advance(); Ok(Pattern::None) }

            // Tuple pattern: (pat, pat)
            Token::LParen => {
                self.advance();
                let mut pats = vec![];
                while !matches!(self.peek(), Token::RParen | Token::EOF) {
                    pats.push(self.parse_pattern()?);
                    if matches!(self.peek(), Token::Comma) { self.advance(); }
                }
                self.expect(&Token::RParen)?;
                Ok(Pattern::Tuple(pats))
            }

            // Named: could be a binding variable or a struct pattern
            Token::Ident(name) => {
                self.advance();
                if matches!(self.peek(), Token::LBrace) {
                    // Struct pattern: Name { field, ... }
                    self.advance();
                    let mut fields = vec![];
                    while !matches!(self.peek(), Token::RBrace | Token::EOF) {
                        let fname = self.expect_ident()?;
                        let pat = if matches!(self.peek(), Token::Colon) {
                            self.advance();
                            self.parse_pattern()?
                        } else {
                            Pattern::Binding(fname.clone()) // shorthand
                        };
                        fields.push((fname, pat));
                        if matches!(self.peek(), Token::Comma) { self.advance(); }
                    }
                    self.expect(&Token::RBrace)?;
                    Ok(Pattern::Struct { name, fields })
                } else {
                    Ok(Pattern::Binding(name))
                }
            }

            other => Err(ParseError::new(
                format!("expected pattern, found {:?}", other),
                line,
            )),
        }
    }

    // ── Argument list ─────────────────────────────────────────────────────────

    fn parse_arg_list(&mut self) -> ParseResult<Vec<Expr>> {
        let mut args = vec![];
        while !matches!(self.peek(), Token::RParen | Token::EOF) {
            args.push(self.parse_expr()?);
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(args)
    }

    // ── Struct literal fields: { x: 1, y: 2 } ────────────────────────────────

    fn parse_struct_literal_fields(&mut self) -> ParseResult<Vec<(String, Expr)>> {
        let mut fields = vec![];
        while !matches!(self.peek(), Token::RBrace | Token::EOF) {
            let name = self.expect_ident()?;
            self.expect(&Token::Colon)?;
            let value = self.parse_expr()?;
            fields.push((name, value));
            if matches!(self.peek(), Token::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        Ok(fields)
    }

    // ── Heuristic: is the next { starting a struct literal or a block? ────────
    //
    // We look ahead: if after { we see  ident :  it's a struct literal.
    // If we see anything else (a statement keyword, an expression, }) it's a block.
    fn is_struct_literal(&self) -> bool {
        // pos is currently AT the LBrace
        if self.pos + 2 >= self.tokens.len() { return false; }
        let after_brace = &self.tokens[self.pos + 1].0;
        let after_ident = &self.tokens[self.pos + 2].0;
        matches!(after_brace, Token::Ident(_)) && matches!(after_ident, Token::Colon)
    }
}

// ── Helper: convert Expr to an assignment target ──────────────────────────────
//
// Only identifiers and field accesses can be assigned to.
// x = 1          ✓  =>  AssignTarget::Ident("x")
// obj.field = 1  ✓  =>  AssignTarget::Field(...)
// 42 = 1         ✗  =>  None

fn expr_to_assign_target(expr: Expr) -> Option<AssignTarget> {
    match expr {
        Expr::Ident(name, line)  => Some(AssignTarget::Ident(name, line)),
        Expr::FieldAccess { obj, field, line } => Some(AssignTarget::Field {
            obj: Box::new(*obj),
            field,
            line,
        }),
        Expr::Index { obj, index, line } => Some(AssignTarget::Index {
            obj: Box::new(*obj),
            index: Box::new(*index),
            line,
        }),
        _ => None,
    }
}
