//! Retained GPUI Kit Diff prototype for immutable Git History content.

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    diff::{Diff, DiffFile, DiffMode, DiffState},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _,
    IntoElement, ParentElement as _, Render, Styled as _, TestSupportExt as _, Window, div, px,
};

use super::{
    model::{ChangedFile, FileContent},
    patch,
};
use crate::{fonts::TERMINAL_FONT_FAMILY, metrics::review_font_size};

pub(crate) struct PreparedKitDiff {
    pub(crate) file: ChangedFile,
    files: Vec<DiffFile>,
}

impl PreparedKitDiff {
    pub(crate) fn new(
        file: ChangedFile,
        old: &str,
        new: &str,
        modes: [&str; 2],
    ) -> Result<Self, String> {
        let files = patch::parse_file(&file, old, new, modes)?;
        Ok(Self { file, files })
    }
}

enum ViewerState {
    Empty,
    Loading(String),
    Loaded(ChangedFile),
    Failed { path: String, message: String },
}

pub(crate) struct KitDiffViewer {
    diff: Entity<DiffState>,
    state: ViewerState,
    wrap: bool,
}

impl KitDiffViewer {
    pub(crate) fn new(cx: &mut Context<Self>) -> Self {
        Self {
            diff: cx.new(|cx| DiffState::new([], cx).with_context_lines(None)),
            state: ViewerState::Empty,
            wrap: false,
        }
    }

    fn reset(&mut self, state: ViewerState, cx: &mut Context<Self>) {
        self.diff.update(cx, |diff, cx| diff.set_files([], cx));
        self.state = state;
        cx.notify();
    }

    pub(crate) fn clear(&mut self, cx: &mut Context<Self>) {
        self.reset(ViewerState::Empty, cx);
    }

    pub(crate) fn set_loading(&mut self, path: String, cx: &mut Context<Self>) {
        self.reset(ViewerState::Loading(path), cx);
    }

    pub(crate) fn set_loaded(&mut self, prepared: PreparedKitDiff, cx: &mut Context<Self>) {
        self.diff
            .update(cx, |diff, cx| diff.set_files(prepared.files, cx));
        self.state = ViewerState::Loaded(prepared.file);
        cx.notify();
    }

    pub(crate) fn set_error(&mut self, path: String, message: String, cx: &mut Context<Self>) {
        self.reset(ViewerState::Failed { path, message }, cx);
    }

    fn set_mode(&mut self, mode: DiffMode, window: &mut Window, cx: &mut Context<Self>) {
        self.diff.update(cx, |diff, cx| diff.set_mode(mode, cx));
        self.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn navigate(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.diff.update(cx, |diff, cx| {
            if forward {
                diff.next_change(cx)
            } else {
                diff.previous_change(cx)
            }
        });
        self.focus_handle(cx).focus(window, cx);
    }

    fn message(title: impl Into<String>, detail: impl Into<String>) -> gpui_kit::AnyElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(div().text_sm().font_semibold().child(title.into()))
            .child(
                div()
                    .text_sm()
                    .max_w(px(560.))
                    .text_center()
                    .child(detail.into()),
            )
            .into_any_element()
    }
}

impl Render for KitDiffViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = v_flex().size_full().min_h_0().min_w_0();
        match &self.state {
            ViewerState::Empty => {
                content = content.child(Self::message(
                    "Select a changed file",
                    "Choose a file from this commit.",
                ))
            }
            ViewerState::Loading(path) => {
                content = content.child(Self::message("Loading diff…", path.clone()))
            }
            ViewerState::Failed { path, message } => {
                content = content.child(Self::message(
                    format!("Could not load {path}"),
                    message.clone(),
                ))
            }
            ViewerState::Loaded(file) => {
                content = content.child(div().px_3().py_1().text_xs().child(format!(
                    "{} · +{} / −{}",
                    file.status.label(),
                    file.additions,
                    file.deletions
                )));
                if let FileContent::Unavailable(reason) = file.content {
                    content = content.child(Self::message(reason.label(), file.path.clone()));
                } else {
                    let mode = self.diff.read(cx).mode();
                    let has_hunks = matches!(&file.content, FileContent::Text { hunks, .. } if !hunks.is_empty());
                    let no_retained_hunks = !has_hunks
                        && matches!(
                            file.content,
                            FileContent::Text {
                                truncated: true,
                                ..
                            }
                        );
                    content =
                        content
                            .child(
                                h_flex()
                                    .flex_wrap()
                                    .gap_1()
                                    .px_2()
                                    .py_1()
                                    .child(
                                        Button::new("history-diff-unified")
                                            .small()
                                            .ghost()
                                            .label("Unified")
                                            .selected(mode == DiffMode::Unified)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.set_mode(DiffMode::Unified, window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("history-diff-split")
                                            .small()
                                            .ghost()
                                            .label("Split")
                                            .selected(mode == DiffMode::Split)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.set_mode(DiffMode::Split, window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("history-diff-wrap")
                                            .small()
                                            .ghost()
                                            .label("Wrap")
                                            .selected(self.wrap)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.wrap = !this.wrap;
                                                this.focus_handle(cx).focus(window, cx);
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("history-diff-previous")
                                            .small()
                                            .ghost()
                                            .label("Previous change")
                                            .disabled(!has_hunks)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.navigate(false, window, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new("history-diff-next")
                                            .small()
                                            .ghost()
                                            .label("Next change")
                                            .disabled(!has_hunks)
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.navigate(true, window, cx)
                                            })),
                                    ),
                            )
                            .child(div().flex_1().min_h_0().min_w_0().child(
                                if no_retained_hunks {
                                    Self::message(
                                        "Diff exceeds the display limit",
                                        "No complete hunks fit within the display limit.",
                                    )
                                } else {
                                    Diff::new(&self.diff)
                                        .soft_wrap(self.wrap)
                                        .font_family(TERMINAL_FONT_FAMILY)
                                        .text_size(px(review_font_size()))
                                        .into_any_element()
                                },
                            ));
                    if matches!(
                        file.content,
                        FileContent::Text {
                            truncated: true,
                            ..
                        }
                    ) {
                        content = content.child(div().id("history-diff-truncated").test_support().px_3().py_2().text_xs()
                            .text_color(cx.theme().warning)
                            .child("Diff truncated: only complete hunks within the 4,000-line display limit are shown. Omitted source cannot be expanded or copied."));
                    }
                }
            }
        }
        let has_body = matches!(&self.state, ViewerState::Loaded(ChangedFile {
            content: FileContent::Text { hunks, truncated }, ..
        }) if !hunks.is_empty() || !truncated);
        let focus = self.focus_handle(cx);
        content
            .id("history-diff-viewer")
            .test_support()
            .when(!has_body, |view| {
                view.track_focus(&focus).on_mouse_down(
                    gpui_kit::MouseButton::Left,
                    cx.listener(|this, _, window, cx| this.focus_handle(cx).focus(window, cx)),
                )
            })
            .border_1()
            .border_color(cx.theme().border)
    }
}

impl Focusable for KitDiffViewer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.diff.focus_handle(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::super::model::{
        FileStatus, MAX_HUNK_LINES_PER_FILE, UnavailableReason, count_changes, diff_text,
    };
    use super::*;
    use gpui_kit::{component::diff::DiffSide, test::TestWindowExt as _};
    use std::{cell::RefCell, rc::Rc};

    fn prepared(old: &str, new: &str) -> PreparedKitDiff {
        let (hunks, truncated) = diff_text(old, new);
        let (additions, deletions) = count_changes(old, new);
        PreparedKitDiff::new(
            ChangedFile {
                path: "file.rs".into(),
                old_path: None,
                status: FileStatus::Modified,
                additions,
                deletions,
                content: FileContent::Text { hunks, truncated },
            },
            old,
            new,
            ["100644"; 2],
        )
        .unwrap()
    }

    fn viewer_window(
        cx: &mut gpui_kit::TestAppContext,
        prepared: PreparedKitDiff,
    ) -> (Entity<KitDiffViewer>, &mut gpui_kit::VisualTestContext) {
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(KitDiffViewer::new);
            view.update(cx, |view, cx| view.set_loaded(prepared, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        (holder.borrow().clone().unwrap(), test_cx)
    }

    #[gpui_kit::test]
    fn history_diff_controls_retain_state_copy_source_and_return_focus(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let old = (1..=60).map(|n| format!("line {n}\n")).collect::<String>();
        let new = old
            .replace("line 5\n", "changed 5\n")
            .replace("line 50\n", "changed 50\n");
        let (view, test_cx) = viewer_window(cx, prepared(&old, &new));
        test_cx.run_until_parked();
        let diff_id = view.read_with(test_cx, |view, _| view.diff.entity_id());
        for (button, expected) in [
            ("history-diff-split", DiffMode::Split),
            ("history-diff-unified", DiffMode::Unified),
        ] {
            test_cx.update(|window, cx| window.click(button, cx));
            test_cx.run_until_parked();
            view.read_with(test_cx, |view, cx| {
                assert_eq!(view.diff.entity_id(), diff_id);
                assert_eq!(view.diff.read(cx).mode(), expected);
            });
            test_cx.update(|window, cx| assert!(view.read(cx).focus_handle(cx).is_focused(window)));
        }
        test_cx.update(|window, cx| window.click("history-diff-wrap", cx));
        test_cx.run_until_parked();
        view.read_with(test_cx, |view, _| assert!(view.wrap));
        test_cx.update(|window, cx| {
            window.click("history-diff-next", cx);
            window.press("home", cx);
            window.press("down", cx);
            window.press("end", cx);
            window.click("history-diff-previous", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.press("home", cx);
            window.press("shift-down", cx);
            window.press("shift-down", cx);
        });
        let selection = view.read_with(test_cx, |view, cx| {
            view.diff.read(cx).selected_lines().unwrap()
        });
        assert_eq!(selection.side(), selection.end_side());
        assert!(selection.end() > selection.start());
        let source = if selection.side() == DiffSide::Original {
            &old
        } else {
            &new
        };
        let expected = source
            .split_inclusive('\n')
            .skip(selection.start() - 1)
            .take(selection.end() - selection.start() + 1)
            .collect::<String>();
        test_cx.update(|window, cx| {
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-c"
                } else {
                    "ctrl-c"
                },
                cx,
            )
        });
        assert_eq!(
            test_cx
                .read(|cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some(expected.as_str())
        );
        view.update(test_cx, |view, cx| {
            view.set_loading("another.rs".into(), cx)
        });
        view.read_with(test_cx, |view, cx| {
            assert!(view.diff.read(cx).files().is_empty());
            assert!(view.diff.read(cx).selected_lines().is_none());
            assert_eq!(view.diff.read(cx).mode(), DiffMode::Unified);
            assert!(view.wrap);
        });
    }

    #[gpui_kit::test]
    fn history_diff_keeps_truncation_visible_and_clears_unavailable_content(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let new = "added\n".repeat(MAX_HUNK_LINES_PER_FILE + 1);
        let (view, test_cx) = viewer_window(cx, prepared("", &new));
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("history-diff-truncated").visible());
            assert!(window.find("history-diff-next").visible());
            assert!(window.try_find("diff-body").is_none());
        });
        view.update(test_cx, |view, cx| {
            view.set_loaded(prepared("old\n", "new\n"), cx)
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("history-diff-truncated").is_none());
            assert!(window.find("diff-body").visible());
        });
        let mut unavailable = prepared("", "");
        unavailable.file.content = FileContent::Unavailable(UnavailableReason::Binary);
        unavailable.files.clear();
        view.update(test_cx, |view, cx| view.set_loaded(unavailable, cx));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("history-diff-split").is_none());
            assert!(window.try_find("diff-body").is_none());
        });
        view.update(test_cx, |view, cx| {
            view.set_error("file.rs".into(), "Read failed".into(), cx)
        });
        view.read_with(test_cx, |view, cx| {
            assert!(matches!(&view.state, ViewerState::Failed { message, .. } if message == "Read failed"));
            assert!(view.diff.read(cx).files().is_empty());
        });
    }
}
