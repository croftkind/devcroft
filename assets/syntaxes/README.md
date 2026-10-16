# Vendored syntect syntaxes

`syntect`'s default syntax set does not bundle Swift, so `.swift` files fall through Review's `syntax_for_path` and render unhighlighted. The Review tab builds its set from the defaults plus the definitions vendored here (see `src/diff/syntax.rs`).

- `Swift.sublime-syntax`: verbatim copy of [`Swift.sublime-syntax`](https://github.com/aerobounce/Swift-Next/blob/main/Swift.sublime-syntax) from [Swift-Next](https://github.com/aerobounce/Swift-Next) (`name: Swift`, `scope: source.swift`, extensions `swift`/`swiftinterface`). License: MIT (`LICENSE`, © 2022 aerobounce).

Keep the copy working with the pinned `syntect`: a definition the YAML loader rejects breaks the `SYNTAXES` static at startup, and `src/diff/syntax.rs` pins Swift resolution plus keyword highlighting with tests so a bad vendoring fails there rather than shipping unhighlighted.
