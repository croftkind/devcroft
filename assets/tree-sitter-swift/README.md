# tree-sitter-swift highlight query

The built-in editor's tree-sitter integration (`gpui-component`) links the Swift grammar from [`tree-sitter-swift`](https://github.com/alex-pinkus/tree-sitter-swift) but registers its language config without a highlight query (as of `gpui-component` 0.7.3), so a `.swift` buffer would parse and paint nothing. The grammar crate does export its query, so a verbatim copy of its `queries/highlights.scm` is vendored here and installed over the bundled config by `src/editor/highlighting.rs`.

Source: `tree-sitter-swift` 0.7.4 on crates.io. License: MIT (`LICENSE`, © 2021 alex-pinkus).

Keep the copy in step with the grammar: a pattern naming a node the grammar does not have makes the whole query fail to compile, and the editor silently falls back to plain text. `src/editor/highlighting.rs` pins the highlight output with a test, so a grammar update that breaks the query fails there rather than shipping unhighlighted.
