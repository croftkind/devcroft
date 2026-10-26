# Vendored syntect syntaxes

`syntect`'s default syntax set does not bundle Swift, TypeScript, or TSX, so those files fall through Review's `syntax_for_path` and render unhighlighted. The Review tab builds its set from the defaults plus the definitions vendored here (see `src/diff/syntax.rs`).

- `Swift.sublime-syntax`: verbatim copy of [`Swift.sublime-syntax`](https://github.com/aerobounce/Swift-Next/blob/main/Swift.sublime-syntax) from [Swift-Next](https://github.com/aerobounce/Swift-Next) (`name: Swift`, `scope: source.swift`, extensions `swift`/`swiftinterface`). License: MIT (`LICENSE`, © 2022 aerobounce).
- `TypeScript.sublime-syntax`: verbatim copy of [bat's TypeScript definition](https://github.com/sharkdp/bat/blob/a9aa73d215455a479f072da75dd39f19826acfea/assets/syntaxes/02_Extra/TypeScript.sublime-syntax), pinned at `a9aa73d215455a479f072da75dd39f19826acfea` (`name: TypeScript`, `scope: source.ts`, extensions `ts`/`mts`/`cts`).
- `TypeScriptReact.sublime-syntax`: verbatim copy of [bat's TSX definition](https://github.com/sharkdp/bat/blob/a9aa73d215455a479f072da75dd39f19826acfea/assets/syntaxes/02_Extra/TypsecriptReact.sublime-syntax), at the same revision; the upstream filename is misspelled `TypsecriptReact` (`name: TypeScriptReact`, `scope: source.tsx`, extension `tsx`).

The TypeScript definitions are bat's Sublime-syntax conversions of Microsoft's [TypeScript-Sublime-Plugin](https://github.com/microsoft/TypeScript-Sublime-Plugin/tree/ba45efd058df5111837e30fb9598cfc8cbd51095), whose revision is pinned by that bat checkout. The upstream Apache 2.0 license is preserved in `TypeScript-LICENSE`; bat's MIT license is preserved in `TypeScript-bat-LICENSE-MIT`. No local grammar changes are applied. Use these syntect-compatible definitions rather than newer Sublime definitions requiring unsupported syntax inheritance.

Keep the copies working with the pinned `syntect`: a definition the YAML loader rejects breaks the `SYNTAXES` static at startup. Tests in `src/diff/syntax.rs` pin path resolution, actual keyword/type/TSX highlighting in both themes, and UTF-8 byte offsets so bad vendoring fails there rather than shipping unhighlighted.
