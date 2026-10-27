//! Review's retained GPUI Kit diff. Logical comment anchors stay in Review;
//! Kit owns code rendering, selection, wrapping and virtualization.

use super::{LoadedReview, ReviewState, ReviewView, model::ReviewDiff, stream};
use crate::{
    diff::kit_viewer::PreparedKitDiff, fonts::TERMINAL_FONT_FAMILY, metrics::review_font_size,
};
use gpui_kit::{
    Context, Focusable as _, IntoElement, ParentElement as _, Styled as _,
    component::{
        ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        diff::{Diff, DiffAnnotation, DiffFile, DiffMode, DiffState},
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};
use std::rc::Rc;

pub(super) fn new_diff_state(cx: &mut Context<DiffState>) -> DiffState {
    let dark = cx.theme().is_dark();
    DiffState::new([], cx)
        .with_context_lines(None)
        .with_syntax_highlighter(move |language, source| {
            crate::diff::syntax::highlight_source(language, source, dark)
        })
}

pub(super) fn prepare_files(diff: &ReviewDiff) -> Result<Vec<DiffFile>, String> {
    diff.files.iter().try_fold(Vec::new(), |mut files, file| {
        files.extend(PreparedKitDiff::from_snapshot(file)?);
        Ok(files)
    })
}

impl ReviewView {
    pub(super) fn sync_diff_theme(&mut self, cx: &mut Context<Self>) {
        let dark = cx.theme().is_dark();
        if self.syntax_dark != dark {
            self.syntax_dark = dark;
            self.diff_state.update(cx, |diff, cx| {
                diff.set_syntax_highlighter(
                    move |language, source| {
                        crate::diff::syntax::highlight_source(language, source, dark)
                    },
                    cx,
                );
            });
        }
    }

    pub(super) fn replace_diff_files(
        &mut self,
        files: Vec<DiffFile>,
        loaded: &LoadedReview,
        restore: bool,
        cx: &mut Context<Self>,
    ) {
        let selection = restore
            .then(|| self.diff_state.read(cx).selected_lines())
            .flatten()
            .and_then(|range| match &self.state {
                ReviewState::Loaded(previous) => {
                    super::viewport::restore_selection(previous, loaded, &range)
                }
                _ => None,
            });
        let path = restore.then(|| self.last_scrolled.clone()).flatten();
        self.diff_state.update(cx, |diff, cx| {
            diff.set_files(files, cx);
            if let Some(selection) =
                selection.filter(|range| diff.files().iter().any(|f| f.path() == range.path()))
            {
                diff.scroll_to_line(
                    gpui_kit::component::diff::DiffLinePosition::new(
                        selection.path().clone(),
                        selection.side(),
                        selection.start(),
                    ),
                    cx,
                );
                diff.set_selected_lines(Some(selection), cx);
            } else if let Some(path) = path {
                diff.scroll_to_file(&path, cx);
            }
        });
    }

    pub(super) fn sync_collapsed(&mut self, cx: &mut Context<Self>) {
        let paths = self
            .diff_state
            .read(cx)
            .files()
            .iter()
            .map(|file| file.path().clone())
            .collect::<Vec<_>>();
        self.diff_state.update(cx, |diff, cx| {
            for path in paths {
                diff.set_file_collapsed(&path, self.collapsed.borrow().contains(path.as_str()), cx);
            }
        });
    }

    pub(super) fn render_diff_controls(&self, cx: &mut Context<Self>) -> gpui_kit::AnyElement {
        if !matches!(self.state, ReviewState::Loaded(_)) {
            return div().into_any_element();
        }
        let mode = self.diff_state.read(cx).mode();
        let disabled = !self
            .diff_state
            .read(cx)
            .files()
            .iter()
            .any(DiffFile::has_changes);
        h_flex()
            .flex_none()
            .gap_1()
            .children(
                [
                    ("review-diff-unified", "Unified", DiffMode::Unified),
                    ("review-diff-split", "Split", DiffMode::Split),
                ]
                .map(|(id, label, next)| {
                    Button::new(id)
                        .small()
                        .ghost()
                        .label(label)
                        .selected(mode == next)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.diff_state
                                .update(cx, |diff, cx| diff.set_mode(next, cx));
                            this.diff_state.focus_handle(cx).focus(window, cx);
                        }))
                }),
            )
            .children(
                [
                    (
                        "review-diff-previous",
                        "Previous change",
                        IconName::ChevronUp,
                        false,
                    ),
                    (
                        "review-diff-next",
                        "Next change",
                        IconName::ChevronDown,
                        true,
                    ),
                ]
                .map(|(id, label, icon, forward)| {
                    Button::new(id)
                        .small()
                        .ghost()
                        .icon(icon)
                        .tooltip(label)
                        .accessibility_label(label)
                        .disabled(disabled)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.navigate_change(forward, cx);
                            this.diff_state.focus_handle(cx).focus(window, cx);
                        }))
                }),
            )
            .into_any_element()
    }

    pub(super) fn render_kit_diff(
        &self,
        loaded: &Rc<LoadedReview>,
        cx: &mut Context<Self>,
    ) -> gpui_kit::AnyElement {
        let headers = loaded.clone();
        let header_view = cx.entity();
        let viewed = self.viewed.clone();
        let open = self.editor_open_available;
        let annotations_view = cx.entity();
        let add_view = cx.entity();
        let mut annotations = self.diff_annotations(loaded);
        for (ix, file) in loaded.diff.files.iter().enumerate() {
            if matches!(&file.content, super::model::FileContent::Text { hunks, truncated: true } if !hunks.is_empty())
            {
                annotations.push(DiffAnnotation::file(
                    ("review-notice", ix),
                    file.path.clone(),
                ));
            }
        }
        let notice_files = loaded.diff.clone();
        let diff = Diff::new(&self.diff_state)
            .soft_wrap(self.diff_state.read(cx).mode() == DiffMode::Split)
            .font_family(TERMINAL_FONT_FAMILY)
            .text_size(px(review_font_size()))
            .annotations(annotations)
            .render_header(move |file, _, _| {
                let file = headers
                    .diff
                    .files
                    .iter()
                    .find(|f| f.path.as_str() == file.path().as_str())
                    .unwrap();
                stream::file_header_row(
                    file,
                    viewed.borrow().contains(&file.path),
                    &header_view,
                    open,
                )
            })
            .render_annotation(move |annotation, _, cx| {
                if let Some((_, file)) = notice_files.files.iter().enumerate().find(|(ix, _)| {
                    annotation.id() == &gpui_kit::ElementId::from(("review-notice", *ix))
                }) {
                    return match &file.content {
                        super::model::FileContent::Unavailable(reason) => {
                            stream::unavailable_row(*reason)
                        }
                        super::model::FileContent::Text {
                            truncated: true, ..
                        } => stream::truncated_row(),
                        _ => stream::no_changes_row(),
                    };
                }
                annotations_view.update(cx, |this, cx| this.render_diff_annotation(annotation, cx))
            })
            .on_add_annotation(move |range, window, cx| {
                add_view.update(cx, |this, cx| this.select_diff_range(range, window, cx));
            });
        v_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(self.render_feedback_error())
            .when(!loaded.diff.files.is_empty(), |body| {
                body.child(div().flex_1().min_h_0().min_w_0().child(diff))
            })
            .child(self.render_outside_comments(loaded, cx))
            .into_any_element()
    }
}
