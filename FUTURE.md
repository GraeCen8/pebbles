# these are the next core things i want pebbles to implement

  1. Diagnostics that users can trust
      - Lexer/parser errors with line/column, file name, and caret highlights.
      - Type errors that show both expected/got types and the expression span.
  2. Memory management story
      - Strings and arrays allocate but never free.
      - Decide: GC, ARC, or manual free semantics. GC is usually easiest for users.
  3. Standard library
      - Core: print, println, len, str, int, float, sqrt (done), plus file I/O, basic collections, and simple math.
      - String utilities: concat (done), substring, split, replace.
      - Array utilities: map/filter/reduce or at least slice + sort.
  4. Module / import system
      - Multiple files, import, namespacing, and a build graph.
  5. Tests + CI
      - Golden tests for parser, typechecker, and codegen.
      - A few end‑to‑end compile+run tests.

  Big missing features

  1. Ownership/borrowing or GC
      - Without it, programs leak and have no clear lifetime model.
  2. Error handling model
      - You have ? optional types and none, but no result/error type.
      - Add Result<T, E> or a built‑in error type.
  3. User‑defined generics
      - Hard to build useful collections without generics.
  4. Better runtime
      - Bounds checks (arrays/strings), panic reporting, safe math, exit codes.

  “Nice to have” that elevates quality

  1. REPL
  2. Formatter
  3. LSP / editor support
  4. Package manager
  5. Docs and a language reference

  ———
