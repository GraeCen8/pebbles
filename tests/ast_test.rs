use pebbles::ast::{FnDef, ImplBlock, Item, Param, Stmt, Type};

#[test]
fn ast_supports_impl_blocks_and_self_type() {
    let method = FnDef {
        name: "value".into(),
        params: vec![Param {
            name: "self".into(),
            ty: Type::SelfType,
            mutable: false,
        }],
        ret: Type::I64,
        body: vec![Stmt::Expr(pebbles::ast::Expr::Int(1, 10))],
        line: 10,
    };

    let impl_block = ImplBlock {
        type_name: "Widget".into(),
        methods: vec![method],
        line: 9,
    };

    let item = Item::Impl(impl_block);

    match item {
        Item::Impl(block) => {
            assert_eq!(block.type_name, "Widget");
            assert_eq!(block.line, 9);
            assert_eq!(block.methods.len(), 1);
            assert_eq!(block.methods[0].name, "value");
            assert_eq!(block.methods[0].line, 10);
            assert!(matches!(block.methods[0].params[0].ty, Type::SelfType));
        }
        _ => panic!("expected Item::Impl"),
    }
}
