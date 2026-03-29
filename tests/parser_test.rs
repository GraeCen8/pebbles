use pebbles::ast::{Item, Type};
use pebbles::lexer::Token;
use pebbles::parser::Parser;

#[test]
fn parses_impl_block_with_self_params() {
    let tokens = vec![
        (Token::Impl, 1),
        (Token::Ident("Foo".into()), 1),
        (Token::LBrace, 1),

        (Token::Fn, 2),
        (Token::Ident("bar".into()), 2),
        (Token::LParen, 2),
        (Token::Self_, 2),
        (Token::RParen, 2),
        (Token::LBrace, 2),
        (Token::RBrace, 2),

        (Token::Fn, 3),
        (Token::Ident("baz".into()), 3),
        (Token::LParen, 3),
        (Token::Mut, 3),
        (Token::Self_, 3),
        (Token::Comma, 3),
        (Token::Ident("x".into()), 3),
        (Token::Colon, 3),
        (Token::Ident("i32".into()), 3),
        (Token::RParen, 3),
        (Token::LBrace, 3),
        (Token::RBrace, 3),

        (Token::RBrace, 4),
        (Token::EOF, 4),
    ];

    let mut parser = Parser::new(tokens);
    let items = parser.parse().expect("parse impl block");

    assert_eq!(items.len(), 1);

    match &items[0] {
        Item::Impl(impl_block) => {
            assert_eq!(impl_block.type_name, "Foo");
            assert_eq!(impl_block.line, 1);
            assert_eq!(impl_block.methods.len(), 2);

            let bar = &impl_block.methods[0];
            assert_eq!(bar.name, "bar");
            assert_eq!(bar.line, 2);
            assert_eq!(bar.params.len(), 1);
            assert!(matches!(bar.params[0].ty, Type::SelfType));
            assert!(!bar.params[0].mutable);

            let baz = &impl_block.methods[1];
            assert_eq!(baz.name, "baz");
            assert_eq!(baz.line, 3);
            assert_eq!(baz.params.len(), 2);

            let self_param = &baz.params[0];
            assert_eq!(self_param.name, "self");
            assert!(matches!(self_param.ty, Type::SelfType));
            assert!(self_param.mutable);

            let x_param = &baz.params[1];
            assert_eq!(x_param.name, "x");
            assert!(matches!(x_param.ty, Type::I32));
        }
        other => panic!("expected impl item, found {:?}", other),
    }
}
