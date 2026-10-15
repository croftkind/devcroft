use super::*;
use gpui_kit::component::{dialog::Cancel, input::Textarea};

const MAX_COMMIT_PREVIEW_FILES: usize = 20;

impl GitView {
    fn commit_block_reason(&self, cx: &App) -> Option<&'static str> {
        if self.busy() {
            return Some("Wait for the active Git operation");
        }
        let LoadState::Loaded(snapshot) = &self.state else {
            return Some("Wait for the repository to load");
        };
        if !snapshot.conflicts.is_empty() {
            Some("Resolve merge changes before committing")
        } else if snapshot.staged.is_empty() {
            Some("Stage at least one file before committing")
        } else if self.commit_message.read(cx).value().trim().is_empty() {
            Some("Enter a commit message")
        } else {
            None
        }
    }

    pub(super) fn open_commit_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_dialog_open || self.busy() || !matches!(self.state, LoadState::Loaded(_)) {
            return;
        }
        self.commit_dialog_open = true;
        self.commit_error = None;
        let git = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let Some(view) = git.upgrade() else {
                return dialog;
            };
            let (busy, reason) =
                view.read_with(cx, |this, cx| (this.busy(), this.commit_block_reason(cx)));
            let body = view.update(cx, |this, cx| this.render_commit_dialog_body(cx));
            let confirm_git = git.clone();
            let cancel_git = git.clone();
            let close_git = git.clone();
            let cancel_anchor = window
                .use_keyed_state("commit-cancel-anchor", cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone();
            let confirm_anchor = window
                .use_keyed_state("commit-confirm-anchor", cx, |_, cx| cx.focus_handle())
                .read(cx)
                .clone();
            dialog
                .title("Create a commit")
                .w(px(720.))
                .max_w((window.viewport_size().width - px(48.)).max(px(280.)))
                .overlay_closable(false)
                .close_button(!busy)
                .child(
                    div()
                        .max_h((window.viewport_size().height - px(220.)).max(px(160.)))
                        .overflow_y_scrollbar()
                        .child(body),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("git-cancel-commit")
                                .label("Cancel")
                                .disabled(busy)
                                .child(div().absolute().size_0().track_focus(&cancel_anchor))
                                .on_click(move |_, window, cx| {
                                    cancel_anchor.dispatch_action(&Cancel, window, cx)
                                }),
                        )
                        .child(
                            Button::new("git-commit")
                                .primary()
                                .icon(IconName::Check)
                                .label(if busy {
                                    "Committing…"
                                } else {
                                    "Commit changes"
                                })
                                .disabled(reason.is_some())
                                .tooltip(reason.unwrap_or("Create commit"))
                                .child(div().absolute().size_0().track_focus(&confirm_anchor))
                                .on_click(move |_, window, cx| {
                                    confirm_anchor.dispatch_action(
                                        &Confirm { secondary: false },
                                        window,
                                        cx,
                                    )
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let _ = confirm_git.update(cx, |this, cx| {
                        // Recheck live state for keyboard confirmation and stale buttons.
                        if this.commit_block_reason(cx).is_none() {
                            this.commit_error = None;
                            let message = this.commit_message.read(cx).value().to_string();
                            this.enqueue_mutation(Mutation::Commit { message }, window, cx);
                        }
                    });
                    // Keep the draft and error visible until the commit succeeds.
                    false
                })
                .on_cancel(move |_, _, cx| {
                    cancel_git
                        .read_with(cx, |this, _| !this.busy())
                        .unwrap_or(true)
                })
                .on_close(move |_, _, cx| {
                    let _ = close_git.update(cx, |this, cx| {
                        this.commit_dialog_open = false;
                        cx.notify();
                    });
                })
        });
        self.commit_message
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
        cx.notify();
    }

    fn render_commit_dialog_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut body = v_flex().gap_3();
        if let LoadState::Loaded(snapshot) = &self.state {
            body = body
                .child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(gpui_kit::assets::IconName::GitBranch).size(px(16.)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .child(snapshot.branch_label()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} staged files", snapshot.staged.len())),
                        ),
                )
                .when(!snapshot.staged.is_empty(), |body| {
                    body.child(
                        div().max_h(px(96.)).overflow_y_scrollbar().child(
                            v_flex()
                                .gap_1()
                                .children(
                                    snapshot.staged.iter().take(MAX_COMMIT_PREVIEW_FILES).map(
                                        |change| {
                                            h_flex()
                                                .gap_2()
                                                .text_xs()
                                                .child(
                                                    div()
                                                        .w(px(16.))
                                                        .text_color(cx.theme().muted_foreground)
                                                        .child(change.kind.label()),
                                                )
                                                .child(div().flex_1().min_w_0().truncate().child(
                                                    change.path.to_string_lossy().into_owned(),
                                                ))
                                        },
                                    ),
                                )
                                .when(snapshot.staged.len() > MAX_COMMIT_PREVIEW_FILES, |files| {
                                    files.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!(
                                                "And {} more staged files",
                                                snapshot.staged.len() - MAX_COMMIT_PREVIEW_FILES
                                            )),
                                    )
                                }),
                        ),
                    )
                });
        }
        body.child(div().text_sm().font_semibold().child("Commit message"))
            .child(
                Textarea::new(&self.commit_message)
                    .h(px(220.))
                    .disabled(self.busy())
                    .aria_label("Commit message"),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Only staged changes will be included in this commit."),
            )
            .when_some(self.commit_block_reason(cx), |body, reason| {
                body.child(div().text_xs().text_color(cx.theme().warning).child(reason))
            })
            .when_some(self.commit_error.as_ref(), |body, message| {
                body.child(
                    div().max_h(px(96.)).overflow_y_scrollbar().child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().danger)
                            .child(message.clone()),
                    ),
                )
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use std::{cell::RefCell, rc::Rc};

    #[gpui_kit::test]
    fn commit_dialog_preserves_drafts_on_cancel_and_failure_then_closes_on_success(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap()
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("file.txt"), "staged content\n").unwrap();
        git(&["add", "file.txt"]);
        let checkout = root.to_owned();
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view =
                cx.new(|cx| GitView::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            view.focus_handle(cx).focus(window, cx);
            view.update(cx, |view, cx| view.open_commit_dialog(window, cx));
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.click("git-commit", cx);
            assert!(!view.read(cx).busy());
            window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
            assert!(!view.read(cx).busy());
            view.update(cx, |view, cx| {
                view.restore_commit_message("Test dialog commit", window, cx)
            });
            window.render_frame(cx);
            window.click("git-cancel-commit", cx);
            assert!(!window.has_active_dialog(cx));
            assert!(!view.read(cx).commit_dialog_open);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(view.focus_handle(cx).is_focused(window));
            assert_eq!(
                view.read(cx).commit_message.read(cx).value().as_ref(),
                "Test dialog commit"
            );
            view.update(cx, |view, cx| view.open_commit_dialog(window, cx));
            window.render_frame(cx);
            window.press("escape", cx);
            assert!(!window.has_active_dialog(cx));
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(view.focus_handle(cx).is_focused(window));
            assert_eq!(
                view.read(cx).commit_message.read(cx).value().as_ref(),
                "Test dialog commit"
            );
            view.update(cx, |view, cx| view.open_commit_dialog(window, cx));
            window.render_frame(cx);
        });
        // A commit rejected by Git must keep the modal and draft for a retry.
        git(&["config", "user.name", ""]);
        test_cx.update(|window, cx| {
            window.click("git-commit", cx);
            assert!(view.read(cx).busy());
            window.dispatch_action(Box::new(Confirm { secondary: false }), cx);
            window.dispatch_action(Box::new(Cancel), cx);
            assert!(window.has_active_dialog(cx));
            assert!(view.read(cx).mutation_queue.is_empty());
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            let view = view.read(cx);
            assert!(!view.busy());
            assert!(view.notice.as_ref().is_some_and(|notice| notice.error));
            assert!(view.commit_error.is_some());
            assert_eq!(
                view.commit_message.read(cx).value().as_ref(),
                "Test dialog commit"
            );
        });
        git(&["config", "user.name", "Test"]);
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("git-commit", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(!window.has_active_dialog(cx));
            assert!(!view.read(cx).commit_dialog_open);
            assert!(view.read(cx).commit_message.read(cx).value().is_empty());
            assert!(view.focus_handle(cx).is_focused(window));
        });
        assert_eq!(
            git(&["log", "-1", "--format=%s"]).trim(),
            "Test dialog commit"
        );
    }
}
