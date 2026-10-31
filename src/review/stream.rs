//! Logical diff rows for comment anchor lookup and Review file headers.
//!
//! Kit owns the virtualized code display. The bounded logical rows keep comment
//! placement independent of the current unified/split layout. Shared messages
//! and colors are also used by the text comparison tool.

use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use gpui_kit::component::{
    IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Entity, Hsla, InteractiveElement, IntoElement, MouseButton,
    ParentElement, Styled, div, px, rgb,
};

use super::ReviewView;
use super::model::{ChangedFile, FileStatus, ReviewDiff, UnavailableReason};

/// Minimum height of a diff content row. File headers use [`FILE_HEADER_H`] so
/// they stand out from code lines (see module docs).
pub(crate) const ROW_H: f32 = 24.0;
/// Tint of a removed line. Shared so the diff tool's cells read as the same
/// kind of change the review rows do.
pub(crate) const DELETION_BG: u32 = 0x33191a;
/// Tint of an added line, shared the same way.
pub(crate) const ADDITION_BG: u32 = 0x0e2a1a;
/// Height of file section headers. Taller than [`ROW_H`] so each file
/// boundary is noticeable while scrolling.
pub(crate) const FILE_HEADER_H: f32 = 36.0;
/// The space above a header's content at the top of the diff: the Diff
/// border, Kit's `py_1` header padding, and the separator Kit adds above
/// every file after the first. Content tops within this distance of the pane
/// top belong to a header sitting at the top.
const FILE_HEADER_TOP_SLACK: f32 = 6.5;
/// Tolerance for comparing prepainted header and pane positions.
const STICKY_HEADER_EPSILON: f32 = 0.5;

/// Finds the file whose header should stay pinned above the Review diff.
///
/// Kit owns the diff's scroll state and exposes no viewport or scroll getter,
/// so Review observes the headers it renders instead: each header records its
/// window position while Kit prepaints it. [`Self::begin_frame`] runs before
/// Kit lays out the diff, and [`Self::resolve`] runs afterwards from the diff
/// wrapper, where the pane's own position is known.
#[derive(Default)]
pub(crate) struct HeaderSpy {
    painted: HashSet<String>,
    tops: HashMap<String, f32>,
}

impl HeaderSpy {
    /// Forgets the previous frame's painted headers. Called before Kit
    /// prepaints the diff so headers scrolled out of the render window are
    /// not mistaken for their last painted position.
    pub(crate) fn begin_frame(&mut self) {
        self.painted.clear();
    }

    pub(crate) fn record(&mut self, path: &str, content_top: f32) {
        self.painted.insert(path.to_owned());
        self.tops.insert(path.to_owned(), content_top);
    }

    /// Drops positions from a previous review. Paths that survive a branch or
    /// scope switch would otherwise map to unrelated files.
    pub(crate) fn clear(&mut self) {
        self.painted.clear();
        self.tops.clear();
    }

    /// Resolves the file at the top of the diff.
    ///
    /// Prefers the lowest painted header at or above the pane top. When only
    /// headers below the top are painted, the previous file still owns the
    /// viewport. A long file can span several screens without painting any
    /// header, so `current` is retained in that case.
    pub(crate) fn resolve(
        &self,
        pane_top: f32,
        files: &[ChangedFile],
        current: Option<&str>,
    ) -> Option<String> {
        let mut above: Option<(f32, &str)> = None;
        let mut first_below: Option<(f32, &str)> = None;
        for path in &self.painted {
            let Some(content_top) = self.tops.get(path).copied() else {
                continue;
            };
            if content_top <= pane_top + FILE_HEADER_TOP_SLACK + STICKY_HEADER_EPSILON {
                if above.is_none_or(|(top, _)| content_top > top) {
                    above = Some((content_top, path));
                }
            } else if first_below.is_none_or(|(top, _)| content_top < top) {
                first_below = Some((content_top, path));
            }
        }
        if let Some((_, path)) = above {
            return Some(path.to_owned());
        }
        if let Some((_, path)) = first_below
            && let Some(ix) = files.iter().position(|file| file.path == path)
        {
            return Some(files[ix.saturating_sub(1)].path.clone());
        }
        current
            .filter(|path| files.iter().any(|file| file.path == *path))
            .map(str::to_owned)
            .or_else(|| files.first().map(|file| file.path.clone()))
    }
}

/// One virtualized row of the diff stream, addressing file/hunk/line by
/// index into a [`LoadedReview`](super::LoadedReview).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamRow {
    FileHeader {
        file: usize,
    },
    HunkHeader {
        file: usize,
        hunk: usize,
    },
    Line {
        file: usize,
        hunk: usize,
        line: usize,
    },
    Collapsed {
        file: usize,
        hunk: usize,
    },
    Truncated {
        file: usize,
    },
    Unavailable {
        file: usize,
    },
    NoChanges {
        file: usize,
    },
}

impl StreamRow {
    pub(crate) fn file(self) -> usize {
        match self {
            Self::FileHeader { file }
            | Self::HunkHeader { file, .. }
            | Self::Line { file, .. }
            | Self::Collapsed { file, .. }
            | Self::Truncated { file }
            | Self::Unavailable { file }
            | Self::NoChanges { file } => file,
        }
    }
}

/// Flatten a diff into rows plus each file's starting row.
///
/// Row order mirrors the input file order, so callers must pass files in
/// [`compare_review_paths`](super::model::compare_review_paths) order to
/// match the sidebar. Files in `collapsed` (by repo-relative path) emit only
/// their [`StreamRow::FileHeader`]; expanding re-flattens to restore content.
pub(crate) fn flatten(
    diff: &ReviewDiff,
    collapsed: &HashSet<String>,
) -> (Vec<StreamRow>, Vec<usize>) {
    let mut rows = Vec::new();
    let mut file_row_start = Vec::with_capacity(diff.files.len());
    for (file_ix, file) in diff.files.iter().enumerate() {
        file_row_start.push(rows.len());
        rows.push(StreamRow::FileHeader { file: file_ix });
        if collapsed.contains(&file.path) {
            continue;
        }
        match &file.content {
            super::model::FileContent::Text { hunks, truncated } => {
                for (hunk_ix, hunk) in hunks.iter().enumerate() {
                    if hunk.collapsed_before > 0 {
                        rows.push(StreamRow::Collapsed {
                            file: file_ix,
                            hunk: hunk_ix,
                        });
                    }
                    rows.push(StreamRow::HunkHeader {
                        file: file_ix,
                        hunk: hunk_ix,
                    });
                    for (line_ix, _) in hunk.lines.iter().enumerate() {
                        rows.push(StreamRow::Line {
                            file: file_ix,
                            hunk: hunk_ix,
                            line: line_ix,
                        });
                    }
                }
                if *truncated {
                    rows.push(StreamRow::Truncated { file: file_ix });
                }
                if hunks.is_empty() && !truncated {
                    rows.push(StreamRow::NoChanges { file: file_ix });
                }
            }
            super::model::FileContent::Unavailable(_) => {
                rows.push(StreamRow::Unavailable { file: file_ix });
            }
        }
    }
    (rows, file_row_start)
}

pub(crate) fn status_color(status: FileStatus) -> u32 {
    match status {
        FileStatus::Added => 0x4ade80,
        FileStatus::Modified => 0xfbbf24,
        FileStatus::Deleted => 0xf87171,
        FileStatus::Renamed => 0x7dd3fc,
        FileStatus::TypeChanged => 0xc4b5fd,
    }
}

pub(crate) fn file_header_row(
    file: &ChangedFile,
    is_viewed: bool,
    view: &Entity<ReviewView>,
    editor_open_available: bool,
    spy: &Rc<RefCell<HeaderSpy>>,
) -> AnyElement {
    let path = file.path.clone();
    let spy = spy.clone();
    div()
        .w_full()
        // Kit renders headers inside the horizontally scrolled diff content.
        // Record their vertical window position so Review can pin the header
        // for the file currently at the top of the pane.
        .on_children_prepainted(move |children, _, _| {
            if let Some(bounds) = children.first() {
                spy.borrow_mut().record(&path, f32::from(bounds.origin.y));
            }
        })
        .child(file_header_content(
            file,
            is_viewed,
            view,
            editor_open_available,
            true,
            None,
        ))
        .into_any_element()
}

/// Theme values and separator state for a header rendered outside Kit's diff.
#[derive(Clone, Copy)]
pub(crate) struct HeaderShell {
    pub background: Hsla,
    pub border: Hsla,
    /// Kit adds a separator above every file after the first.
    pub top_border: bool,
}

/// The header that stays pinned above the diff while its file is in view.
///
/// Unlike [`file_header_row`], it is rendered outside Kit's horizontal
/// scroll, so its path and viewed toggle remain reachable without scrolling
/// sideways. The shell copies Kit's header row — same padding, collapse
/// button, borders, and background — so pinned and in-flow headers read as
/// one control.
pub(crate) fn sticky_header_row(
    file: &ChangedFile,
    is_viewed: bool,
    is_collapsed: bool,
    shell: HeaderShell,
    view: &Entity<ReviewView>,
    editor_open_available: bool,
) -> AnyElement {
    let path = file.path.clone();
    let toggle_view = view.clone();
    h_flex()
        .w_full()
        .min_w_0()
        .pl_1()
        .pr_3()
        .py_1()
        .gap_1()
        .when(shell.top_border, |this| this.border_t_1())
        .bg(shell.background)
        .border_b_1()
        .border_color(shell.border)
        .child(
            Button::new("review-sticky-collapse")
                .ghost()
                .xsmall()
                .icon(if is_collapsed {
                    IconName::ChevronRight
                } else {
                    IconName::ChevronDown
                })
                .accessibility_label(if is_collapsed {
                    "Expand file"
                } else {
                    "Collapse file"
                })
                .on_click(move |_, _, cx: &mut App| {
                    cx.update_entity(&toggle_view, |view, cx| {
                        view.toggle_collapsed(&path, cx);
                    });
                }),
        )
        .child(div().flex_1().min_w_0().child(file_header_content(
            file,
            is_viewed,
            view,
            editor_open_available,
            true,
            Some("review-sticky-viewed"),
        )))
        .into_any_element()
}

fn file_header_content(
    file: &ChangedFile,
    is_viewed: bool,
    view: &Entity<ReviewView>,
    editor_open_available: bool,
    chrome: bool,
    viewed_selector: Option<&'static str>,
) -> impl IntoElement {
    let path = file.path.clone();
    let toggle_view = view.clone();
    let open_view = view.clone();
    let open_path = file.path.clone();
    let stats = if file.additions > 0 || file.deletions > 0 {
        format!("+{} −{}", file.additions, file.deletions)
    } else {
        String::new()
    };
    let title = match (&file.status, &file.old_path) {
        (FileStatus::Renamed, Some(old)) => format!("{old} → {}", file.path),
        _ => file.path.clone(),
    };
    div()
        .h(px(FILE_HEADER_H))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px_3()
        .gap_2()
        .when(chrome, |this| {
            this.bg(rgb(0x151a1a))
                .border_b_1()
                .border_color(rgb(0x2a2e2e))
        })
        .child(
            div()
                .flex_none()
                .text_xs()
                .font_semibold()
                .text_color(rgb(status_color(file.status)))
                .child(file.status.label()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .text_sm()
                .font_semibold()
                .child(title),
        )
        .child(
            div()
                .flex_none()
                .text_xs()
                .text_color(rgb(0x858989))
                .child(stats),
        )
        .when(editor_open_available, |this| {
            this.child(
                div()
                    .flex_none()
                    .text_xs()
                    .cursor_pointer()
                    .text_color(rgb(0x7dd3fc))
                    .child("Open in Editor")
                    .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                        cx.update_entity(&open_view, |view, cx| {
                            view.open_file(&open_path, None, cx);
                        });
                        cx.stop_propagation();
                    }),
            )
        })
        .child(
            div()
                .flex_none()
                .text_xs()
                .cursor_pointer()
                .text_color(if is_viewed {
                    rgb(0x4ade80)
                } else {
                    rgb(0x555a5a)
                })
                .when_some(viewed_selector, |this, selector| {
                    this.debug_selector(move || selector.to_string())
                })
                .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                    cx.update_entity(&toggle_view, |view, cx| {
                        view.toggle_viewed(&path, cx);
                    });
                    cx.stop_propagation();
                })
                .child(if is_viewed { "✓" } else { "○" }),
        )
}

pub(super) fn unavailable_row(reason: UnavailableReason) -> AnyElement {
    div()
        .h(px(ROW_H))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px_3()
        .text_color(rgb(0x737878))
        .child(format!("○ {}", reason.label()))
        .into_any_element()
}

pub(crate) fn no_changes_row() -> AnyElement {
    div()
        .h(px(ROW_H))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px_3()
        .text_color(rgb(0x555a5a))
        .text_sm()
        .child("No line changes")
        .into_any_element()
}

pub(crate) fn truncated_row() -> AnyElement {
    div()
        .h(px(ROW_H))
        .flex_none()
        .flex()
        .flex_row()
        .items_center()
        .px_3()
        .text_color(rgb(0xd9a648))
        .child("Diff truncated: only complete hunks within the 4,000-line display limit are shown.")
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::super::model::{
        ChangedFile, FileContent, FileStatus, Hunk, HunkLine, LineTag, ReviewDiff,
    };
    use super::*;

    fn line(tag: LineTag) -> HunkLine {
        HunkLine {
            tag,
            old_no: Some(1),
            new_no: Some(1),
            text: "x".to_owned(),
            ending: "\n",
        }
    }

    #[test]
    fn flatten_assigns_every_row_and_file_start() {
        let diff = ReviewDiff {
            files: vec![
                ChangedFile {
                    path: "a.rs".to_owned(),
                    old_path: None,
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 1,
                    content: FileContent::Text {
                        hunks: vec![Hunk {
                            old_start: 1,
                            old_lines: 3,
                            new_start: 1,
                            new_lines: 3,
                            collapsed_before: 10,
                            lines: vec![
                                line(LineTag::Context),
                                line(LineTag::Deletion),
                                line(LineTag::Addition),
                            ],
                        }],
                        truncated: false,
                    },
                },
                ChangedFile {
                    path: "b.bin".to_owned(),
                    old_path: None,
                    status: FileStatus::Added,
                    additions: 0,
                    deletions: 0,
                    content: FileContent::Unavailable(UnavailableReason::Binary),
                },
            ],
            file_versions: Default::default(),
            base_commit: String::new(),
            head_commit: String::new(),
            base_ref: None,
            head_branch: None,
        };
        let (rows, starts) = flatten(&diff, &HashSet::new());
        assert_eq!(
            rows,
            vec![
                StreamRow::FileHeader { file: 0 },
                StreamRow::Collapsed { file: 0, hunk: 0 },
                StreamRow::HunkHeader { file: 0, hunk: 0 },
                StreamRow::Line {
                    file: 0,
                    hunk: 0,
                    line: 0
                },
                StreamRow::Line {
                    file: 0,
                    hunk: 0,
                    line: 1
                },
                StreamRow::Line {
                    file: 0,
                    hunk: 0,
                    line: 2
                },
                StreamRow::FileHeader { file: 1 },
                StreamRow::Unavailable { file: 1 },
            ]
        );
        assert_eq!(starts, vec![0, 6]);
        assert!(rows.iter().all(|row| row.file() < diff.files.len()));
    }

    #[test]
    fn flatten_collapsed_file_emits_only_its_header() {
        let diff = ReviewDiff {
            files: vec![
                ChangedFile {
                    path: "a.rs".to_owned(),
                    old_path: None,
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 1,
                    content: FileContent::Text {
                        hunks: vec![Hunk {
                            old_start: 1,
                            old_lines: 3,
                            new_start: 1,
                            new_lines: 3,
                            collapsed_before: 0,
                            lines: vec![line(LineTag::Context)],
                        }],
                        truncated: false,
                    },
                },
                ChangedFile {
                    path: "b.rs".to_owned(),
                    old_path: None,
                    status: FileStatus::Modified,
                    additions: 1,
                    deletions: 0,
                    content: FileContent::Text {
                        hunks: vec![Hunk {
                            old_start: 1,
                            old_lines: 1,
                            new_start: 1,
                            new_lines: 2,
                            collapsed_before: 0,
                            lines: vec![line(LineTag::Addition)],
                        }],
                        truncated: false,
                    },
                },
            ],
            file_versions: Default::default(),
            base_commit: String::new(),
            head_commit: String::new(),
            base_ref: None,
            head_branch: None,
        };
        let collapsed = HashSet::from(["a.rs".to_owned()]);
        let (rows, starts) = flatten(&diff, &collapsed);
        assert_eq!(
            rows,
            vec![
                StreamRow::FileHeader { file: 0 },
                StreamRow::FileHeader { file: 1 },
                StreamRow::HunkHeader { file: 1, hunk: 0 },
                StreamRow::Line {
                    file: 1,
                    hunk: 0,
                    line: 0
                },
            ]
        );
        assert_eq!(starts, vec![0, 1]);
    }

    fn changed_file(path: &str) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            old_path: None,
            status: FileStatus::Modified,
            additions: 1,
            deletions: 1,
            content: FileContent::Unavailable(UnavailableReason::Binary),
        }
    }

    fn files() -> Vec<ChangedFile> {
        ["a.rs", "b.rs", "c.rs"].map(changed_file).to_vec()
    }

    #[test]
    fn header_spy_prefers_the_lowest_header_at_the_viewport_top() {
        let files = files();
        let mut spy = HeaderSpy::default();
        spy.begin_frame();
        spy.record("a.rs", 104.);
        spy.record("b.rs", 604.);
        assert_eq!(spy.resolve(100., &files, None).as_deref(), Some("a.rs"));

        spy.begin_frame();
        spy.record("a.rs", -496.);
        spy.record("b.rs", 104.);
        assert_eq!(spy.resolve(100., &files, None).as_deref(), Some("b.rs"));
    }

    #[test]
    fn header_spy_keeps_the_previous_file_until_the_next_header_arrives() {
        let files = files();
        let mut spy = HeaderSpy::default();
        spy.begin_frame();
        spy.record("a.rs", -900.);
        spy.record("b.rs", 108.);
        assert_eq!(spy.resolve(100., &files, None).as_deref(), Some("a.rs"));

        // A long file can paint no headers at all; the resolved file must not
        // reset to the first file while scrolling inside it.
        spy.begin_frame();
        assert_eq!(
            spy.resolve(100., &files, Some("b.rs")).as_deref(),
            Some("b.rs")
        );
    }

    #[test]
    fn header_spy_drops_a_file_that_left_the_review() {
        let files = vec![changed_file("a.rs"), changed_file("b.rs")];
        let spy = HeaderSpy::default();
        assert_eq!(
            spy.resolve(100., &files, Some("removed.rs")).as_deref(),
            Some("a.rs")
        );
        assert_eq!(spy.resolve(100., &[], Some("a.rs")), None);
    }
}
