set shell := ["bash", "-cu"]

default: test

# Run all tests individually (explicitly listed)
test:
	cargo test --test lexer_test
	cargo test --test parser_test
	cargo test --test ast_test
	cargo test --test typeck_test

# Build the project
build:
	cargo build

# Run the parser over example.pbl (main)
run:
	cargo run
