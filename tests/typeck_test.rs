use pebbles::lexer::Token;
use pebbles::parser::Parser;
use pebbles::typeck::TypeChecker;

#[test]
fn typechecks_struct_impl_and_method_call() {
    // Manually provide lexer output to isolate parser/typechecker behavior.
    let tokens = vec![
        // struct Point { x: i32 }
        (Token::Struct, 1),
        (Token::Ident("Point".into()), 1),
        (Token::LBrace, 1),
        (Token::Ident("x".into()), 1),
        (Token::Colon, 1),
        (Token::Ident("i32".into()), 1),
        (Token::RBrace, 1),

        // impl Point { fn get(self) -> i32 { return 1 } }
        (Token::Impl, 2),
        (Token::Ident("Point".into()), 2),
        (Token::LBrace, 2),
        (Token::Fn, 2),
        (Token::Ident("get".into()), 2),
        (Token::LParen, 2),
        (Token::Self_, 2),
        (Token::RParen, 2),
        (Token::Arrow, 2),
        (Token::Ident("i32".into()), 2),
        (Token::LBrace, 2),
        (Token::Return, 2),
        (Token::Int(1), 2),
        (Token::RBrace, 2),
        (Token::RBrace, 2),

        // fn main() { let p = Point { x: 1 } let y: i32 = p.get() }
        (Token::Fn, 3),
        (Token::Ident("main".into()), 3),
        (Token::LParen, 3),
        (Token::RParen, 3),
        (Token::LBrace, 3),
        (Token::Let, 3),
        (Token::Ident("p".into()), 3),
        (Token::Assign, 3),
        (Token::Ident("Point".into()), 3),
        (Token::LBrace, 3),
        (Token::Ident("x".into()), 3),
        (Token::Colon, 3),
        (Token::Int(1), 3),
        (Token::RBrace, 3),
        (Token::Let, 3),
        (Token::Ident("y".into()), 3),
        (Token::Colon, 3),
        (Token::Ident("i32".into()), 3),
        (Token::Assign, 3),
        (Token::Ident("p".into()), 3),
        (Token::Dot, 3),
        (Token::Ident("get".into()), 3),
        (Token::LParen, 3),
        (Token::RParen, 3),
        (Token::RBrace, 3),
        (Token::EOF, 4),
    ];

    let mut parser = Parser::new(tokens);
    let items = parser.parse().expect("parse program");

    let mut tc = TypeChecker::new();
    tc.check(&items).expect("typecheck program");
}
