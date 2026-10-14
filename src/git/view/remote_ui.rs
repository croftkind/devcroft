use super::*;
use crate::git::remote::{Action, Source};
use gpui_kit::component::{
    Selectable as _,
    menu::{DropdownMenu as _, PopupMenuItem},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RemoteForm {
    Track,
    Push,
}

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
        if matches!(
            action,
            Action::Rebase { .. } | Action::Pull { rebase: true }
        ) {
            let target = match &action {
                Action::Rebase { target, .. } => target.clone(),
                _ => snapshot.upstream.clone().unwrap_or_default(),
            };
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
            self.remote_form = Some(RemoteForm::Push);
            cx.notify();
        }
    }

    pub(super) fn render_remote_toolbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let LoadState::Loaded(snapshot) = &self.state else {
            return div().into_any_element();
        };
        let busy = self.active_mutation.is_some();
        let remote = snapshot.remotes.is_empty();
        let pull_disabled = busy || snapshot.upstream.is_none() || snapshot.operation.is_some();
        h_flex()
            .flex_none()
            .gap_1()
            .child(
                Button::new("git-fetch")
                    .small()
                    .ghost()
                    .label("Fetch")
                    .disabled(busy || remote)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.request_remote(Action::Fetch, window, cx)
                    })),
            )
            .child(
                Button::new("git-pull")
                    .small()
                    .ghost()
                    .label("Pull")
                    .dropdown_caret(true)
                    .tooltip(if busy {
                        "Wait for the active Git operation"
                    } else if snapshot.operation.is_some() {
                        "Finish the current Git operation before pulling"
                    } else if snapshot.upstream.is_none() {
                        "Configure an upstream branch before pulling"
                    } else {
                        "Choose fast-forward pull or pull with rebase"
                    })
                    .disabled(pull_disabled)
                    .dropdown_menu({
                        let git = cx.entity().downgrade();
                        move |mut menu, _, _| {
                            for (label, rebase) in [
                                ("Pull (fast-forward only)", false),
                                ("Pull with rebase…", true),
                            ] {
                                let git = git.clone();
                                menu = menu.item(
                                    PopupMenuItem::new(label).disabled(pull_disabled).on_click(
                                        move |_, window, cx| {
                                            let _ = git.update(cx, |this, cx| {
                                                this.request_remote(
                                                    Action::Pull { rebase },
                                                    window,
                                                    cx,
                                                );
                                            });
                                        },
                                    ),
                                );
                            }
                            menu
                        }
                    }),
            )
            .child(
                Button::new("git-push")
                    .small()
                    .ghost()
                    .label("Push")
                    .disabled(
                        busy || remote
                            || snapshot.detached
                            || snapshot.unborn
                            || snapshot.operation.is_some(),
                    )
                    .on_click(cx.listener(|this, _, window, cx| this.request_push(window, cx))),
            )
            .into_any_element()
    }

    pub(super) fn render_remote_form(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(form) = self.remote_form else {
            return div().into_any_element();
        };
        let LoadState::Loaded(snapshot) = &self.state else {
            return div().into_any_element();
        };
        let push = form == RemoteForm::Push;
        let start = self.selected_ref.clone().unwrap_or_else(|| "HEAD".into());
        let start_oid = if start == "HEAD" {
            snapshot.oid.clone()
        } else {
            snapshot
                .branches
                .iter()
                .find(|branch| branch.reference == start)
                .map(|branch| branch.oid.clone())
        };
        let busy = self.active_mutation.is_some();
        let blank = if push {
            self.push_branch.read(cx).value().trim().is_empty()
        } else {
            self.branch_name.read(cx).value().trim().is_empty()
        };
        let mut panel = v_flex()
            .flex_none()
            .gap_2()
            .p_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().text_sm().font_semibold().child(if push {
                "Push and set upstream"
            } else {
                "Create a local tracking branch"
            }));
        if push {
            panel = panel
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
                .child(Input::new(&self.push_branch));
        } else {
            panel = panel
                .child(div().text_xs().child(format!(
                    "Start point: {start} · select a branch below to change it"
                )))
                .child(Input::new(&self.branch_name));
        }
        panel
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
                                this.remote_form = None;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("git-form-submit")
                            .small()
                            .primary()
                            .label(if push {
                                "Push and Set Upstream"
                            } else {
                                "Create and Switch"
                            })
                            .disabled(
                                busy || blank
                                    || (push && self.selected_remote.is_none())
                                    || (!push && start_oid.is_none()),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let action = if push {
                                    Action::Push {
                                        remote: this.selected_remote.clone().unwrap_or_default(),
                                        branch: this.push_branch.read(cx).value().to_string(),
                                        set_upstream: true,
                                    }
                                } else {
                                    Action::Create {
                                        name: this.branch_name.read(cx).value().to_string(),
                                        start: start.clone(),
                                        start_oid: start_oid.clone().unwrap_or_default(),
                                        track: form == RemoteForm::Track,
                                    }
                                };
                                this.request_remote(action, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_branches(
        &self,
        snapshot: &RepositorySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.active_mutation.is_some();
        let query = self.branch_query.read(cx).value().to_lowercase();
        let selected = snapshot
            .branches
            .iter()
            .find(|branch| self.selected_ref.as_ref() == Some(&branch.reference))
            .or_else(|| snapshot.branches.iter().find(|branch| branch.current));
        let mut list = v_flex().gap_3();
        for remote in [false, true] {
            let matching: Vec<_> = snapshot
                .branches
                .iter()
                .filter(|branch| {
                    branch.remote == remote
                        && format!("{} {}", branch.name, branch.subject)
                            .to_lowercase()
                            .contains(&query)
                })
                .collect();
            let mut section = v_flex().gap_1().child(
                h_flex()
                    .h(px(28.))
                    .px_2()
                    .gap_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().flex_1().font_semibold().child(if remote {
                        "Remote branches"
                    } else {
                        "Local branches"
                    }))
                    .child(self.render_count(matching.len(), cx)),
            );
            if matching.is_empty() {
                section = section.child(
                    div()
                        .px_2()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(if query.is_empty() {
                            "No branches"
                        } else {
                            "No matching branches"
                        }),
                );
            }
            for (index, branch) in matching.iter().take(250).enumerate() {
                let reference = branch.reference.clone();
                section = section.child(
                    Button::new((
                        if remote {
                            "git-remote-branch"
                        } else {
                            "git-local-branch"
                        },
                        index,
                    ))
                    .ghost()
                    .w_full()
                    .h(px(56.))
                    .px_2()
                    .accessibility_label(branch.name.clone())
                    .tooltip(branch.name.clone())
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .child(
                                Icon::new(gpui_kit::assets::IconName::GitBranch)
                                    .size(px(14.))
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .gap_1()
                                    .child(div().truncate().text_sm().child(branch.name.clone()))
                                    .child(
                                        div()
                                            .truncate()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(branch.subject.clone()),
                                    ),
                            )
                            .when(branch.current, |view| {
                                view.child(
                                    Icon::new(IconName::Check)
                                        .size(px(14.))
                                        .text_color(cx.theme().success),
                                )
                            }),
                    )
                    .selected(selected.is_some_and(|selected| selected.reference == reference))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_ref = Some(reference.clone());
                        cx.notify();
                    })),
                );
            }
            if matching.len() > 250 {
                section = section.child(
                    div()
                        .text_xs()
                        .child("Showing 250 branches. Narrow the search to find more."),
                );
            }
            list = list.child(section);
        }
        let mut detail = v_flex().w_full().max_w(px(720.)).gap_5().p_6();
        if let Some(branch) = selected {
            detail = detail
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(if branch.remote {
                                    "REMOTE BRANCH"
                                } else {
                                    "LOCAL BRANCH"
                                }),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_xl()
                                .font_semibold()
                                .child(branch.name.clone()),
                        )
                        .when(branch.current, |view| {
                            view.child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().success)
                                    .child("Currently checked out"),
                            )
                        }),
                )
                .child(self.render_detail(
                    "Commit",
                    format!(
                        "{}  {}",
                        &branch.oid[..branch.oid.len().min(12)],
                        branch.subject
                    ),
                    cx,
                ))
                .child(self.render_detail(
                    "Upstream",
                    if branch.upstream.is_empty() {
                        "No upstream configured".into()
                    } else {
                        format!("{} {}", branch.upstream, branch.tracking)
                    },
                    cx,
                ));
            let reference = branch.reference.clone();
            let is_remote = branch.remote;
            let actions = h_flex().gap_2().flex_wrap().child(
                Button::new("git-switch-branch")
                    .small()
                    .primary()
                    .label(if is_remote {
                        "Track as Local Branch…"
                    } else {
                        "Switch to Branch"
                    })
                    .disabled(busy || branch.current || snapshot.operation.is_some())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if is_remote {
                            this.remote_form = Some(RemoteForm::Track);
                            this.branch_name
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            cx.notify();
                        } else {
                            this.request_remote(
                                Action::Switch {
                                    reference: reference.clone(),
                                },
                                window,
                                cx,
                            );
                        }
                    })),
            );
            let target = branch.reference.clone();
            let target_oid = branch.oid.clone();
            let actions = actions.child(
                Button::new("git-rebase-onto")
                    .small()
                    .ghost()
                    .label("Rebase onto this branch…")
                    .disabled(
                        busy || branch.current
                            || snapshot.detached
                            || snapshot.unborn
                            || snapshot.operation.is_some(),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.request_remote(
                            Action::Rebase {
                                target: target.clone(),
                                target_oid: target_oid.clone(),
                            },
                            window,
                            cx,
                        )
                    })),
            );
            detail = detail.child(actions);
        } else {
            detail = detail
                .child(self.render_detail("Current branch", snapshot.branch_label(), cx))
                .child(
                    div()
                        .text_sm()
                        .child("Select a branch to inspect, switch or rebase."),
                );
        }
        h_flex()
            .size_full()
            .min_h_0()
            .child(
                v_flex()
                    .w(px(340.))
                    .max_w(relative(0.4))
                    .bg(cx.theme().sidebar)
                    .h_full()
                    .min_h_0()
                    .flex_none()
                    .gap_3()
                    .p_3()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .h(px(30.))
                            .gap_2()
                            .child(div().flex_1().text_sm().font_semibold().child("Branches"))
                            .child(self.render_count(snapshot.branches.len(), cx)),
                    )
                    .child(Input::new(&self.branch_query))
                    .child(
                        Button::new("git-new-branch")
                            .small()
                            .icon(IconName::Plus)
                            .label("New branch…")
                            .disabled(busy || snapshot.unborn || snapshot.operation.is_some())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_new_branch_dialog(window, cx);
                            })),
                    )
                    .child(div().flex_1().min_h_0().overflow_y_scrollbar().child(list)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scrollbar()
                    .child(detail),
            )
            .into_any_element()
    }
}
