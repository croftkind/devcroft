//! File identity is independent of both the highlighting grammar and server.
use super::lsp::catalog::ServerId;
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Language {
    pub grammar: &'static str,
    pub id: &'static str,
    pub server: Option<ServerId>,
}

pub(crate) fn detect(path: &Path) -> Language {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
    let (grammar, id, server) = if path.file_name().is_some_and(|n| n == "Cargo.lock") {
        ("toml", "toml", None)
    } else {
        match ext {
            "rs" => ("rust", "rust", Some(ServerId::Rust)),
            "go" => ("go", "go", Some(ServerId::Go)),
            "js" | "mjs" | "cjs" => ("javascript", "javascript", Some(ServerId::TypeScript)),
            "jsx" => ("javascript", "javascriptreact", Some(ServerId::TypeScript)),
            "ts" | "mts" | "cts" => ("typescript", "typescript", Some(ServerId::TypeScript)),
            "tsx" => ("tsx", "typescriptreact", Some(ServerId::TypeScript)),
            "py" | "pyi" => ("python", "python", Some(ServerId::Python)),
            "toml" => ("toml", "toml", None),
            "sh" | "bash" => ("bash", "shellscript", Some(ServerId::Bash)),
            ext if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") => {
                ("markdown", "markdown", None)
            }
            "html" | "htm" => ("html", "html", Some(ServerId::Html)),
            "css" => ("css", "css", Some(ServerId::Css)),
            "json" => ("json", "json", Some(ServerId::Json)),
            "yml" | "yaml" => ("yaml", "yaml", Some(ServerId::Yaml)),
            "c" | "h" => ("c", "c", Some(ServerId::Clangd)),
            "C" | "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "H" => {
                ("cpp", "cpp", Some(ServerId::Clangd))
            }
            "java" => ("java", "java", None),
            "swift" | "swiftinterface" => ("swift", "swift", Some(ServerId::Swift)),
            // No protobuf language server is wired, so the id is not sent
            // anywhere yet.
            "proto" => ("proto", "protobuf", None),
            "rb" => ("ruby", "ruby", None),
            _ => ("", "plaintext", None),
        }
    };
    Language {
        grammar,
        id,
        server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grammar_and_protocol_ids_are_independent() {
        assert_eq!(detect(Path::new("app.tsx")).grammar, "tsx");
        assert_eq!(detect(Path::new("app.tsx")).id, "typescriptreact");
        assert_eq!(detect(Path::new("app.jsx")).id, "javascriptreact");
        assert_eq!(
            detect(Path::new("app.mts")).server,
            Some(ServerId::TypeScript)
        );
        assert!(detect(Path::new("README.md")).server.is_none());
        assert_eq!(detect(Path::new("README.MD")).grammar, "markdown");
        assert_eq!(detect(Path::new("notes.markdown")).grammar, "markdown");
    }

    #[test]
    fn common_languages_route_to_their_servers_and_grammars() {
        for (names, grammar, id, server) in [
            ("main.c api.h", "c", "c", ServerId::Clangd),
            (
                "main.cpp main.cc main.cxx main.C api.hpp api.hh api.hxx api.H main.c++ api.h++",
                "cpp",
                "cpp",
                ServerId::Clangd,
            ),
            ("index.html index.htm", "html", "html", ServerId::Html),
            ("styles.css", "css", "css", ServerId::Css),
            ("package.json", "json", "json", ServerId::Json),
            ("config.yaml config.yml", "yaml", "yaml", ServerId::Yaml),
            ("run.sh run.bash", "bash", "shellscript", ServerId::Bash),
            (
                "main.swift Module.swiftinterface",
                "swift",
                "swift",
                ServerId::Swift,
            ),
        ] {
            for name in names.split_whitespace() {
                let language = detect(Path::new(name));
                assert_eq!(
                    (language.grammar, language.id, language.server),
                    (grammar, id, Some(server)),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn protobuf_files_use_the_bundled_grammar() {
        let language = detect(Path::new("proto/fleet.proto"));
        assert_eq!(language.grammar, "proto");
        assert_eq!(language.id, "protobuf");
        assert!(language.server.is_none());
    }

    #[test]
    fn swift_files_use_the_bundled_grammar() {
        let language = detect(Path::new("Sources/ContentView.swift"));
        assert_eq!(language.grammar, "swift");
        assert_eq!(language.id, "swift");
        assert_eq!(language.server, Some(ServerId::Swift));
        let interface = detect(Path::new("Module.swiftinterface"));
        assert_eq!(interface.grammar, "swift");
        assert_eq!(interface.server, Some(ServerId::Swift));
    }
}
