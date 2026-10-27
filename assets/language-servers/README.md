# Managed language-server packages

The JSON manifests and npm lockfiles are bundled installation recipes, not project dependencies. Devcroft installs them in device-local staging directories with `npm ci --ignore-scripts --engine-strict` and validates the expected entry before publishing an active receipt.

- `typescript`: [typescript-language-server](https://github.com/typescript-language-server/typescript-language-server) and TypeScript.
- `python`: [basedpyright](https://github.com/DetachHead/basedpyright).
- `web`: [vscode-langservers-extracted](https://github.com/hrsh7th/vscode-langservers-extracted) 4.10.0 supplies the HTML, CSS, and JSON servers. Each is installed and managed independently.
- `yaml`: [yaml-language-server](https://github.com/redhat-developer/yaml-language-server) 2.0.0.
- `bash`: [bash-language-server](https://github.com/bash-lsp/bash-language-server) 5.8.1.

Swift uses [SourceKit-LSP](https://github.com/swiftlang/sourcekit-lsp) from an existing Xcode/Swift toolchain rather than a managed package.

The native [clangd 23.1.0](https://github.com/clangd/clangd/releases/tag/23.1.0) recipe in `src/editor/lsp/install.rs` pins the official release archive SHA-256 digests. It preserves `bin/`, resource headers under `lib/`, and the upstream license. macOS archives are universal; Linux archives support x86_64. Acquisition requires curl and unzip.
