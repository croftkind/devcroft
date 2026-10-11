use super::*;
use crate::git::{
    conflicts::{Continuation, OperationDetails, Resolution},
    merge_editor::{MergeEditor, RecoveryChanged, ResolveRequest},
};
impl GitDialog {
    pub(super) fn discover_conflict_drafts(&self, cx: &mut Context<Self>) {
        let Some(data_root) = self.data_root.clone() else {
            return;
        };
        let root = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::git::merge_editor::journal::discover(&data_root, &root)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(discovery) => {
                        this.recovery_paths.extend(discovery.paths);
                        this.recovery_warning = discovery.warning;
                    }
                    Err(message) => this.recovery_warning = Some(message),
                }
                cx.notify();
            });
        })
        .detach();
    }
    pub(super) fn ensure_merge_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_next_mutation(window, cx);
        let busy = self.active_mutation.is_some()
            || self.continuation_loading
            || self.confirmation.is_some();
        for editor in self.merge_editors.values() {
            editor.update(cx, |editor, cx| editor.set_busy(busy, cx));
        }
        let Some(target) = &self.selection else {
            return;
        };
        if target.area != ChangeArea::Conflict {
            return;
        }
        let path = target.change.path.clone();
        if let Some(editor) = self.merge_editors.get(&path) {
            if let LoadState::Loaded(snapshot) = &self.state
                && let Some(index) = snapshot
                    .conflict_entries
                    .iter()
                    .find(|entry| entry.path == path)
            {
                editor.update(cx, |editor, cx| {
                    editor.reconcile(index, &snapshot.operation_details, window, cx)
                });
            }
            return;
        }
        let editor = cx.new(|cx| {
            MergeEditor::new(
                self.checkout.clone(),
                path.clone(),
                self.data_root.as_deref(),
                self.external_editors.clone(),
                window,
                cx,
            )
        });
        cx.subscribe(&editor,|this,_,request:&ResolveRequest,cx|{
            if this.active_mutation.is_some(){return}
            let mutation=Mutation::Resolve{source:Box::new(request.source.clone()),choice:request.choice.clone()};
            let marker_warning=matches!(&request.choice,Resolution::Text{stage:true,result} if crate::git::conflicts::marker_warning(result));
            if !matches!(request.choice,Resolution::Text{..}) || marker_warning {
                this.confirmation=Some(Confirmation::Remote{mutation,title:format!("{} {}?",request.choice.label(),request.source.index.path.display()),detail:if marker_warning{"Marker-like lines remain in Result. Review them before staging. Some strings may be ordinary content; Devcroft cannot prove the merge is complete.".into()}else{"Replace this working-tree path with the chosen index side (or delete it), then stage only this path. Any working-tree edits to this path will be replaced.".into()}});
            }else{
                // Entity events have no Window. Queue here and start during the next render.
                if this.is_dirty_in_editor(&request.source.index.path){this.notice=Some(OperationNotice{message:"Save or discard this file's built-in editor draft before resolving it in Git.".into(),error:true});}else{this.mutation_queue.push_back(mutation);}
            }
            cx.notify();
        }).detach();
        cx.subscribe(&editor, |this, _, event: &RecoveryChanged, cx| {
            if event.exists {
                this.recovery_paths.insert(event.path.clone());
            } else {
                this.recovery_paths.remove(&event.path);
            }
            cx.notify();
        })
        .detach();
        self.merge_editors.insert(path, editor);
        self.start_next_mutation(window, cx);
    }
    pub(super) fn selected_change_view(&self) -> AnyElement {
        if let Some(target) = &self.selection
            && target.area == ChangeArea::Conflict
            && let Some(editor) = self.merge_editors.get(&target.change.path)
        {
            return editor.clone().into_any_element();
        }
        self.diff_viewer.clone().into_any_element()
    }
    fn request_continuation(
        &mut self,
        source: OperationDetails,
        action: Continuation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .merge_editors
            .values()
            .any(|editor| editor.read(cx).dirty(cx))
        {
            self.notice=Some(OperationNotice{message:"Save your unsaved merge Results before continuing, aborting, or skipping. Their recovery drafts remain available when you reopen Git.".into(),error:true});
            cx.notify();
            return;
        }
        if self.active_mutation.is_some() || self.continuation_loading {
            return;
        }
        self.continuation_loading = true;
        let root = self.checkout.clone();
        cx.spawn_in(window,async move|this,cx|{
            let checked=cx.background_spawn(async move{crate::git::conflicts::capture_continuation(&root,&source)}).await;
            let _=cx.update(|window,cx|this.update(cx,|this,cx|{
                this.continuation_loading=false;
                match checked{
                    Ok(source)=>{
                        let mutation=Mutation::Continue{source:Box::new(source),action};
                        if action==Continuation::Continue{this.enqueue_mutation(mutation,window,cx)}else{
                            this.confirmation=Some(Confirmation::Remote{mutation,title:format!("{} this Git operation?",action.label()),detail:if action==Continuation::Abort{"Git will restore the pre-operation state. Saved conflict edits may be replaced; Git can refuse if other work would be lost.".into()}else{"Git will discard this rebase step and continue with the next commit. Saved edits to the skipped commit may be replaced.".into()}});
                        }
                    },Err(message)=>this.notice=Some(OperationNotice{message,error:true})
                }
                cx.notify();
            }));
        }).detach();
        cx.notify();
    }
    pub(super) fn render_orphaned_results(
        &self,
        snapshot: &RepositorySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut paths = self
            .recovery_paths
            .iter()
            .filter(|path| !snapshot.conflicts.iter().any(|c| &c.path == *path))
            .cloned()
            .collect::<Vec<_>>();
        paths.extend(
            self.merge_editors
                .iter()
                .filter(|(path, editor)| {
                    editor.read(cx).has_draft(cx)
                        && !snapshot.conflicts.iter().any(|c| &c.path == *path)
                })
                .map(|(path, _)| path.clone()),
        );
        paths.sort();
        paths.dedup();
        paths.sort();
        v_flex()
            .gap_1()
            .when_some(self.recovery_warning.as_ref(), |view, warning| {
                view.child(
                    div()
                        .text_xs()
                        .text_color(rgb(0xfbbf24))
                        .child(warning.clone()),
                )
                .when_some(self.data_root.as_ref(), |view, root| {
                    let directory =
                        crate::git::merge_editor::journal::directory_for(root, &self.checkout);
                    view.child(
                        Button::new("git-recovery-folder")
                            .small()
                            .ghost()
                            .label("Open recovery folder")
                            .on_click(cx.listener(move |_, _, _, cx| cx.reveal_path(&directory))),
                    )
                })
            })
            .when(!paths.is_empty(), |view| {
                view.child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .child("Result recovery drafts"),
                )
            })
            .children(paths.into_iter().enumerate().map(|(i, path)| {
                Button::new(("git-orphan-draft", i))
                    .small()
                    .ghost()
                    .label(path.to_string_lossy().into_owned())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_change(
                            DiffTarget {
                                area: ChangeArea::Conflict,
                                change: Change {
                                    path: path.clone(),
                                    original_path: None,
                                    kind: crate::git::model::ChangeKind::Unmerged,
                                    index_mode: None,
                                    index_oid: None,
                                },
                            },
                            cx,
                        )
                    }))
            }))
            .into_any_element()
    }
    pub(super) fn render_operation_banner(&self, cx: &mut Context<Self>) -> AnyElement {
        let LoadState::Loaded(snapshot) = &self.state else {
            return div().into_any_element();
        };
        let Some(operation) = &snapshot.operation_details else {
            return div().into_any_element();
        };
        let busy = self.active_mutation.is_some() || self.continuation_loading;
        let unresolved = !snapshot.conflicts.is_empty();
        h_flex()
            .flex_none()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(rgb(0x262a30))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_semibold().child(format!(
                        "{} · {}",
                        operation.kind.label(),
                        operation.step
                    )))
                    .child(
                        div()
                            .text_xs()
                            .truncate()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Target: {} · {} unresolved paths",
                                operation.target,
                                snapshot.conflicts.len()
                            )),
                    ),
            )
            .children(
                [
                    Continuation::Continue,
                    Continuation::Abort,
                    Continuation::Skip,
                ]
                .into_iter()
                .filter(|a| {
                    *a != Continuation::Skip
                        || operation.kind == crate::git::model::OperationState::Rebase
                })
                .enumerate()
                .map(|(i, action)| {
                    let source = operation.clone();
                    Button::new(("git-continue-operation", i))
                        .small()
                        .label(action.label())
                        .disabled(busy || (action == Continuation::Continue && unresolved))
                        .tooltip(if action == Continuation::Continue && unresolved {
                            "Resolve every unmerged index entry first"
                        } else {
                            "Run Git's normal operation command"
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_continuation(source.clone(), action, window, cx)
                        }))
                }),
            )
            .into_any_element()
    }
}
