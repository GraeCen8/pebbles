use crate::ast::Token as Tok;

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            input: src.chars().collect(),
            pos: 0,
        }
    }

    pub fn tokenize(&mut self) -> Vec<Tok> {
        let mut tokens: Vec<Tok> = vec![];
        loop {
            let tok = self.next_token();
            let done = tok == Tok::EOF;
            tokens.push(tok);
            if done {
                break;
            }
        }
        tokens
    }

    fn peek(&self) -> Option<char> {
        self.input.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.input.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> char {
        let c = self.input[self.pos];
        self.pos += 1;
        c
    }

    fn next_token(&mut self) -> Tok {
        //skip whitespace
        while self.peek().map_or(false, |c| c.is_whitespace()) {
            self.advance();
        }

        match self.peek() {
            None => Tok::EOF,
            Some(c) => match c {
                '+' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Tok::PlusAssign
                    } else {
                        Tok::Plus
                    }
                }
                '-' => {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        Tok::Arrow
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Tok::MinusAssign
                    } else {
                        Tok::Minus
                    }
                }
                '*' => {
                    self.advance();
                    Tok::Star
                }
                '/' => {
                    self.advance();
                    if self.peek() == Some('/') {
                        while self.peek().map_or(false, |ch| ch != '\n') {
                            self.advance();
                        }
                        self.next_token()
                    } else {
                        Tok::Slash
                    }
                }
                '%' => {
                    self.advance();
                    Tok::Percent
                }
                '=' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Tok::Eq
                    } else if self.peek() == Some('>') {
                        self.advance();
                        Tok::FatArrow
                    } else {
                        Tok::Assign
                    }
                }
                '!' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Tok::NotEq
                    } else {
                        Tok::Bang
                    }
                }
                '<' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Tok::LtEq
                    } else {
                        Tok::Lt
                    }
                }
                '>' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        Tok::GtEq
                    } else {
                        Tok::Gt
                    }
                }
                '&' => {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance();
                        Tok::And
                    } else {
                        self.next_token()
                    }
                }
                '|' => {
                    self.advance();
                    if self.peek() == Some('|') {
                        self.advance();
                        Tok::Or
                    } else {
                        self.next_token()
                    }
                }
                '.' => {
                    self.advance();
                    if self.peek() == Some('.') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            Tok::DotDotEq
                        } else {
                            Tok::DotDot
                        }
                    } else {
                        Tok::Dot
                    }
                }
                ',' => {
                    self.advance();
                    Tok::Comma
                }
                ':' => {
                    self.advance();
                    Tok::Colon
                }
                ';' => {
                    self.advance();
                    Tok::Semicolon
                }
                '(' => {
                    self.advance();
                    Tok::LParen
                }
                ')' => {
                    self.advance();
                    Tok::RParen
                }
                '{' => {
                    self.advance();
                    Tok::LBrace
                }
                '}' => {
                    self.advance();
                    Tok::RBrace
                }
                '[' => {
                    self.advance();
                    Tok::LBracket
                }
                ']' => {
                    self.advance();
                    Tok::RBracket
                }
                '?' => {
                    self.advance();
                    Tok::Question
                }
                '0'..='9' => self.lex_number(),
                '"' => self.lex_string(),
                'a'..='z' | 'A'..='Z' | '_' => self.lex_indent(),
                _ => {
                    // add error handling here later
                    self.advance();
                    self.next_token()
                }
            },
        }
    }

    fn lex_indent(&mut self) -> Tok {
        let mut s = String::new();
        while self
            .peek()
            .map_or(false, |c| c.is_alphanumeric() || c == '_')
        {
            s.push(self.advance());
        }
        match s.as_str() {
            "let" => Tok::Let,
            "mut" => Tok::Mut,
            "fn" => Tok::Fn,
            "return" => Tok::Return,
            "if" => Tok::If,
            "else" => Tok::Else,
            "while" => Tok::While,
            "for" => Tok::For,
            "in" => Tok::In,
            "match" => Tok::Match,
            "struct" => Tok::Struct,
            "impl" => Tok::Impl,
            "as" => Tok::As,
            "true" => Tok::Bool(true),
            "false" => Tok::Bool(false),
            "none" => Tok::None_,
            _ => Tok::Ident(s),
        }
    }

    fn lex_number(&mut self) -> Tok {
        let mut num = String::new();

        while self.peek().map_or(false, |c| c.is_ascii_digit()) {
            num.push(self.advance());
        }

        if self.peek() == Some('.') && self.peek_next() != Some('.') {
            let mut is_float = false;
            if self.peek_next().map_or(false, |c| c.is_ascii_digit()) {
                is_float = true;
                num.push(self.advance()); // '.'
                while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                    num.push(self.advance());
                }
            }
            if is_float {
                return Tok::Float(num.parse::<f64>().unwrap_or(0.0));
            }
        }

        Tok::Int(num.parse::<i64>().unwrap_or(0))
    }

    fn lex_string(&mut self) -> Tok {
        let mut s = String::new();
        self.advance(); // opening quote
        while let Some(c) = self.peek() {
            if c == '"' {
                self.advance();
                break;
            }
            if c == '\\' {
                self.advance();
                match self.peek() {
                    Some('n') => {
                        self.advance();
                        s.push('\n');
                    }
                    Some('t') => {
                        self.advance();
                        s.push('\t');
                    }
                    Some('r') => {
                        self.advance();
                        s.push('\r');
                    }
                    Some('"') => {
                        self.advance();
                        s.push('"');
                    }
                    Some('\\') => {
                        self.advance();
                        s.push('\\');
                    }
                    Some(other) => {
                        self.advance();
                        s.push(other);
                    }
                    None => break,
                }
            } else {
                s.push(self.advance());
            }
        }
        Tok::Str(s)
    }
}
