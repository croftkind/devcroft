//! Highlight queries for grammars whose bundled config ships without one.
//!
//! `gpui-component` links the protobuf and Swift grammars behind its
//! `tree-sitter-proto` / `tree-sitter-swift` features but registers those
//! languages with an empty highlight query (as of 0.7.1/0.7.3), so the
//! registry parses a `.proto` / `.swift` buffer and paints nothing. Neither
//! grammar crate's query is picked up by the bundle, so verbatim copies live
//! under `assets/tree-sitter-proto/` and `assets/tree-sitter-swift/` and are
//! installed over the bundled configs here. A query the bundle already has is
//! left untouched, so a future upstream fix retires the registration instead
//! of being masked by it.
//!
//! Installation is a one-time process-wide side effect on the language
//! registry, so it hangs off the editor's grammar resolution
//! (`native::language_for`) rather than app startup: a highlighter cannot be
//! created before the query it needs is installed, and the tests below resolve
//! a path through the same function the editor uses. A future surface that
//! assigns a proto or Swift highlighter without resolving a path through
//! `native::language_for` must call [`ensure_for`] itself.

use std::sync::Once;

/// Verbatim copy of `tree-sitter-proto` 0.2.0's `queries/highlights.scm`.
///
/// A query naming a node the linked grammar does not have fails to compile,
/// and the editor silently falls back to plain text, so the tests below pin
/// the highlight output rather than the query text.
const PROTOBUF_HIGHLIGHTS: &str = include_str!("../../assets/tree-sitter-proto/highlights.scm");

/// Verbatim copy of `tree-sitter-swift` 0.7.4's `queries/highlights.scm`.
///
/// Same compile-or-silently-plain-text contract as above: the tests below pin
/// the highlight output rather than the query text.
const SWIFT_HIGHLIGHTS: &str = include_str!("../../assets/tree-sitter-swift/highlights.scm");

static PROTOBUF: Once = Once::new();
static SWIFT: Once = Once::new();

/// Install the vendored query for `grammar` when the bundled config has none.
///
/// Cheap on every grammar resolution: after the first call this only checks a
/// `Once`. A no-op for grammars that need nothing, and when the grammar is not
/// linked in.
pub(crate) fn ensure_for(grammar: &str) {
    if grammar == "proto" {
        PROTOBUF.call_once(install_protobuf);
    } else if grammar == "swift" {
        SWIFT.call_once(install_swift);
    }
}

fn install_protobuf() {
    install_query("proto", PROTOBUF_HIGHLIGHTS);
}

fn install_swift() {
    install_query("swift", SWIFT_HIGHLIGHTS);
}

fn install_query(grammar: &str, highlights: &str) {
    let registry = gpui_kit::component::highlighter::LanguageRegistry::singleton();
    let Some(mut config) = registry.language(grammar) else {
        return;
    };
    if !config.highlights.is_empty() {
        return;
    }
    config.highlights = highlights.into();
    registry.register(grammar, &config);
}

#[cfg(test)]
mod tests {
    use crate::editor::native::language_for;
    use gpui_kit::base::input::Rope;
    use gpui_kit::component::highlighter::{HighlightTheme, LanguageRegistry, SyntaxHighlighter};
    use std::path::Path;

    const SOURCE: &str = r#"// The fleet's vehicles.
syntax = "proto3";

package fleet;

message Vehicle {
  string vin = 1;
  repeated int32 wheel_count = 2;
}

enum State {
  IDLE = 0;
  MOVING = 1;
}

service FleetService {
  rpc Get(GetRequest) returns (Vehicle);
}
"#;

    /// Resolve the grammar exactly as the editor does. The tests go through
    /// this path instead of calling [`ensure_for`] directly, so removing the
    /// installation from the editor's grammar resolution fails them rather
    /// than being masked by a test-only call.
    fn resolve_proto() {
        assert_eq!(language_for(Path::new("proto/fleet.proto")), "proto");
    }

    /// Resolve a Swift path exactly as the editor does (see above).
    fn resolve_swift() {
        assert_eq!(
            language_for(Path::new("Sources/ContentView.swift")),
            "swift"
        );
    }

    #[test]
    fn resolving_a_proto_path_installs_the_grammar_query() {
        resolve_proto();
        let config = LanguageRegistry::singleton()
            .language("proto")
            .expect("the tree-sitter-proto feature must register the language");
        assert!(
            config.has_grammar(),
            "the protobuf grammar must be linked in"
        );
        assert!(
            !config.highlights.is_empty(),
            "the registered protobuf config must carry a highlight query"
        );
    }

    #[test]
    fn protobuf_documents_highlight_keywords_comments_and_literals() {
        resolve_proto();
        let mut highlighter = SyntaxHighlighter::new("proto");
        let text = Rope::from_str(SOURCE);
        assert!(
            highlighter.update(None, &text, None),
            "the protobuf highlighter must parse the document"
        );
        // A query the grammar rejects makes `new` fall back to the inert
        // "text" highlighter, which paints nothing.
        assert_eq!(
            highlighter.language().as_ref(),
            "proto",
            "the highlighter must not fall back to plain text"
        );
        let theme = HighlightTheme::default_dark();
        let styles = highlighter.styles(&(0..text.len()), &*theme);
        assert!(
            styles.len() > 1,
            "protobuf must produce distinct styled spans, got {styles:?}"
        );
        let colored = |from: usize, through: usize| {
            styles.iter().any(|(range, style)| {
                range.start <= from && range.end >= through && style.color.is_some()
            })
        };
        let message_at = SOURCE.find("message").unwrap();
        assert!(
            colored(message_at, message_at + "message".len()),
            "the `message` keyword must carry a syntax color, got {styles:?}"
        );
        let comment_end = SOURCE.find('\n').unwrap();
        assert!(
            colored(0, comment_end),
            "the line comment must carry a syntax color, got {styles:?}"
        );
        let package = SOURCE.find("package").unwrap();
        assert!(
            colored(package, package + "package".len()),
            "the `package` keyword must carry a syntax color, got {styles:?}"
        );
    }

    const DOTENV_SOURCE: &str = r#"# Local development
TOKEN=dev-secret
DATABASE_URL="postgres://localhost/dev"
"#;

    #[test]
    fn dotenv_documents_highlight_keys_comments_and_quoted_values() {
        assert_eq!(language_for(Path::new(".env")), "bash");
        let mut highlighter = SyntaxHighlighter::new("bash");
        let text = Rope::from_str(DOTENV_SOURCE);
        assert!(
            highlighter.update(None, &text, None),
            "the bash highlighter must parse a dotenv document"
        );
        assert_eq!(
            highlighter.language().as_ref(),
            "bash",
            "the highlighter must not fall back to plain text"
        );
        let theme = HighlightTheme::default_dark();
        let styles = highlighter.styles(&(0..text.len()), &*theme);
        assert!(
            styles.len() > 1,
            "dotenv must produce distinct styled spans, got {styles:?}"
        );
        let colored = |from: usize, through: usize| {
            styles.iter().any(|(range, style)| {
                range.start <= from && range.end >= through && style.color.is_some()
            })
        };
        let token = DOTENV_SOURCE.find("TOKEN").unwrap();
        assert!(
            colored(token, token + "TOKEN".len()),
            "the `TOKEN` key must carry a syntax color, got {styles:?}"
        );
        let comment_end = DOTENV_SOURCE.find('\n').unwrap();
        assert!(
            colored(0, comment_end),
            "the comment must carry a syntax color, got {styles:?}"
        );
        let url = DOTENV_SOURCE.find("postgres://localhost/dev").unwrap();
        assert!(
            colored(url, url + "postgres://localhost/dev".len()),
            "the quoted value must carry a syntax color, got {styles:?}"
        );
    }

    const SWIFT_SOURCE: &str = r#"// The fleet's vehicles.
import Foundation

struct Vehicle {
  let vin: String
  var wheelCount: Int = 4

  func describe() -> String {
    return "VIN \(vin)"
  }
}
"#;

    #[test]
    fn resolving_a_swift_path_installs_the_grammar_query() {
        resolve_swift();
        let config = LanguageRegistry::singleton()
            .language("swift")
            .expect("the tree-sitter-swift feature must register the language");
        assert!(config.has_grammar(), "the Swift grammar must be linked in");
        assert!(
            !config.highlights.is_empty(),
            "the registered Swift config must carry a highlight query"
        );
    }

    #[test]
    fn swift_documents_highlight_keywords_comments_and_types() {
        resolve_swift();
        let mut highlighter = SyntaxHighlighter::new("swift");
        let text = Rope::from_str(SWIFT_SOURCE);
        assert!(
            highlighter.update(None, &text, None),
            "the Swift highlighter must parse the document"
        );
        // A query the grammar rejects makes `new` fall back to the inert
        // "text" highlighter, which paints nothing.
        assert_eq!(
            highlighter.language().as_ref(),
            "swift",
            "the highlighter must not fall back to plain text"
        );
        let theme = HighlightTheme::default_dark();
        let styles = highlighter.styles(&(0..text.len()), &*theme);
        assert!(
            styles.len() > 1,
            "Swift must produce distinct styled spans, got {styles:?}"
        );
        let colored = |from: usize, through: usize| {
            styles.iter().any(|(range, style)| {
                range.start <= from && range.end >= through && style.color.is_some()
            })
        };
        let struct_at = SWIFT_SOURCE.find("struct").unwrap();
        assert!(
            colored(struct_at, struct_at + "struct".len()),
            "the `struct` keyword must carry a syntax color, got {styles:?}"
        );
        let comment_end = SWIFT_SOURCE.find('\n').unwrap();
        assert!(
            colored(0, comment_end),
            "the line comment must carry a syntax color, got {styles:?}"
        );
        let import_at = SWIFT_SOURCE.find("import").unwrap();
        assert!(
            colored(import_at, import_at + "import".len()),
            "the `import` keyword must carry a syntax color, got {styles:?}"
        );
    }
}
