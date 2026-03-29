fn main() {
    let src = std::fs::read_to_string("example.pbl").expect("read example.pbl");
    let mut lexer = pebbles::lexer::Lexer::new(&src);
    let tokens = lexer.tokenize();
    let mut parser = pebbles::parser::Parser::new(tokens);
    let _ast = parser.parse().expect("parse example.pbl");
}
