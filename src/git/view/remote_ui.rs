use super::*;
use crate::git::remote::{Action, Source};
use gpui_kit::component::{
    Selectable as _,
    menu::{DropdownMenu as _, PopupMenuItem},
};

impl GitView {
    pub(super) fn request_remote(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.active_mutation.is_some() {
            return;
        }
        let LoadState::Loaded(snapshot) = &self.state else {
            return;
        };
        let mutation = Mutation::Remote {
            source: Source::new(snapshot),
            action: action.clone(),
        };
        if matches!(action, Action::Pull { rebase: true }) {
            let target = snapshot.upstream.clone().unwrap_or_default();
            if snapshot.is_dirty()
                || snapshot.operation.is_some()
                || !self.dirty_editor_paths.is_empty()
            {
                self.notice = Some(OperationNotice { message: "Finish the current Git operation and save, commit or stash changes before rebasing. Devcroft does not auto-stash.".into(), error: true });
            } else {
                self.confirmation = Some(Confirmation::Remote { mutation, title: format!("Rebase {} onto {}?", snapshot.branch_label(), target), detail: "Commits on the current branch may be rewritten. On conflict, Git will preserve the operation and the dialog will show Merge Changes.".into() });
            }
            cx.notify();
        } else {
            self.enqueue_mutation(mutation, window, cx);
        }
    }

    fn request_push(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let LoadState::Loaded(snapshot) = &self.state else {
            return;
        };
        if let Some(branch) = snapshot
            .branches
            .iter()
            .find(|branch| branch.current && !branch.upstream.is_empty())
        {
            let Some(destination) = branch.push_ref.strip_prefix("refs/heads/") else {
                return;
            };
            self.request_remote(
                Action::Push {
                    remote: branch.push_remote.clone(),
                    branch: destination.into(),
                    set_upstream: false,
                },
                window,
                cx,
            );
        } else {
            self.selected_remote = snapshot.remotes.first().cloned();
            let branch = snapshot.branch.clone().unwrap_or_default();
            self.push_branch
                .update(cx, |input, cx| input.set_value(branch, window, cx));
            self.push_form_open = true;
            cx.notify();
        }
    }

    pub(crate) fn render_header_actions(&self, cx: &mut Context<Self>) -> AnyElement {
        let snapshot = match &self.state {
            LoadState::Loaded(snapshot) => Some(snapshot.as_ref()),
            _ => None,
        };
        let busy = self.active_mutation.is_some();
        let fetch_disabled = busy || snapshot.is_none_or(|snapshot| snapshot.remotes.is_empty());
        let pull_disabled = busy
            || snapshot
                .is_none_or(|snapshot| snapshot.upstream.is_none() || snapshot.operation.is_some());
        let push_disabled = busy
            || snapshot.is_none_or(|snapshot| {
                snapshot.remotes.is_empty()
                    || snapshot.detached
                    || snapshot.unborn
                    || snapshot.operation.is_some()
            });
        let refreshing = self.load_in_flight;
        let git = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .h(px(28.))
            .rounded_md()
            .border_1()
            .border_color(rgb(0x2a2d2d))
            .bg(rgb(0x222525))
            .child(
                Button::new("git-open-commit")
                    .small()
                    .ghost()
                    .h(px(26.))
                    .px_2()
                    .icon(gpui_kit::assets::IconName::GitCommitHorizontal)
                    .label("Commit")
                    .tooltip("Create a commit…")
                    .disabled(self.busy() || snapshot.is_none())
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_commit_dialog(window, cx)),
                    ),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(rgb(0x3a3d3d)))
            .child(
                Button::new("git-fetch")
                    .small()
                    .ghost()
                    .h(px(26.))
                    .px_2()
                    .icon(IconName::ArrowDown)
                    .label(if busy { "Working…" } else { "Fetch" })
                    .tooltip(
                        self.operation_notice()
                            .map(|(message, _)| message.to_owned())
                            .unwrap_or_else(|| "Fetch remote changes".into()),
                    )
                    .disabled(fetch_disabled)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.request_remote(Action::Fetch, window, cx)
                    })),
            )
            .child(div().w(px(1.)).h(px(16.)).bg(rgb(0x3a3d3d)))
            .child(
                Button::new("git-actions-menu")
                    .small()
                    .ghost()
                    .size(px(26.))
                    .icon(IconName::ChevronDown)
                    .tooltip("Git actions")
                    .accessibility_label("Git actions")
                    .dropdown_menu_with_anchor(
                        gpui_kit::Anchor::TopRight,
                        move |mut menu, _, _| {
                            for (label, rebase) in [
                                ("Pull (fast-forward only)", false),
                                ("Pull with rebase…", true),
                            ] {
                                let git = git.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label)
                                        .icon(IconName::ArrowDown)
                                        .disabled(pull_disabled)
                                        .on_click(move |_, window, cx| {
                                            let _ = git.update(cx, |this, cx| {
                                                this.request_remote(
                                                    Action::Pull { rebase },
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                );
                            }
                            let push_git = git.clone();
                            let refresh_git = git.clone();
                            menu.item(
                                PopupMenuItem::new("Push")
                                    .icon(IconName::ArrowUp)
                                    .disabled(push_disabled)
                                    .on_click(move |_, window, cx| {
                                        let _ = push_git
                                            .update(cx, |this, cx| this.request_push(window, cx));
                                    }),
                            )
                            .separator()
                            .item(
                                PopupMenuItem::new(if refreshing {
                                    "Refreshing repository…"
                                } else {
                                    "Refresh repository"
                                })
                                .icon(IconName::RotateCw)
                                .disabled(busy || refreshing)
                                .on_click(move |_, _, cx| {
                                    let _ = refresh_git.update(cx, |this, cx| {
                                        if this.active_mutation.is_none() && !this.load_in_flight {
                                            this.file_changes.mark_changed();
                                            this.reload(true, cx);
                                            this.reset_history(cx);
                                        }
                                    });
                                }),
                            )
                        },
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_remote_form(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.push_form_open {
            return div().into_any_element();
        }
        let LoadState::Loaded(snapshot) = &self.state else {
            return div().into_any_element();
        };
        let busy = self.active_mutation.is_some();
        let blank = self.push_branch.read(cx).value().trim().is_empty();
        v_flex()
            .flex_none()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child("Push and set upstream"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .children(snapshot.remotes.iter().enumerate().map(|(index, remote)| {
                        let remote = remote.clone();
                        Button::new(("git-push-remote", index))
                            .small()
                            .ghost()
                            .label(remote.clone())
                            .selected(self.selected_remote.as_ref() == Some(&remote))
                            .disabled(busy)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected_remote = Some(remote.clone());
                                cx.notify();
                            }))
                    })),
            )
            .child(Input::new(&self.push_branch))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("git-form-cancel")
                            .small()
                            .ghost()
                            .label("Cancel")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.push_form_open = false;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("git-form-submit")
                            .small()
                            .primary()
                            .label("Push and Set Upstream")
                            .disabled(busy || blank || self.selected_remote.is_none())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_remote(
                                    Action::Push {
                                        remote: this.selected_remote.clone().unwrap_or_default(),
                                        branch: this.push_branch.read(cx).value().to_string(),
                                        set_upstream: true,
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            )
            .into_any_element()
    }
}
