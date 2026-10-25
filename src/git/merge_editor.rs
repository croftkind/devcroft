pub(super) mod journal;
use super::conflicts::{self, ConflictFile, Resolution, Source};
use crate::editor::{
    ExternalEditor, ExternalEditorKind,
    native::{file_bytes_for_buffer, language_for},
};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Editor, EditorState, InputEvent},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement,
    Render, Styled, Window, div, px, rgb,
};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(super) struct ResolveRequest {
    pub source: Source,
    pub choice: Resolution,
}
pub(super) struct MergeEditor {
    root: PathBuf,
    path: PathBuf,
    file: Option<ConflictFile>,
    loading: bool,
    error: Option<String>,
    result: Entity<EditorState>,
    current: Entity<EditorState>,
    incoming: Entity<EditorState>,
    base: Entity<EditorState>,
    saved: String,
    busy: bool,
    show_base: bool,
    block: usize,
    recovery: Option<journal::Draft>,
    journal_path: Option<PathBuf>,
    writer: Option<journal::SharedWriter>,
    external: Vec<ExternalEditorKind>,
    generation: u64,
    confirm_reload: bool,
    resolved: bool,
}
#[derive(Clone, Debug)]
pub(super) struct RecoveryChanged {
    pub path: PathBuf,
    pub exists: bool,
}
impl EventEmitter<ResolveRequest> for MergeEditor {}
impl EventEmitter<RecoveryChanged> for MergeEditor {}
impl MergeEditor {
    pub fn new(
        root: PathBuf,
        path: PathBuf,
        data_root: Option<&Path>,
        external: Vec<ExternalEditorKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let language = language_for(&path);
        let make = |window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| {
                let mut editor = EditorState::new(window, cx);
                editor.set_highlighter(language, cx);
                editor
            })
        };
        let result = make(window, cx);
        cx.subscribe(&result, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.persist(cx);
                cx.notify();
            }
        })
        .detach();
        let journal_path = data_root.map(|data| journal::path(data, &root, &path));
        let writer = journal_path.as_deref().map(journal::writer);
        cx.on_release(|this, cx| {
            if this.recovery.is_none() && this.file.is_some() {
                let _ = this.flush(cx);
            }
        })
        .detach();
        cx.on_app_quit(|this, cx| {
            let task = this.journal_snapshot(cx).map(|(path, writer, draft)| {
                cx.background_spawn(async move {
                    let _ = journal::flush(&path, &writer, draft.as_ref());
                })
            });
            async move {
                if let Some(task) = task {
                    task.await;
                }
            }
        })
        .detach();
        let mut view = Self {
            root,
            path,
            file: None,
            loading: false,
            error: None,
            result,
            current: make(window, cx),
            incoming: make(window, cx),
            base: make(window, cx),
            saved: String::new(),
            busy: false,
            show_base: false,
            block: 0,
            recovery: None,
            journal_path,
            writer,
            external,
            generation: 0,
            confirm_reload: false,
            resolved: false,
        };
        view.reload(window, cx);
        view
    }
    pub fn set_busy(&mut self, busy: bool, cx: &mut Context<Self>) {
        if self.busy != busy {
            self.busy = busy;
            cx.notify();
        }
    }
    pub fn reconcile(
        &mut self,
        index: &conflicts::IndexConflict,
        operation: &Option<conflicts::OperationDetails>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loading {
            return;
        }
        if self
            .file
            .as_ref()
            .is_some_and(|file| &file.source.index != index || &file.source.operation != operation)
        {
            if self.dirty(cx) || self.recovery.is_some() {
                self.error = Some("The index or Git operation changed. Your Result is preserved; copy it before reloading.".into());
            } else {
                self.reload(window, cx);
            }
        }
    }
    pub fn dirty(&self, cx: &App) -> bool {
        !self.resolved
            && self.file.as_ref().is_some_and(|f| f.text.is_some())
            && self.output(cx) != self.saved
    }
    pub fn has_draft(&self, cx: &App) -> bool {
        self.dirty(cx) || self.recovery.is_some()
    }
    pub fn matches_result(&self, result: &str, cx: &App) -> bool {
        self.output(cx) == result
    }
    fn output(&self, cx: &App) -> String {
        file_bytes_for_buffer(&self.saved, &self.result.read(cx).value()).into_owned()
    }
    fn draft(&self, cx: &App) -> Option<journal::Draft> {
        self.dirty(cx).then(|| journal::Draft {
            source: self.file.as_ref().unwrap().source.clone(),
            text: self.output(cx),
        })
    }
    fn journal_snapshot(
        &self,
        cx: &App,
    ) -> Option<(PathBuf, journal::SharedWriter, Option<journal::Draft>)> {
        if self.recovery.is_some() || self.file.is_none() {
            return None;
        }
        Some((
            self.journal_path.clone()?,
            self.writer.clone()?,
            self.draft(cx),
        ))
    }
    fn flush(&self, cx: &App) -> Result<(), String> {
        if let Some((path, writer, draft)) = self.journal_snapshot(cx) {
            journal::flush(&path, &writer, draft.as_ref())
        } else {
            Ok(())
        }
    }
    fn persist(&mut self, cx: &mut Context<Self>) {
        cx.emit(RecoveryChanged {
            path: self.path.clone(),
            exists: self.has_draft(cx),
        });
        let Some((path, writer, draft)) = self.journal_snapshot(cx) else {
            if self.journal_path.is_none() && self.dirty(cx) {
                self.error = Some(
                    "Recovery storage is unavailable. Copy your Result before closing.".into(),
                );
            }
            return;
        };
        let revision = journal::next(&writer);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    journal::write_latest(&path, &writer, revision, draft.as_ref())
                })
                .await;
            if let Err(error) = result {
                let _ = this.update(cx, |this, cx| {
                    this.error = Some(format!(
                        "Could not journal Result: {error}. Copy it before closing."
                    ));
                    cx.notify();
                });
            }
        })
        .detach();
    }
    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.loading = true;
        self.confirm_reload = false;
        let root = self.root.clone();
        let path = self.path.clone();
        let journal = self.journal_path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let (file, recovery) = cx
                .background_spawn(async move {
                    let file = conflicts::load(&root, &path);
                    let recovery = journal
                        .as_ref()
                        .map(|journal| journal::read(journal, &path))
                        .transpose();
                    (file, recovery)
                })
                .await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    if this.generation != generation {
                        return;
                    }
                    this.loading = false;
                    match recovery {
                        Ok(draft) => {
                            this.recovery = draft.flatten();
                            this.error = None;
                        }
                        Err(error) => this.error = Some(error),
                    }
                    match file {
                        Ok(file) => {
                            this.resolved = false;
                            if let Some(text) = &file.text {
                                this.saved = text.result.clone();
                                for (editor, value) in [
                                    (&this.result, &text.result),
                                    (&this.current, &text.current),
                                    (&this.incoming, &text.incoming),
                                    (&this.base, &text.base),
                                ] {
                                    editor.update(cx, |editor, cx| {
                                        editor.set_value(value.clone(), window, cx)
                                    });
                                }
                            }
                            this.file = Some(file);
                        }
                        Err(error) => {
                            this.file = None;
                            this.error = Some(error);
                        }
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }
    fn request_reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dirty(cx) && !self.confirm_reload {
            self.confirm_reload = true;
            cx.notify();
            return;
        }
        if self.confirm_reload {
            self.result.update(cx, |editor, cx| {
                editor.set_value(self.saved.clone(), window, cx)
            });
            self.recovery = None;
            if let Err(error) = self.flush(cx) {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        self.reload(window, cx);
    }
    fn recover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(draft) = self.recovery.clone() else {
            return;
        };
        if self
            .file
            .as_ref()
            .is_some_and(|f| f.source == draft.source && f.text.is_some())
        {
            self.recovery = None;
            self.result
                .update(cx, |editor, cx| editor.set_value(draft.text, window, cx));
            self.persist(cx);
        } else {
            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(draft.text));
            self.error=Some("The recovered draft was copied. Git or the file changed since it was written; review the current file before pasting it.".into());
        }
        cx.notify();
    }
    fn discard_recovery(&mut self, cx: &mut Context<Self>) {
        if let (Some(path), Some(writer)) = (&self.journal_path, &self.writer)
            && let Err(error) = journal::flush(path, writer, None)
        {
            self.error = Some(error);
            cx.notify();
            return;
        }
        self.recovery = None;
        self.persist(cx);
        cx.notify();
    }
    fn request(&mut self, choice: Resolution, cx: &mut Context<Self>) {
        if self.busy || self.loading {
            return;
        }
        if let Some(file) = &self.file {
            cx.emit(ResolveRequest {
                source: file.source.clone(),
                choice,
            });
        }
    }
    pub fn completed(
        &mut self,
        choice: Resolution,
        success: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unchanged =
            !matches!(&choice, Resolution::Text { result, .. } if !self.matches_result(result, cx));
        if success && unchanged && !matches!(choice, Resolution::Text { stage: false, .. }) {
            self.resolved = true;
            self.persist(cx);
            cx.notify();
            return;
        }
        if let Resolution::Text { result, .. } = choice {
            // Saving may succeed before `git add` fails. Adopt only the exact submitted disk content,
            // retain the editor and allow a retry without an unsafe overwrite.
            let root = self.root.clone();
            let path = self.path.clone();
            let submitted = result;
            cx.spawn_in(window, async move |this, cx| {
                let loaded = cx
                    .background_spawn(async move { conflicts::load(&root, &path) })
                    .await;
                let _ = cx.update(|_, cx| {
                    this.update(cx, |this, cx| {
                        if let Ok(file) = loaded
                            && file.text.as_ref().is_some_and(|t| t.result == submitted)
                            && this.file.as_ref().is_some_and(|old| {
                                old.source.index == file.source.index
                                    && old.source.operation == file.source.operation
                            })
                        {
                            this.saved = submitted;
                            this.file = Some(file);
                            this.persist(cx);
                            cx.notify();
                        }
                    })
                });
            })
            .detach();
        }
    }
    fn accept(&mut self, side: usize, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.result.read(cx).value().to_string();
        let blocks = conflicts::blocks(&text);
        let Some(block) = blocks.get(self.block.min(blocks.len().saturating_sub(1))) else {
            return;
        };
        let replacement = match side {
            0 => block.current.clone(),
            1 => block.incoming.clone(),
            _ => format!("{}{}", block.current, block.incoming),
        };
        self.result.update(cx, |editor, cx| {
            editor.set_selected_range(block.start..block.end, cx);
            editor.replace(replacement, window, cx);
            editor.focus(window, cx);
        });
        self.persist(cx);
        cx.notify();
    }
    fn regenerate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = self.file.as_ref().and_then(|f| f.text.as_ref()) {
            let value = text.generated.clone();
            self.result
                .update(cx, |editor, cx| editor.replace_all(value, window, cx));
            self.persist(cx);
            cx.notify();
        }
    }
    fn pane(
        &self,
        label: String,
        editor: &Entity<EditorState>,
        readonly: bool,
        cx: &App,
    ) -> AnyElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .border_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .px_2()
                    .py_1()
                    .flex_none()
                    .text_xs()
                    .font_semibold()
                    .child(label),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Editor::new(editor)
                        .h_full()
                        .appearance(false)
                        .readonly(readonly)
                        .aria_label(if readonly {
                            "Merge input"
                        } else {
                            "Merge Result"
                        }),
                ),
            )
            .into_any_element()
    }
    fn external_buttons(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .gap_2()
            .child(
                Button::new("merge-reveal")
                    .small()
                    .ghost()
                    .label("Open Externally…")
                    .tooltip("Reveal this path in the file manager to open with another app")
                    .on_click(
                        cx.listener(|this, _, _, cx| cx.reveal_path(&this.root.join(&this.path))),
                    ),
            )
            .children(self.external.iter().copied().enumerate().map(|(i, kind)| {
                Button::new(("merge-external", i))
                    .small()
                    .ghost()
                    .label(format!("Open in {}", kind.label()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        match ExternalEditor::new(kind, None).launch(
                            &this.root,
                            Some(&this.path),
                            None,
                        ) {
                            Ok(()) => {}
                            Err(e) => this.error = Some(format!("{e:#}")),
                        }
                        cx.notify();
                    }))
            }))
            .into_any_element()
    }
}
impl Render for MergeEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.busy || self.loading || self.resolved || self.recovery.is_some();
        let dirty = self.dirty(cx);
        let value = self.result.read(cx).value().to_string();
        let blocks = conflicts::blocks(&value);
        let block_index = self.block.min(blocks.len().saturating_sub(1));
        let body = if let Some(file) = &self.file {
            if file.text.is_some() {
                let labels = file
                    .source
                    .operation
                    .as_ref()
                    .map(|o| (o.current_label.clone(), o.incoming_label.clone()))
                    .unwrap_or_else(|| {
                        (
                            "Current · index stage 2".into(),
                            "Incoming · index stage 3".into(),
                        )
                    });
                v_flex().flex_1().min_h_0().min_w_0().gap_2()
                .child(h_flex().h(px(180.)).min_h_0().gap_2().child(self.pane(labels.0,&self.current,true,cx)).child(self.pane(labels.1,&self.incoming,true,cx)))
                .when(self.show_base,|view|view.child(div().h(px(120.)).min_h_0().child(self.pane("Base · index stage 1".into(),&self.base,true,cx))))
                .child(h_flex().flex_none().gap_2().items_center()
                    .child(div().flex_1().text_xs().child(if blocks.is_empty(){"No complete conflict blocks".into()}else{format!("Conflict {} of {} · line {}",block_index+1,blocks.len(),value[..blocks[block_index].start].bytes().filter(|b|*b==b'\n').count()+1)}))
                    .child(Button::new("merge-prev").small().ghost().label("Previous").disabled(busy||blocks.is_empty()||block_index==0).on_click(cx.listener(|this,_,_,cx|{this.block=this.block.saturating_sub(1);cx.notify();})))
                    .child(Button::new("merge-next").small().ghost().label("Next").disabled(busy||blocks.is_empty()||block_index+1>=blocks.len()).on_click(cx.listener(|this,_,_,cx|{this.block+=1;cx.notify();})))
                    .children(["Accept Current","Accept Incoming","Accept Both"].into_iter().enumerate().map(|(side,label)|Button::new(("merge-accept",side)).small().ghost().label(label).disabled(busy||blocks.is_empty()).on_click(cx.listener(move|this,_,window,cx|this.accept(side,window,cx)))))
                    .child(Button::new("merge-base").small().ghost().label("Compare with Base").on_click(cx.listener(|this,_,_,cx|{this.show_base = !this.show_base;cx.notify();}))))
                .child(self.pane(if dirty{"Result · unsaved".into()}else{"Result".into()},&self.result,busy,cx))
                .when(conflicts::marker_warning(&value),|view|view.child(div().text_xs().text_color(rgb(0xfbbf24)).child("Marker-like lines remain. Review the Result before staging; this check cannot prove every conflict is resolved.")))
                .child(h_flex().gap_2().flex_none()
                    .child(Button::new("merge-search").small().ghost().label("Search Result").on_click(cx.listener(|this,_,window,cx|{this.result.update(cx,|editor,cx|{editor.focus(window,cx);editor.open_search(false,cx);});})))
                    .child(Button::new("merge-regenerate").small().ghost().label("Generate diff3").tooltip("Replace Result with a three-way merge; Undo restores your edits").disabled(busy).on_click(cx.listener(|this,_,window,cx|this.regenerate(window,cx))))
                    .child(Button::new("merge-copy").small().ghost().label("Copy Result").on_click(cx.listener(|this,_,_,cx|cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(this.output(cx))))))
                    .child(div().flex_1())
                    .child(Button::new("merge-save").small().label("Save Result").disabled(busy||self.recovery.is_some()).on_click(cx.listener(|this,_,_,cx|this.request(Resolution::Text{result:this.output(cx),stage:false},cx))))
                    .child(Button::new("merge-resolved").small().primary().label("Mark Resolved").disabled(busy||self.recovery.is_some()).on_click(cx.listener(|this,_,_,cx|this.request(Resolution::Text{result:this.output(cx),stage:true},cx)))))
                .into_any_element()
            } else {
                v_flex()
                    .flex_1()
                    .gap_3()
                    .p_4()
                    .child(div().text_sm().child(file.reason.clone()))
                    .child(div().text_xs().child(format!(
                        "Base: {} · Current: {} · Incoming: {}",
                        stage_label(file, 0),
                        stage_label(file, 1),
                        stage_label(file, 2)
                    )))
                    .child(
                        h_flex()
                            .gap_2()
                            .children(
                                [(Resolution::Current, 1), (Resolution::Incoming, 2)]
                                    .into_iter()
                                    .enumerate()
                                    .map(|(i, (choice, stage))| {
                                        Button::new(("merge-whole", i))
                                            .small()
                                            .label(choice.label())
                                            .disabled(
                                                busy || file.source.index.stages[stage].is_none(),
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.request(choice.clone(), cx)
                                            }))
                                    }),
                            )
                            .child(
                                Button::new("merge-delete")
                                    .small()
                                    .ghost()
                                    .label("Delete")
                                    .disabled(
                                        busy || matches!(
                                            file.source.disk,
                                            conflicts::Baseline::Directory
                                        ) || file
                                            .source
                                            .index
                                            .stages
                                            .iter()
                                            .flatten()
                                            .any(|s| s.mode == "160000"),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.request(Resolution::Delete, cx)
                                    })),
                            ),
                    )
                    .child(self.external_buttons(cx))
                    .into_any_element()
            }
        } else {
            v_flex()
                .flex_1()
                .p_4()
                .child(if self.loading {
                    "Loading conflict inputs…"
                } else {
                    "Conflict inputs unavailable"
                })
                .child(self.external_buttons(cx))
                .into_any_element()
        };
        v_flex().size_full().min_h_0().min_w_0().gap_2().p_2()
            .child(h_flex().gap_2().items_center().flex_none().child(div().flex_1().text_sm().font_semibold().child(self.path.to_string_lossy().into_owned()))
                .child(Button::new("merge-reload").small().ghost().label(if self.confirm_reload{"Discard draft and reload"}else{"Reload Disk"}).disabled(busy).on_click(cx.listener(|this,_,window,cx|this.request_reload(window,cx)))))
            .when_some(self.file.as_ref().and_then(|f|f.source.operation.as_ref()),|view,op|view.child(div().text_xs().flex_none().text_color(cx.theme().muted_foreground).child(op.explanation.clone())))
            .when_some(self.error.as_ref(),|view,error|view.child(div().text_xs().flex_none().text_color(rgb(0xf87171)).child(error.clone())))
            .when_some(self.recovery.as_ref(),|view,draft|{
                let compatible=self.file.as_ref().is_some_and(|f|f.source==draft.source&&f.text.is_some());
                view.child(h_flex().gap_2().flex_none().child(div().flex_1().text_xs().child(if compatible{"An unsaved Result is available for recovery."}else{"A saved draft belongs to an earlier file/index state. Copy it to review safely."}))
                    .child(Button::new("merge-recover").small().label(if compatible{"Recover Result"}else{"Copy Recovery Draft"}).disabled(self.busy || self.loading).on_click(cx.listener(|this,_,window,cx|this.recover(window,cx))))
                    .child(Button::new("merge-discard-recovery").small().ghost().label("Discard Recovery").disabled(self.busy || self.loading).on_click(cx.listener(|this,_,_,cx|{this.discard_recovery(cx);}))))
            }).child(body)
    }
}
fn stage_label(file: &ConflictFile, index: usize) -> String {
    file.source.index.stages[index]
        .as_ref()
        .map(|s| format!("{} {}", s.mode, &s.oid[..10]))
        .unwrap_or_else(|| "deleted / absent".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::conflicts::tests::{setup, text_conflict};
    use std::{cell::RefCell, rc::Rc};
    fn install(
        view: &mut MergeEditor,
        file: ConflictFile,
        window: &mut Window,
        cx: &mut Context<MergeEditor>,
    ) {
        view.generation += 1;
        view.loading = false;
        let text = file.text.as_ref().unwrap();
        view.saved = text.result.clone();
        for (editor, value) in [
            (&view.result, &text.result),
            (&view.current, &text.current),
            (&view.incoming, &text.incoming),
            (&view.base, &text.base),
        ] {
            editor.update(cx, |editor, cx| editor.set_value(value.clone(), window, cx));
        }
        view.file = Some(file);
        cx.notify();
    }
    #[gpui_kit::test]
    fn accepting_a_block_is_undoable_and_never_changes_inputs_or_disk(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        let original = "prefix\n<<<<<<< HEAD\nবাংলা\n=======\nincoming\n>>>>>>> other\nsuffix\n";
        std::fs::write(root.join("file.txt"), original).unwrap();
        let file = conflicts::load(root, Path::new("file.txt")).unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let checkout = root.to_owned();
        cx.update(gpui_kit::init);
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let editor = cx.new(|cx| {
                MergeEditor::new(checkout, "file.txt".into(), None, Vec::new(), window, cx)
            });
            editor.update(cx, |view, cx| install(view, file, window, cx));
            *slot.borrow_mut() = Some(editor.clone());
            gpui_kit::component::Root::new(editor, window, cx)
        });
        let editor = holder.borrow().clone().unwrap();
        gpui_kit::VisualTestContext::update(test_cx, |window, cx| {
            editor.update(cx, |view, cx| view.accept(2, window, cx));
            window.draw(cx).clear(cx);
        });
        editor.read_with(test_cx, |view, cx| {
            assert_eq!(view.output(cx), "prefix\nবাংলা\nincoming\nsuffix\n");
            assert_eq!(view.current.read(cx).value().as_ref(), "current\n");
            assert_eq!(view.incoming.read(cx).value().as_ref(), "incoming\n");
            assert!(view.dirty(cx));
        });
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            original
        );
        test_cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        editor.read_with(test_cx, |view, cx| assert_eq!(view.output(cx), original));
        test_cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-y"
        });
        editor.read_with(test_cx, |view, cx| {
            assert_eq!(view.output(cx), "prefix\nবাংলা\nincoming\nsuffix\n")
        });
    }
    #[gpui_kit::test]
    fn recovery_prompt_protects_input_and_recovery_never_changes_disk(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        let file = conflicts::load(root, Path::new("file.txt")).unwrap();
        let original = file.text.as_ref().unwrap().result.clone();
        let recovery = journal::Draft {
            source: file.source.clone(),
            text: "recovered result\n".into(),
        };
        let data = tempfile::tempdir().unwrap();
        let journal_path = journal::path(data.path(), root, Path::new("file.txt"));
        journal::flush(
            &journal_path,
            &journal::writer(&journal_path),
            Some(&recovery),
        )
        .unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let checkout = root.to_owned();
        let data_root = data.path().to_owned();
        cx.update(gpui_kit::init);
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let editor = cx.new(|cx| {
                MergeEditor::new(
                    checkout,
                    "file.txt".into(),
                    Some(&data_root),
                    Vec::new(),
                    window,
                    cx,
                )
            });
            editor.update(cx, |view, cx| {
                install(view, file, window, cx);
                view.recovery = Some(recovery);
                view.result
                    .update(cx, |editor, cx| editor.focus(window, cx));
            });
            *slot.borrow_mut() = Some(editor.clone());
            gpui_kit::component::Root::new(editor, window, cx)
        });
        let editor = holder.borrow().clone().unwrap();
        gpui_kit::VisualTestContext::update(test_cx, |window, cx| window.draw(cx).clear(cx));
        test_cx.simulate_keystrokes("x");
        editor.read_with(test_cx, |view, cx| assert_eq!(view.output(cx), original));
        gpui_kit::VisualTestContext::update(test_cx, |window, cx| {
            editor.update(cx, |view, cx| {
                view.recover(window, cx);
                assert!(view.recovery.is_none());
                assert_eq!(view.output(cx), "recovered result\n");
                view.flush(cx).unwrap();
            });
            window.draw(cx).clear(cx);
        });
        assert_eq!(
            std::fs::read_to_string(root.join("file.txt")).unwrap(),
            original
        );
        assert_eq!(
            journal::read(&journal_path, Path::new("file.txt"))
                .unwrap()
                .unwrap()
                .text,
            "recovered result\n"
        );
    }
    #[gpui_kit::test]
    fn resolution_completion_preserves_a_newer_result(cx: &mut gpui_kit::TestAppContext) {
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        let file = conflicts::load(root, Path::new("file.txt")).unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let checkout = root.to_owned();
        cx.update(gpui_kit::init);
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let editor = cx.new(|cx| {
                MergeEditor::new(checkout, "file.txt".into(), None, Vec::new(), window, cx)
            });
            editor.update(cx, |view, cx| install(view, file, window, cx));
            *slot.borrow_mut() = Some(editor.clone());
            gpui_kit::component::Root::new(editor, window, cx)
        });
        let editor = holder.borrow().clone().unwrap();
        gpui_kit::VisualTestContext::update(test_cx, |window, cx| {
            editor.update(cx, |view, cx| {
                view.result.update(cx, |editor, cx| {
                    editor.set_value("newer draft\n", window, cx)
                });
                view.completed(
                    Resolution::Text {
                        result: "submitted\n".into(),
                        stage: true,
                    },
                    true,
                    window,
                    cx,
                );
                assert!(!view.resolved);
                assert!(view.dirty(cx));
                assert_eq!(view.output(cx), "newer draft\n");
            })
        });
    }

    #[gpui_kit::test]
    fn orphaned_recovery_can_be_copied_and_explicitly_discarded(cx: &mut gpui_kit::TestAppContext) {
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        let file = conflicts::load(root, Path::new("file.txt")).unwrap();
        let recovery = journal::Draft {
            source: file.source,
            text: "orphaned draft\n".into(),
        };
        let data = tempfile::tempdir().unwrap();
        let journal_path = journal::path(data.path(), root, Path::new("file.txt"));
        journal::flush(
            &journal_path,
            &journal::writer(&journal_path),
            Some(&recovery),
        )
        .unwrap();
        let checkout = root.to_owned();
        let data_root = data.path().to_owned();
        cx.update(gpui_kit::init);
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let editor = cx.new(|cx| {
                MergeEditor::new(
                    checkout,
                    "file.txt".into(),
                    Some(&data_root),
                    Vec::new(),
                    window,
                    cx,
                )
            });
            editor.update(cx, |view, cx| {
                view.generation += 1;
                view.loading = false;
                view.file = None;
                view.recovery = Some(recovery);
                view.recover(window, cx);
                assert!(view.recovery.is_some());
                assert!(view.has_draft(cx));
                assert!(
                    journal::read(&journal_path, Path::new("file.txt"))
                        .unwrap()
                        .is_some()
                );
                view.discard_recovery(cx);
                assert!(!view.has_draft(cx));
                assert!(!journal_path.exists());
            });
            gpui_kit::component::Root::new(editor, window, cx)
        });
        gpui_kit::VisualTestContext::update(test_cx, |window, cx| window.draw(cx).clear(cx));
    }
}
