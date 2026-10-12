//! Bridge the bounded display model to a valid upstream patch document.
//!
//! Raw source is used for line bytes: the display model strips CR and does not
//! retain the final-newline bit. Only complete retained hunks are serialized.

use std::fmt::Write as _;

use gpui_kit::component::diff::DiffFile;

use super::model::{ChangedFile, FileContent, FileStatus, LineTag};

pub(crate) fn parse_file(
    file: &ChangedFile,
    old: &str,
    new: &str,
    modes: [&str; 2],
) -> Result<Vec<DiffFile>, String> {
    let FileContent::Text { .. } = &file.content else {
        return Ok(vec![]);
    };
    let patch = serialize(file, old, new, modes)?;
    let files =
        DiffFile::parse(&patch).map_err(|error| format!("Could not prepare diff: {error}"))?;
    if files.len() != 1 || files[0].path().as_str() != file.path {
        return Err("Could not prepare the selected file's diff".into());
    }
    Ok(files)
}

fn serialize(file: &ChangedFile, old: &str, new: &str, modes: [&str; 2]) -> Result<String, String> {
    let FileContent::Text { hunks, .. } = &file.content else {
        return Err("Text content required for a patch".into());
    };
    let original = file.old_path.as_deref().unwrap_or(&file.path);
    let old_path = quote_path(&format!("a/{original}"));
    let new_path = quote_path(&format!("b/{}", file.path));
    let mut patch = format!("diff --git {old_path} {new_path}\n");
    match file.status {
        FileStatus::Added => writeln!(patch, "new file mode {}", modes[1]).unwrap(),
        FileStatus::Deleted => writeln!(patch, "deleted file mode {}", modes[0]).unwrap(),
        FileStatus::Renamed => {
            writeln!(patch, "rename from {}", quote_path(original)).unwrap();
            writeln!(patch, "rename to {}", quote_path(&file.path)).unwrap();
        }
        _ => {}
    }
    if modes[0] != modes[1] && !matches!(file.status, FileStatus::Added | FileStatus::Deleted) {
        writeln!(patch, "old mode {}\nnew mode {}", modes[0], modes[1]).unwrap();
    }
    writeln!(
        patch,
        "--- {}\n+++ {}",
        if file.status == FileStatus::Added {
            "/dev/null"
        } else {
            &old_path
        },
        if file.status == FileStatus::Deleted {
            "/dev/null"
        } else {
            &new_path
        },
    )
    .unwrap();
    // Walk each side once rather than allocating an index for every source
    // line. Large files may have millions of lines but only 4,000 retained.
    let mut sources = [SourceLines::new(old), SourceLines::new(new)];
    for hunk in hunks {
        // The model stores empty spans one-based; unified patches store the
        // line immediately before an empty span, including zero at file start.
        let old_start = hunk
            .old_start
            .saturating_sub(u32::from(hunk.old_lines == 0));
        let new_start = hunk
            .new_start
            .saturating_sub(u32::from(hunk.new_lines == 0));
        writeln!(
            patch,
            "@@ -{old_start},{} +{new_start},{} @@",
            hunk.old_lines, hunk.new_lines
        )
        .unwrap();
        for line in &hunk.lines {
            let (prefix, side, number) = match line.tag {
                LineTag::Context => (' ', 1, line.new_no),
                LineTag::Deletion => ('-', 0, line.old_no),
                LineTag::Addition => ('+', 1, line.new_no),
            };
            let source = number
                .and_then(|number| sources[side].line(number))
                .ok_or("Diff line does not match its source")?;
            patch.push(prefix);
            patch.push_str(source);
            if !source.ends_with('\n') {
                patch.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    Ok(patch)
}

struct SourceLines<'a> {
    lines: std::str::SplitInclusive<'a, char>,
    consumed: u32,
}

impl<'a> SourceLines<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            lines: text.split_inclusive('\n'),
            consumed: 0,
        }
    }

    fn line(&mut self, number: u32) -> Option<&'a str> {
        let skip = number.checked_sub(self.consumed)?.checked_sub(1)?;
        let line = self.lines.nth(skip as usize)?;
        self.consumed = number;
        Some(line)
    }
}

fn quote_path(path: &str) -> String {
    let mut quoted = String::from("\"");
    for byte in path.bytes() {
        match byte {
            b'"' => quoted.push_str("\\\""),
            b'\\' => quoted.push_str("\\\\"),
            32..=126 => quoted.push(byte as char),
            _ => write!(quoted, "\\{byte:03o}").unwrap(),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::super::model::{MAX_HUNK_LINES_PER_FILE, diff_text};
    use super::*;
    use gpui_kit::{
        AppContext as _,
        component::diff::{DiffFileStatus, DiffLineRange, DiffSide, DiffState},
    };

    fn changed(old: &str, new: &str, status: FileStatus) -> ChangedFile {
        let (hunks, truncated) = diff_text(old, new);
        let (additions, deletions) = super::super::model::count_changes(old, new);
        ChangedFile {
            path: "file.txt".into(),
            old_path: None,
            status,
            additions,
            deletions,
            content: FileContent::Text { hunks, truncated },
        }
    }

    #[test]
    fn empty_ranges_and_empty_files_preserve_status_and_coordinates() {
        for (old, new, status, expected, modes, header) in [
            (
                "",
                "new\n",
                FileStatus::Added,
                DiffFileStatus::Added,
                ["000000", "100644"],
                "@@ -0,0 +1,1 @@",
            ),
            (
                "old\n",
                "",
                FileStatus::Deleted,
                DiffFileStatus::Deleted,
                ["100644", "000000"],
                "@@ -1,1 +0,0 @@",
            ),
            (
                "",
                "",
                FileStatus::Added,
                DiffFileStatus::Added,
                ["000000", "100644"],
                "new file mode 100644",
            ),
            (
                "",
                "",
                FileStatus::Deleted,
                DiffFileStatus::Deleted,
                ["100644", "000000"],
                "deleted file mode 100644",
            ),
        ] {
            let file = changed(old, new, status);
            assert!(serialize(&file, old, new, modes).unwrap().contains(header));
            let parsed = parse_file(&file, old, new, modes).unwrap();
            assert_eq!(parsed[0].status(), expected);
            assert_eq!(
                (parsed[0].additions(), parsed[0].deletions()),
                (file.additions as usize, file.deletions as usize)
            );
            assert!(parsed[0].has_changes());
        }
        // An empty range in the middle of a file also uses the preceding line.
        let mut file = changed(
            "first\nlast\n",
            "first\ninsert\nlast\n",
            FileStatus::Modified,
        );
        let FileContent::Text { hunks, .. } = &mut file.content else {
            unreachable!()
        };
        let insertion = hunks[0]
            .lines
            .iter()
            .find(|line| line.tag == LineTag::Addition)
            .unwrap()
            .clone();
        hunks[0].old_start = 2;
        hunks[0].old_lines = 0;
        hunks[0].new_start = 2;
        hunks[0].new_lines = 1;
        hunks[0].lines = vec![insertion];
        let patch = serialize(
            &file,
            "first\nlast\n",
            "first\ninsert\nlast\n",
            ["100644"; 2],
        )
        .unwrap();
        assert!(patch.contains("@@ -1,0 +2,1 @@"));
        assert_eq!(DiffFile::parse(&patch).unwrap()[0].additions(), 1);
    }

    #[test]
    fn quoted_unicode_rename_and_mode_only_metadata_survive() {
        let mut file = changed("same\n", "same\n", FileStatus::Renamed);
        file.path = "new/বাংলা \"name\"\t\n\\.rs".into();
        file.old_path = Some("old/বাংলা \"name\"\t\n\\.rs".into());
        let parsed = parse_file(&file, "same\n", "same\n", ["100644", "100755"]).unwrap();
        assert_eq!(parsed[0].path().as_str(), file.path);
        assert_eq!(
            parsed[0].original_path().unwrap().as_str(),
            file.old_path.as_ref().unwrap()
        );
        assert_eq!(parsed[0].status(), DiffFileStatus::Renamed);
        assert!(
            parsed[0]
                .extended_headers()
                .iter()
                .any(|header| header.as_str() == "new mode 100755")
        );
        assert!(parsed[0].has_changes());
        let mode_only = changed("same\n", "same\n", FileStatus::Modified);
        assert!(
            parse_file(&mode_only, "same\n", "same\n", ["100644", "100755"]).unwrap()[0]
                .has_changes()
        );
    }

    #[gpui_kit::test]
    fn raw_source_copy_preserves_crlf_eof_and_hunk_source_numbers(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        for (old, new) in [
            ("α\r\nold\r\nend", "α\r\nnew\r\nend"),
            ("same\n", "same"),
            ("same", "same\n"),
            ("old", "new"),
        ] {
            let file = changed(old, new, FileStatus::Modified);
            let files = parse_file(&file, old, new, ["100644"; 2]).unwrap();
            cx.update(|cx| {
                let state = cx.new(|cx| DiffState::new(files, cx));
                state.update(cx, |state, cx| {
                    for (side, source) in [(DiffSide::Original, old), (DiffSide::Modified, new)] {
                        state.set_selected_lines(
                            Some(DiffLineRange::new(
                                "file.txt",
                                side,
                                1,
                                source.lines().count(),
                            )),
                            cx,
                        );
                        assert_eq!(state.selected_text(cx), source);
                    }
                });
            });
        }
        let old = (1..=60).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old.replace("line 30\n", "changed 30\n");
        let file = changed(&old, &new, FileStatus::Modified);
        let files = parse_file(&file, &old, &new, ["100644"; 2]).unwrap();
        cx.update(|cx| {
            let state = cx.new(|cx| DiffState::new(files, cx));
            state.update(cx, |state, cx| {
                state.set_selected_lines(
                    Some(DiffLineRange::new("file.txt", DiffSide::Modified, 30, 30)),
                    cx,
                );
                assert_eq!(state.selected_text(cx), "changed 30\n");
                state.set_selected_lines(
                    Some(DiffLineRange::new("file.txt", DiffSide::Original, 30, 30)),
                    cx,
                );
                assert_eq!(state.selected_text(cx), "line 30\n");
            });
        });
    }

    #[test]
    fn truncation_keeps_complete_hunks_and_rejects_missing_source() {
        let old = (1..=60).map(|n| format!("line {n}\n")).collect::<String>();
        let new = format!(
            "{}{}",
            old.replace("line 1\n", "changed 1\n"),
            "added\n".repeat(MAX_HUNK_LINES_PER_FILE + 1)
        );
        let file = changed(&old, &new, FileStatus::Modified);
        let FileContent::Text { hunks, truncated } = &file.content else {
            unreachable!()
        };
        assert!(*truncated);
        assert_eq!(hunks.len(), 1);
        let parsed = parse_file(&file, &old, &new, ["100644"; 2]).unwrap();
        assert_eq!(parsed[0].additions(), 1);
        assert!(parsed[0].additions() < file.additions as usize);
        assert!(
            parse_file(&file, "", "", ["100644"; 2])
                .err()
                .unwrap()
                .contains("does not match its source")
        );
    }
}
