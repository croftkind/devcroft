//! Cached syntax styles for diff lines.
//!
//! Highlighting is computed once with the diff on the background executor.
//! The nested cache mirrors `files -> hunks -> lines`, so virtualized row
//! rendering only has to look up already-resolved byte ranges.
//!
//! The Review tab resolves the syntax from each file's path ([`highlight`]);
//! the text diff tool has no path, so its dialog offers a language picker
//! ([`syntax_names`]) and highlights its single document with
//! [`highlight_hunks`]. The row rendering they share lives in
//! [`super::stream`].

use std::path::Path;
use std::sync::LazyLock;

use crate::editor::is_dotenv;
use gpui_kit::{FontStyle as TextFontStyle, FontWeight, HighlightStyle, UnderlineStyle, px, rgba};
use syntect::easy::HighlightLines;
use syntect::highlighting::{FontStyle, Style, Theme, ThemeSet};
/// Re-exported so callers that detect a language without a file path (the
/// text diff tool) can name what they get back.
pub(crate) use syntect::parsing::SyntaxReference;
use syntect::parsing::{SyntaxDefinition, SyntaxSet};

#[cfg(test)]
use super::model::{FileContent, ReviewDiff};
use super::model::{Hunk, LineTag};

/// Verbatim copy of Swift-Next's `Swift.sublime-syntax` (see
/// `assets/syntaxes/README.md`). `syntect`'s default set does not bundle
/// Swift, so without this `.swift` files resolve to nothing and the Review
/// tab paints them plain.
const SWIFT_SYNTAX: &str = include_str!("../../assets/syntaxes/Swift.sublime-syntax");
/// Dotenv files have no usable extension (`.env`) or one that names their
/// environment (`.env.local`), so syntect's extension lookup misses them. This
/// definition from bat colors their assignment keys, comments, and values.
const DOTENV_SYNTAX: &str = include_str!("../../assets/syntaxes/DotENV.sublime-syntax");
// Syntect's defaults also omit TypeScript and TSX. These definitions are
// vendored from bat's syntect-compatible bundle (see the same README).
const TYPESCRIPT_SYNTAX: &str = include_str!("../../assets/syntaxes/TypeScript.sublime-syntax");
const TSX_SYNTAX: &str = include_str!("../../assets/syntaxes/TypeScriptReact.sublime-syntax");

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(|| {
    let mut builder = SyntaxSet::load_defaults_newlines().into_builder();
    for (name, source) in [
        ("DotENV", DOTENV_SYNTAX),
        ("Swift", SWIFT_SYNTAX),
        ("TypeScript", TYPESCRIPT_SYNTAX),
        ("TSX", TSX_SYNTAX),
    ] {
        let syntax = SyntaxDefinition::load_from_str(source, true, None)
            .unwrap_or_else(|error| panic!("the vendored {name} syntax must parse: {error}"));
        builder.add(syntax);
    }
    builder.build()
});
static THEMES: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

const DARK_THEME: &str = "base16-ocean.dark";
const LIGHT_THEME: &str = "InspiredGitHub";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SyntaxSpan {
    pub(crate) range: std::ops::Range<usize>,
    pub(crate) rgba: u32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
}

/// Convert resolved spans into the style runs GPUI text wants. Shared by the
/// review stream and the diff tool's cells, so a span paints identically in
/// both surfaces.
pub(crate) fn styled_highlights(
    spans: &[SyntaxSpan],
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    spans
        .iter()
        .map(|span| {
            (
                span.range.clone(),
                HighlightStyle {
                    color: Some(rgba(span.rgba).into()),
                    font_weight: span.bold.then_some(FontWeight::BOLD),
                    font_style: span.italic.then_some(TextFontStyle::Italic),
                    underline: span.underline.then_some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(rgba(span.rgba).into()),
                        wavy: false,
                    }),
                    ..Default::default()
                },
            )
        })
        .collect()
}

#[derive(Default)]
struct FileHighlights {
    /// One span list per line, grouped by hunk.
    hunks: Vec<Vec<Vec<SyntaxSpan>>>,
}

#[derive(Default)]
struct PaletteHighlights {
    files: Vec<FileHighlights>,
}

/// Both appearance variants are cached because changing the application
/// theme does not reload the underlying git diff.
#[derive(Default)]
pub(crate) struct SyntaxHighlights {
    light: PaletteHighlights,
    dark: PaletteHighlights,
}

impl SyntaxHighlights {
    pub(crate) fn line(&self, dark: bool, file: usize, hunk: usize, line: usize) -> &[SyntaxSpan] {
        let palette = if dark { &self.dark } else { &self.light };
        palette
            .files
            .get(file)
            .and_then(|file| file.hunks.get(hunk))
            .and_then(|hunk| hunk.get(line))
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// One line of a single-document highlight ([`highlight_hunks`]): the
    /// text diff tool's comparison, which has no file dimension.
    pub(crate) fn document_line(&self, dark: bool, hunk: usize, line: usize) -> &[SyntaxSpan] {
        self.line(dark, 0, hunk, line)
    }
}

#[cfg(test)]
pub(crate) fn highlight(diff: &ReviewDiff) -> SyntaxHighlights {
    SyntaxHighlights {
        light: highlight_with_theme(diff, &THEMES.themes[LIGHT_THEME]),
        dark: highlight_with_theme(diff, &THEMES.themes[DARK_THEME]),
    }
}

/// Highlight the hunks of one document with `syntax`, for the text diff
/// tool. The review's files go through [`highlight`] instead, which resolves
/// a syntax per path; this is the same per-hunk machinery for a comparison
/// that has no path to resolve from.
pub(crate) fn highlight_hunks(hunks: &[Hunk], syntax: &SyntaxReference) -> SyntaxHighlights {
    let palette = |theme: &Theme| PaletteHighlights {
        files: vec![FileHighlights {
            hunks: hunks
                .iter()
                .map(|hunk| highlight_hunk(hunk, syntax, theme))
                .collect(),
        }],
    };
    SyntaxHighlights {
        light: palette(&THEMES.themes[LIGHT_THEME]),
        dark: palette(&THEMES.themes[DARK_THEME]),
    }
}

/// Resolve a syntax by display name (`"JSON"`, `"Rust"`), for callers that
/// know the language without a file path.
pub(crate) fn syntax_by_name(name: &str) -> Option<&'static SyntaxReference> {
    SYNTAXES.find_syntax_by_name(name)
}

/// Kit supplies one hunk side at a time. Keep its byte offsets across lines
/// while using the same per-line styles and fresh parser state as Review.
pub(crate) fn highlight_source(
    language: &str,
    source: &str,
    dark: bool,
) -> Vec<(std::ops::Range<usize>, HighlightStyle)> {
    let Some(syntax) = syntax_by_name(language) else {
        return Vec::new();
    };
    let theme = &THEMES.themes[if dark { DARK_THEME } else { LIGHT_THEME }];
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut offset = 0;
    let mut highlights = Vec::new();
    for line in source.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        let spans = highlight_line(
            &mut highlighter,
            &format!("{text}\n"),
            text.len(),
            theme.settings.foreground,
        );
        highlights.extend(
            styled_highlights(&spans)
                .into_iter()
                .map(|(range, style)| (range.start + offset..range.end + offset, style)),
        );
        offset += line.len();
    }
    highlights
}

/// Every bundled syntax's display name, sorted, for a language picker.
pub(crate) fn syntax_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = SYNTAXES
        .syntaxes()
        .iter()
        .map(|syntax| syntax.name.as_str())
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

#[cfg(test)]
fn highlight_with_theme(diff: &ReviewDiff, theme: &Theme) -> PaletteHighlights {
    let files = diff
        .files
        .iter()
        .map(|file| {
            let FileContent::Text { hunks, .. } = &file.content else {
                return FileHighlights::default();
            };
            let Some(syntax) = syntax_for_path(&file.path) else {
                return FileHighlights::default();
            };
            FileHighlights {
                hunks: hunks
                    .iter()
                    .map(|hunk| highlight_hunk(hunk, syntax, theme))
                    .collect(),
            }
        })
        .collect();
    PaletteHighlights { files }
}

pub(super) fn syntax_for_path(path: &str) -> Option<&'static SyntaxReference> {
    let path = Path::new(path);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    // Dotenv files have no usable extension (`.env`) or one that names their
    // environment (`.env.local`), so route them by name rather than relying on
    // the definition's extension list.
    if is_dotenv(path) {
        return syntax_by_name("DotENV");
    }
    // Keep C/C++ header and alternate suffix identities consistent with the
    // editor, including uppercase .C (C++, unlike lowercase .c).
    match extension {
        "c" | "h" => return syntax_by_name("C"),
        "C" | "cc" | "cpp" | "cxx" | "c++" | "hh" | "hpp" | "hxx" | "h++" | "H" => {
            return syntax_by_name("C++");
        }
        _ => {}
    }
    SYNTAXES
        .find_syntax_by_extension(file_name)
        .or_else(|| SYNTAXES.find_syntax_by_extension(extension))
}

fn highlight_hunk(hunk: &Hunk, syntax: &SyntaxReference, theme: &Theme) -> Vec<Vec<SyntaxSpan>> {
    let mut old = HighlightLines::new(syntax, theme);
    let mut new = HighlightLines::new(syntax, theme);
    let base_color = theme.settings.foreground;

    hunk.lines
        .iter()
        .map(|line| {
            // The default syntax set expects line endings. Keep the diff model's
            // newline-free text untouched and clamp returned spans before it.
            let source = format!("{}\n", line.text);
            match line.tag {
                LineTag::Deletion => highlight_line(&mut old, &source, line.text.len(), base_color),
                LineTag::Addition => highlight_line(&mut new, &source, line.text.len(), base_color),
                LineTag::Context => {
                    // Advance both sides through shared context. Their parser
                    // states may differ after a changed multiline construct;
                    // the new side is what the worktree will retain.
                    let _ = old.highlight_line(&source, &SYNTAXES);
                    highlight_line(&mut new, &source, line.text.len(), base_color)
                }
            }
        })
        .collect()
}

fn highlight_line(
    highlighter: &mut HighlightLines<'_>,
    source: &str,
    text_len: usize,
    base_color: Option<syntect::highlighting::Color>,
) -> Vec<SyntaxSpan> {
    let Ok(parts) = highlighter.highlight_line(source, &SYNTAXES) else {
        return Vec::new();
    };
    let mut offset = 0;
    parts
        .into_iter()
        .filter_map(|(style, text)| {
            let start = offset.min(text_len);
            offset += text.len();
            let end = offset.min(text_len);
            (start < end && is_visible_style(style, base_color)).then(|| SyntaxSpan {
                range: start..end,
                rgba: u32::from_be_bytes([
                    style.foreground.r,
                    style.foreground.g,
                    style.foreground.b,
                    style.foreground.a,
                ]),
                bold: style.font_style.contains(FontStyle::BOLD),
                italic: style.font_style.contains(FontStyle::ITALIC),
                underline: style.font_style.contains(FontStyle::UNDERLINE),
            })
        })
        .collect()
}

fn is_visible_style(style: Style, base_color: Option<syntect::highlighting::Color>) -> bool {
    base_color != Some(style.foreground) || !style.font_style.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::model::{ChangedFile, FileStatus, HunkLine};

    fn rust_diff() -> ReviewDiff {
        ReviewDiff {
            files: vec![ChangedFile {
                path: "src/main.rs".into(),
                old_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                content: FileContent::Text {
                    hunks: vec![Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        collapsed_before: 0,
                        lines: vec![
                            HunkLine {
                                tag: LineTag::Deletion,
                                old_no: Some(1),
                                new_no: None,
                                text: "fn old() {}".into(),
                                ending: "\n",
                            },
                            HunkLine {
                                tag: LineTag::Addition,
                                old_no: None,
                                new_no: Some(1),
                                text: "fn new() {}".into(),
                                ending: "\n",
                            },
                        ],
                    }],
                    truncated: false,
                },
            }],
            file_versions: Default::default(),
            base_commit: String::new(),
            head_commit: String::new(),
            base_ref: None,
            head_branch: None,
        }
    }

    #[test]
    fn recognizes_extensions_and_special_file_names() {
        assert_eq!(syntax_for_path("src/lib.rs").unwrap().name, "Rust");
        assert_eq!(syntax_for_path("scripts/foo.py").unwrap().name, "Python");
        assert_eq!(syntax_for_path("src/foo.PY").unwrap().name, "Python");
        assert_eq!(syntax_for_path("./foo.py").unwrap().name, "Python");
        assert_eq!(syntax_for_path("/abs/path/foo.py").unwrap().name, "Python");
        assert_eq!(syntax_for_path("foo.pyw").unwrap().name, "Python");
        assert_eq!(syntax_for_path("foo.pyi").unwrap().name, "Python");
        assert_eq!(syntax_for_path("foo.py3").unwrap().name, "Python");
        assert!(syntax_for_path("Makefile").is_some());
        assert!(syntax_for_path("notes.unknown-extension").is_none());
    }

    #[test]
    fn dotenv_files_resolve_to_the_dotenv_syntax() {
        for path in [
            ".env",
            ".env.local",
            ".env.production",
            "config/.env.example",
            "local.env",
        ] {
            assert_eq!(
                syntax_for_path(path).map(|syntax| syntax.name.as_str()),
                Some("DotENV"),
                "{path}"
            );
        }
        assert!(syntax_for_path(".environment").is_none());
    }

    #[test]
    fn dotenv_assignments_and_comments_highlight_in_both_themes() {
        let source =
            "# Local development\nTOKEN=dev-secret\nDATABASE_URL=\"postgres://localhost/dev\"\n";
        for dark in [false, true] {
            let spans = highlight_source("DotENV", source, dark);
            for token in ["# Local development", "postgres://localhost/dev"] {
                let start = source.find(token).unwrap();
                assert!(
                    spans
                        .iter()
                        .any(|(range, _)| range.start < start + token.len() && range.end > start),
                    "dark={dark}: {token} must be colored, got {spans:?}"
                );
            }
        }
        // The light theme leaves `variable.other` at the base foreground, so
        // `highlight_source` filters the key as invisible; the dark theme gives
        // it a distinct color.
        let spans = highlight_source("DotENV", source, true);
        let token = source.find("TOKEN").unwrap();
        assert!(
            spans
                .iter()
                .any(|(range, _)| { range.start <= token && range.end >= token + "TOKEN".len() }),
            "the assignment key must carry a syntax color, got {spans:?}"
        );
    }

    #[test]
    fn recognizes_typescript_and_tsx_extensions() {
        for path in [
            "src/main.ts",
            "src/main.mts",
            "src/main.cts",
            "types/index.d.ts",
            "types/index.d.mts",
            "types/index.d.cts",
            "src/main.TS",
        ] {
            assert_eq!(
                syntax_for_path(path).map(|syntax| syntax.name.as_str()),
                Some("TypeScript"),
                "{path} must resolve to TypeScript"
            );
        }
        for path in ["src/App.tsx", "src/App.TSX"] {
            assert_eq!(
                syntax_for_path(path).map(|syntax| syntax.name.as_str()),
                Some("TypeScriptReact"),
                "{path} must resolve to TSX"
            );
        }
    }

    #[test]
    fn common_languages_highlight_review_changes_in_both_themes() {
        for (paths, name, old, new, token) in [
            (
                "run.sh run.bash",
                "Bourne Again Shell (bash)",
                "if true; then echo \"before\"; fi\n",
                "if true; then echo \"after\"; fi\n",
                "if",
            ),
            (
                "main.swift Module.swiftinterface",
                "Swift",
                "func before() -> Int { return 1 }\n",
                "func after() -> Int { return 2 }\n",
                "func",
            ),
            (
                "main.c api.h",
                "C",
                "int main() { return 1; }\n",
                "int main() { return 2; }\n",
                "return",
            ),
            (
                "main.cpp main.cc main.cxx main.C api.hpp api.hh api.hxx api.H main.c++ api.h++",
                "C++",
                "class Before { public: int value; };\n",
                "class After { public: int value; };\n",
                "class",
            ),
            (
                "index.html index.htm",
                "HTML",
                "<button title=\"before\">Old</button>\n",
                "<button title=\"after\">New</button>\n",
                "button",
            ),
            (
                "styles.css",
                "CSS",
                "button { color: red; }\n",
                "button { color: blue; }\n",
                "button",
            ),
            (
                "package.json",
                "JSON",
                "{\"enabled\": false, \"count\": 1}\n",
                "{\"enabled\": true, \"count\": 2}\n",
                "enabled",
            ),
            (
                "config.yaml config.yml",
                "YAML",
                "enabled: false\n",
                "enabled: true\n",
                "enabled",
            ),
        ] {
            for path in paths.split_whitespace() {
                assert_eq!(
                    syntax_for_path(path).map(|s| s.name.as_str()),
                    Some(name),
                    "{path}"
                );
                let mut diff = rust_diff();
                let (hunks, truncated) = super::super::model::diff_text(old, new);
                diff.files[0].path = path.into();
                diff.files[0].content = FileContent::Text { hunks, truncated };
                let highlights = highlight(&diff);
                let FileContent::Text { hunks, .. } = &diff.files[0].content else {
                    unreachable!()
                };
                for dark in [false, true] {
                    for (index, line) in hunks[0].lines.iter().enumerate() {
                        assert!(matches!(line.tag, LineTag::Addition | LineTag::Deletion));
                        let spans = highlights.line(dark, 0, 0, index);
                        let start = line.text.find(token).unwrap();
                        assert!(
                            spans.iter().any(|span| span.range.start <= start
                                && span.range.end >= start + token.len()),
                            "{path} dark={dark} {:?}: {token} must be colored: {spans:?}",
                            line.tag
                        );
                        for span in spans {
                            assert!(line.text.get(span.range.clone()).is_some());
                        }
                    }
                    assert!(
                        !highlight_source(name, new, dark).is_empty(),
                        "{name} Kit highlighting"
                    );
                }
                assert_ne!(
                    highlights.line(false, 0, 0, 0),
                    highlights.line(true, 0, 0, 0),
                    "{path} theme colors"
                );
            }
        }
    }

    #[test]
    fn typescript_and_tsx_highlight_both_sides_in_both_themes() {
        for (path, old, new, tokens) in [
            (
                "src/main.ts",
                "export type Label = \"before\";\n",
                "export type Label = \"after\";\n",
                ["export", "type"],
            ),
            (
                "src/App.tsx",
                "export const view = <button title=\"before\">Hello</button>;\n",
                "export const view = <button title=\"after\">Hello</button>;\n",
                ["export", "button"],
            ),
        ] {
            let mut diff = rust_diff();
            let (hunks, truncated) = super::super::model::diff_text(old, new);
            diff.files[0].path = path.into();
            diff.files[0].content = FileContent::Text { hunks, truncated };
            let highlights = highlight(&diff);
            let FileContent::Text { hunks, .. } = &diff.files[0].content else {
                unreachable!();
            };
            for dark in [false, true] {
                for (line_index, line) in hunks[0].lines.iter().enumerate() {
                    let spans = highlights.line(dark, 0, 0, line_index);
                    let literal = match line.tag {
                        LineTag::Deletion => "before",
                        LineTag::Addition => "after",
                        LineTag::Context => unreachable!("fixture changes every line"),
                    };
                    for token in tokens.into_iter().chain([literal]) {
                        let start = line.text.find(token).unwrap();
                        assert!(
                            spans.iter().any(|span| {
                                span.range.start <= start && span.range.end >= start + token.len()
                            }),
                            "{path} {dark:?} {:?}: {token} must be colored, got {spans:?}",
                            line.tag
                        );
                    }
                }
            }
            assert_ne!(
                highlights.line(false, 0, 0, 0),
                highlights.line(true, 0, 0, 0),
                "{path} must use the selected theme"
            );
        }
    }

    #[test]
    fn kit_highlights_typescript_and_tsx_with_valid_utf8_offsets() {
        for (language, source, keyword) in [
            (
                "TypeScript",
                "/* café\r\n続き */\r\nexport type Label = \"日本語\";",
                "type",
            ),
            (
                "TypeScriptReact",
                "/* café\r\n続き */\r\nexport const view = <button title=\"日本語\" />;",
                "button",
            ),
        ] {
            for dark in [false, true] {
                let spans = highlight_source(language, source, dark);
                let start = source.find(keyword).unwrap();
                assert!(spans.iter().any(|(range, _)| {
                    range.start <= start && range.end >= start + keyword.len()
                }));
                assert!(spans.iter().any(|(range, _)| {
                    source
                        .get(range.clone())
                        .is_some_and(|text| text.contains("日本語"))
                }));
                for (range, _) in spans {
                    let text = source.get(range).expect("UTF-8 byte-aligned span");
                    assert!(!text.contains(['\r', '\n']));
                }
            }
        }
    }

    #[test]
    fn kit_hunk_offsets_match_review_with_unicode_crlf_and_multiline_syntax() {
        let source = "/* café\r\n続き */\r\nfn main() { let value = \"日本語\"; }";
        let (hunks, _) = super::super::model::diff_text("", source);
        let review = highlight_hunks(&hunks, syntax_by_name("Rust").unwrap());
        for dark in [false, true] {
            let actual = highlight_source("Rust", source, dark);
            let mut expected = Vec::new();
            let mut offset = 0;
            for (line, raw) in source.split_inclusive('\n').enumerate() {
                expected.extend(
                    styled_highlights(review.document_line(dark, 0, line))
                        .into_iter()
                        .map(|(range, style)| (range.start + offset..range.end + offset, style)),
                );
                offset += raw.len();
            }
            assert_eq!(actual, expected);
            assert!(actual.iter().any(|(range, _)| {
                source
                    .get(range.clone())
                    .is_some_and(|text| text.contains("日本語"))
            }));
            for (range, _) in actual {
                let text = source.get(range).expect("UTF-8 byte-aligned span");
                assert!(!text.contains(['\r', '\n']));
            }
        }
        assert_ne!(
            highlight_source("Rust", source, false),
            highlight_source("Rust", source, true),
        );
    }

    #[test]
    fn kit_hunks_reset_parser_state_and_unknown_languages_stay_plain() {
        for dark in [false, true] {
            let _ = highlight_source("Rust", "/* unfinished comment", dark);
            let fresh = highlight_source("Rust", "fn clean() {}", dark);
            assert!(fresh.iter().any(|(range, _)| range == &(0..2)));
            assert!(highlight_source("unknown-language", "fn clean() {}", dark).is_empty());
            assert!(highlight_source("Plain Text", "fn clean() {}", dark).is_empty());
        }
    }

    #[test]
    fn caches_both_sides_and_both_appearance_variants() {
        let highlights = highlight(&rust_diff());
        for dark in [false, true] {
            for line in [0, 1] {
                let spans = highlights.line(dark, 0, 0, line);
                assert!(
                    spans
                        .iter()
                        .any(|span| span.range.start == 0 && span.range.end >= 2),
                    "Rust's `fn` keyword should be highlighted"
                );
            }
        }
        assert_ne!(
            highlights.line(false, 0, 0, 1)[0].rgba,
            highlights.line(true, 0, 0, 1)[0].rgba,
        );
    }

    fn python_diff() -> ReviewDiff {
        ReviewDiff {
            files: vec![ChangedFile {
                path: "scripts/foo.py".into(),
                old_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                content: FileContent::Text {
                    hunks: vec![Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        collapsed_before: 0,
                        lines: vec![
                            HunkLine {
                                tag: LineTag::Deletion,
                                old_no: Some(1),
                                new_no: None,
                                text: "def old():".into(),
                                ending: "\n",
                            },
                            HunkLine {
                                tag: LineTag::Addition,
                                old_no: None,
                                new_no: Some(1),
                                text: "def new():".into(),
                                ending: "\n",
                            },
                        ],
                    }],
                    truncated: false,
                },
            }],
            file_versions: Default::default(),
            base_commit: String::new(),
            head_commit: String::new(),
            base_ref: None,
            head_branch: None,
        }
    }

    #[test]
    fn python_files_highlight_in_both_appearance_variants() {
        let highlights = highlight(&python_diff());
        for dark in [false, true] {
            for line in [0, 1] {
                let spans = highlights.line(dark, 0, 0, line);
                assert!(
                    spans
                        .iter()
                        .any(|span| span.range.start == 0 && span.range.end >= 3),
                    "Python's `def` keyword should be highlighted"
                );
            }
        }
        assert_ne!(
            highlights.line(false, 0, 0, 1)[0].rgba,
            highlights.line(true, 0, 0, 1)[0].rgba,
        );
    }

    fn swift_diff() -> ReviewDiff {
        ReviewDiff {
            files: vec![ChangedFile {
                path: "Sources/ContentView.swift".into(),
                old_path: None,
                status: FileStatus::Modified,
                additions: 1,
                deletions: 1,
                content: FileContent::Text {
                    hunks: vec![Hunk {
                        old_start: 1,
                        old_lines: 1,
                        new_start: 1,
                        new_lines: 1,
                        collapsed_before: 0,
                        lines: vec![
                            HunkLine {
                                tag: LineTag::Deletion,
                                old_no: Some(1),
                                new_no: None,
                                text: "func old() {}".into(),
                                ending: "\n",
                            },
                            HunkLine {
                                tag: LineTag::Addition,
                                old_no: None,
                                new_no: Some(1),
                                text: "func new() {}".into(),
                                ending: "\n",
                            },
                        ],
                    }],
                    truncated: false,
                },
            }],
            file_versions: Default::default(),
            base_commit: String::new(),
            head_commit: String::new(),
            base_ref: None,
            head_branch: None,
        }
    }

    #[test]
    fn swift_files_highlight_in_both_appearance_variants() {
        assert_eq!(
            syntax_for_path("Sources/ContentView.swift").unwrap().name,
            "Swift"
        );
        let highlights = highlight(&swift_diff());
        for dark in [false, true] {
            for line in [0, 1] {
                let spans = highlights.line(dark, 0, 0, line);
                assert!(
                    spans
                        .iter()
                        .any(|span| span.range.start == 0 && span.range.end >= 4),
                    "Swift's `func` keyword should be highlighted"
                );
            }
        }
        assert_ne!(
            highlights.line(false, 0, 0, 1)[0].rgba,
            highlights.line(true, 0, 0, 1)[0].rgba,
        );
    }

    #[test]
    fn looks_up_syntaxes_by_name_and_lists_them_for_a_picker() {
        assert_eq!(syntax_by_name("JSON").unwrap().name, "JSON");
        assert!(syntax_by_name("No Such Language").is_none());
        let names = syntax_names();
        assert!(names.contains(&"Rust"), "{names:?}");
        assert!(names.contains(&"Plain Text"), "{names:?}");
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, names, "the picker list must be sorted and unique");
    }

    #[test]
    fn single_document_highlighting_uses_the_review_machine() {
        let diff = rust_diff();
        let FileContent::Text { hunks, .. } = &diff.files[0].content else {
            panic!("the fixture must be a text diff");
        };
        let highlights = highlight_hunks(hunks, syntax_by_name("Rust").unwrap());
        for dark in [false, true] {
            for line in [0, 1] {
                let spans = highlights.document_line(dark, 0, line);
                assert!(
                    spans
                        .iter()
                        .any(|span| span.range.start == 0 && span.range.end >= 2),
                    "Rust's `fn` keyword should be highlighted"
                );
            }
        }
        // Indices past the document are empty, not panics.
        assert!(highlights.document_line(true, 9, 0).is_empty());
        assert!(highlights.document_line(true, 0, 9).is_empty());
    }

    #[test]
    fn unknown_file_types_fall_back_to_plain_text() {
        let mut diff = rust_diff();
        diff.files[0].path = "data.unknown-extension".into();
        let highlights = highlight(&diff);
        assert!(highlights.line(false, 0, 0, 0).is_empty());
        assert!(highlights.line(true, 0, 0, 1).is_empty());
    }
}
