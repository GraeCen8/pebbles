pub use crate::ast::Token;

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    line: usize,
}

impl Lexer {
    pub fn new(src: &str) -> Self {
        Self {
            input: src.chars().collect(),
            pos: 0,
            line: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<(Token, usize)> {
        let mut tokens: Vec<(Token, usize)> = vec![];
        loop {
            let (tok, line) = self.next_token();
            let done = tok == Token::EOF;
            tokens.push((tok, line));
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
        if c == '\n' {
            self.line += 1;
        }
        c
    }

    fn next_token(&mut self) -> (Token, usize) {
        //skip whitespace
        while self.peek().map_or(false, |c| c.is_whitespace()) {
            self.advance();
        }

        let line = self.line;

        match self.peek() {
            None => (Token::EOF, line),
            Some(c) => match c {
                '+' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::PlusAssign, line)
                    } else {
                        (Token::Plus, line)
                    }
                }
                '-' => {
                    self.advance();
                    if self.peek() == Some('>') {
                        self.advance();
                        (Token::Arrow, line)
                    } else if self.peek() == Some('=') {
                        self.advance();
                        (Token::MinusAssign, line)
                    } else {
                        (Token::Minus, line)
                    }
                }
                '*' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::StarAssign, line)
                    } else {
                        (Token::Star, line)
                    }
                }
                '/' => {
                    self.advance();
                    if self.peek() == Some('/') {
                        while self.peek().map_or(false, |ch| ch != '\n') {
                            self.advance();
                        }
                        return self.next_token();
                    } else if self.peek() == Some('*') {
                        self.advance(); // consume '*'
                        while let Some(ch) = self.peek() {
                            if ch == '*' && self.peek_next() == Some('/') {
                                self.advance(); // '*'
                                self.advance(); // '/'
                                break;
                            }
                            self.advance();
                        }
                        return self.next_token();
                    } else if self.peek() == Some('=') {
                        self.advance();
                        (Token::SlashAssign, line)
                    } else {
                        (Token::Slash, line)
                    }
                }
                '%' => {
                    self.advance();
                    (Token::Percent, line)
                }
                '=' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::Eq, line)
                    } else if self.peek() == Some('>') {
                        self.advance();
                        (Token::FatArrow, line)
                    } else {
                        (Token::Assign, line)
                    }
                }
                '!' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::NotEq, line)
                    } else {
                        (Token::Bang, line)
                    }
                }
                '<' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::LtEq, line)
                    } else {
                        (Token::Lt, line)
                    }
                }
                '>' => {
                    self.advance();
                    if self.peek() == Some('=') {
                        self.advance();
                        (Token::GtEq, line)
                    } else {
                        (Token::Gt, line)
                    }
                }
                '&' => {
                    self.advance();
                    if self.peek() == Some('&') {
                        self.advance();
                        (Token::And, line)
                    } else {
                        return self.next_token();
                    }
                }
                '|' => {
                    self.advance();
                    if self.peek() == Some('|') {
                        self.advance();
                        (Token::Or, line)
                    } else {
                        return self.next_token();
                    }
                }
                '.' => {
                    self.advance();
                    if self.peek() == Some('.') {
                        self.advance();
                        if self.peek() == Some('=') {
                            self.advance();
                            (Token::DotDotEq, line)
                        } else {
                            (Token::DotDot, line)
                        }
                    } else {
                        (Token::Dot, line)
                    }
                }
                ',' => {
                    self.advance();
                    (Token::Comma, line)
                }
                ':' => {
                    self.advance();
                    (Token::Colon, line)
                }
                ';' => {
                    self.advance();
                    (Token::Semicolon, line)
                }
                '(' => {
                    self.advance();
                    (Token::LParen, line)
                }
                ')' => {
                    self.advance();
                    (Token::RParen, line)
                }
                '{' => {
                    self.advance();
                    (Token::LBrace, line)
                }
                '}' => {
                    self.advance();
                    (Token::RBrace, line)
                }
                '[' => {
                    self.advance();
                    (Token::LBracket, line)
                }
                ']' => {
                    self.advance();
                    (Token::RBracket, line)
                }
                '?' => {
                    self.advance();
                    (Token::Question, line)
                }
                '0'..='9' => (self.lex_number(), line),
                '"' => (self.lex_string(), line),
                'a'..='z' | 'A'..='Z' | '_' => (self.lex_indent(), line),
                _ => {
                    // add error handling here later
                    self.advance();
                    self.next_token()
                }
            },
        }
    }

    fn lex_indent(&mut self) -> Token {
        let mut s = String::new();
        while self
            .peek()
            .map_or(false, |c| c.is_alphanumeric() || c == '_')
        {
            s.push(self.advance());
        }
        match s.as_str() {
            "_" => Token::Underscore,
            "let" => Token::Let,
            "mut" => Token::Mut,
            "fn" => Token::Fn,
            "return" => Token::Return,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "in" => Token::In,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "match" => Token::Match,
            "struct" => Token::Struct,
            "impl" => Token::Impl,
            "self" => Token::Self_,
            "as" => Token::As,
            "true" => Token::Bool(true),
            "false" => Token::Bool(false),
            "none" => Token::None_,
            _ => Token::Ident(s),
        }
    }

    fn lex_number(&mut self) -> Token {
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
                return Token::Float(num.parse::<f64>().unwrap_or(0.0));
            }
        }

        Token::Int(num.parse::<i64>().unwrap_or(0))
    }

    fn lex_string(&mut self) -> Token {
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
        Token::StringLit(s)
    }
}
