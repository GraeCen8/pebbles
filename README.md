# Pebble

A small, statically-typed compiled language that feels like writing pseudocode. Pebble compiles to native binaries via LLVM.

```
fn greet(name: str) {
    print("hello, " + name)
}

fn main() {
    greet("world")
}
```

---

## Installation

**Requirements**

- Rust 1.70+
- LLVM 17 (`brew install llvm@17` on macOS, `apt install llvm-17` on Linux)

**Build from source**

```bash
git clone https://github.com/you/pebble
cd pebble
cargo build --release
```

The compiler binary will be at `target/release/pebble`.

---

## Usage

```bash
# Compile a file
pebble hello.pbl

# Run the output
./out

# Compile and run in one step
pebble hello.pbl && ./out
```

---

## Language tour

### Variables

```pebble
let x = 10              // immutable, type inferred as i32
let y: f64 = 3.14       // explicit type annotation
let mut count = 0       // mutable variable
count += 1
```

### Types

| Type | Description | Example |
|------|-------------|---------|
| `i32` | 32-bit integer | `42` |
| `i64` | 64-bit integer | `9999999999` |
| `f64` | 64-bit float | `3.14` |
| `bool` | Boolean | `true` / `false` |
| `str` | UTF-8 string | `"hello"` |
| `[T]` | Array of T | `[1, 2, 3]` |
| `?T` | Optional (nullable) T | `none` / `42` |

### Functions

```pebble
fn add(a: i32, b: i32) -> i32 {
    return a + b
}

// Last expression is an implicit return
fn mul(a: i32, b: i32) -> i32 {
    a * b
}
```

### Control flow

```pebble
// if / else if / else
if x > 10 {
    print("big")
} else if x == 10 {
    print("exact")
} else {
    print("small")
}

// while loop
let mut i = 0
while i < 5 {
    print(i)
    i += 1
}

// for loop with range
for i in 0..10 {    // exclusive: 0 to 9
    print(i)
}

for i in 0..=10 {   // inclusive: 0 to 10
    print(i)
}

// for loop over array
for item in items {
    print(item)
}
```

### Match

```pebble
match status {
    200 => print("ok")
    404 => print("not found")
    500 => print("server error")
    _   => print("unknown")     // wildcard
}

// match on tuples
let fizz = n % 3 == 0
let buzz = n % 5 == 0

match (fizz, buzz) {
    (true,  true)  => print("FizzBuzz")
    (true,  false) => print("Fizz")
    (false, true)  => print("Buzz")
    _              => print(str(n))
}
```

### Structs

```pebble
struct Point {
    x: i32
    y: i32
}

let p = Point { x: 10, y: 20 }
print(p.x)   // 10
```

### Methods

```pebble
impl Point {
    fn distance(self) -> f64 {
        let dx = self.x as f64
        let dy = self.y as f64
        sqrt(dx * dx + dy * dy)
    }

    fn translate(mut self, dx: i32, dy: i32) {
        self.x += dx
        self.y += dy
    }
}

let mut p = Point { x: 3, y: 4 }
print(p.distance())    // 5.0
p.translate(1, 1)
```

### Optionals

```pebble
let maybe: ?i32 = none
let val:   ?i32 = 42

if val != none {
    print(val)
}
```

### Type casting

```pebble
let x: i32 = 5
let y = x as f64    // explicit cast required between numeric types
```

---

## Built-in functions

| Function | Description |
|----------|-------------|
| `print(x)` | Print any value to stdout |
| `input() -> str` | Read a line from stdin |
| `len(x) -> i32` | Length of array or string |
| `str(x) -> str` | Convert any value to string |
| `int(x: str) -> i32` | Parse string to integer |
| `float(x: str) -> f64` | Parse string to float |
| `sqrt(x: f64) -> f64` | Square root |

---

## Full example

```pebble
struct Config {
    limit: i32
    fizz:  str
    buzz:  str
}

fn fizzbuzz(cfg: Config) {
    for i in 1..=cfg.limit {
        let f = i % 3 == 0
        let b = i % 5 == 0

        match (f, b) {
            (true,  true)  => print(cfg.fizz + cfg.buzz)
            (true,  false) => print(cfg.fizz)
            (false, true)  => print(cfg.buzz)
            _              => print(str(i))
        }
    }
}

fn main() {
    let cfg = Config { limit: 30, fizz: "Fizz", buzz: "Buzz" }
    fizzbuzz(cfg)
}
```

---

## Project structure

```
src/
  main.rs       — CLI entry point
  lexer.rs      — tokeniser
  ast.rs        — AST node types
  parser.rs     — recursive descent parser
  typeck.rs     — type checker
  codegen.rs    — LLVM IR emitter
```

---

## License

MIT
