fn main() {
    let src = std::fs::read_to_string("example.pbl").expect("read example.pbl");

    let mut lexer = pebbles::lexer::Lexer::new(&src);
    let tokens = lexer.tokenize();

    let mut parser = pebbles::parser::Parser::new(tokens);
    let ast = parser.parse().expect("parse example.pbl");

    let mut checker = pebbles::typeck::TypeChecker::new();
    checker.check(&ast).unwrap_or_else(|e| {
        eprintln!("error {}", e);
        std::process::exit(1);
    });

    // codegen
}
