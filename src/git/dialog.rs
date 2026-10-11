mod conflict_ui;
mod history_ui;
mod remote_ui;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState, Textarea, TextareaState},
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, Styled, Subscription, Window, div, px,
    rgb,
};

use crate::{diff::viewer::DiffViewer, git_status::GitStatus};

use super::{
    diff::{ChangeArea, DiffTarget, load_change_diff},
    model::{Change, RepositorySnapshot},
    operations::{
        Mutation, TargetFingerprint, capture_restore_target, capture_trash_target, execute,
        trash_available,
    },
    repository::{LoadError, load_snapshot},
    watch::FileChanges,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const MAX_VISIBLE_CHANGES: usize = 250;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GitMode {
    Changes,
    Branches,
    History,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum LoadState {
    Loading,
    Loaded(Box<RepositorySnapshot>),
    NotRepository,
    GitUnavailable,
    Failed(String),
}

#[derive(Clone, Debug)]
pub(crate) struct SnapshotChanged {
    pub(crate) checkout: PathBuf,
    pub(crate) status: GitStatus,
}

#[derive(Clone, Debug)]
pub(crate) struct OpenGitFile {
    pub(crate) checkout: PathBuf,
    pub(crate) path: PathBuf,
}

#[derive(Clone, Debug)]
pub(crate) struct CommitDraftChanged {
    pub(crate) checkout: PathBuf,
    pub(crate) message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Confirmation {
    Remote {
        mutation: Mutation,
        title: String,
        detail: String,
    },
    Restore {
        change: Change,
        fingerprint: TargetFingerprint,
    },
    Trash {
        path: PathBuf,
        fingerprint: TargetFingerprint,
    },
}

#[derive(Clone, Debug)]
struct OperationNotice {
    message: String,
    error: bool,
}

pub(crate) struct GitDialog {
    focus_handle: FocusHandle,
    _tab_interceptor: Subscription,
    checkout: PathBuf,
    mode: GitMode,
    state: LoadState,
    generation: u64,
    load_in_flight: bool,
    file_changes: FileChanges,
    selection: Option<DiffTarget>,
    visible_changes: [usize; 4],
    diff_generation: u64,
    diff_viewer: Entity<DiffViewer>,
    commit_message: Entity<TextareaState>,
    mutation_queue: VecDeque<Mutation>,
    active_mutation: Option<Mutation>,
    mutation_generation: u64,
    notice: Option<OperationNotice>,
    confirmation: Option<Confirmation>,
    dirty_editor_paths: HashSet<PathBuf>,
    branch_query: Entity<InputState>,
    branch_name: Entity<InputState>,
    push_branch: Entity<InputState>,
    selected_ref: Option<String>,
    selected_remote: Option<String>,
    remote_form: Option<remote_ui::RemoteForm>,
    active_started: Option<Instant>,
    check_conflicts: bool,
    continuation_loading: bool,
    recovery_paths: HashSet<PathBuf>,
    recovery_warning: Option<String>,
    history: history_ui::HistoryState,
    merge_editors: HashMap<PathBuf, Entity<super::merge_editor::MergeEditor>>,
    data_root: Option<PathBuf>,
    external_editors: Vec<crate::editor::ExternalEditorKind>,
}

impl EventEmitter<SnapshotChanged> for GitDialog {}
impl EventEmitter<OpenGitFile> for GitDialog {}
impl EventEmitter<CommitDraftChanged> for GitDialog {}

impl GitDialog {
    pub(crate) fn new(
        checkout: PathBuf,
        dirty_editor_paths: HashSet<PathBuf>,
        data_root: Option<PathBuf>,
        external_editors: Vec<crate::editor::ExternalEditorKind>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let commit_message = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 7)
                .placeholder("Commit message")
        });
        cx.subscribe(
            &commit_message,
            |this, _, event: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                    this.publish_commit_draft(cx);
                }
                cx.notify();
            },
        )
        .detach();
        let branch_query = cx.new(|cx| InputState::new(window, cx).placeholder("Search branches…"));
        let branch_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("New local branch name"));
        let push_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder("Remote branch name"));
        for input in [&branch_query, &branch_name, &push_branch] {
            cx.observe(input, |_, _, cx| cx.notify()).detach();
        }
        let history = history_ui::HistoryState::new(window, cx);
        cx.subscribe(
            &history.query,
            |this, _, event: &gpui_kit::component::input::InputEvent, cx| {
                if matches!(event, gpui_kit::component::input::InputEvent::Change) {
                    this.history_filter_changed(cx);
                }
            },
        )
        .detach();
        cx.observe(&history.branch_name, |_, _, cx| cx.notify())
            .detach();
        let focus_handle = cx.focus_handle();
        let tab_focus = focus_handle.clone();
        let target_window = window.window_handle();
        // Intercept before input bindings resolve: code editors otherwise use
        // Tab for indentation, trapping keyboard users inside Result.
        let tab_interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            let key = &event.keystroke;
            let modifiers = &key.modifiers;
            if window.window_handle() == target_window
                && tab_focus.contains_focused(window, cx)
                && key.key == "tab"
                && !modifiers.platform
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.function
            {
                if modifiers.shift {
                    window.focus_prev(cx);
                } else {
                    window.focus_next(cx);
                }
                window.prevent_default();
                cx.stop_propagation();
            }
        });
        let mut dialog = Self {
            focus_handle,
            _tab_interceptor: tab_interceptor,
            file_changes: FileChanges::new(&checkout),
            checkout,
            mode: GitMode::Changes,
            state: LoadState::Loading,
            generation: 0,
            load_in_flight: false,
            selection: None,
            visible_changes: [MAX_VISIBLE_CHANGES; 4],
            diff_generation: 0,
            diff_viewer: cx.new(DiffViewer::new),
            commit_message,
            mutation_queue: VecDeque::new(),
            active_mutation: None,
            mutation_generation: 0,
            notice: None,
            confirmation: None,
            dirty_editor_paths,
            branch_query,
            branch_name,
            push_branch,
            selected_ref: None,
            selected_remote: None,
            remote_form: None,
            active_started: None,
            check_conflicts: false,
            continuation_loading: false,
            recovery_paths: HashSet::new(),
            recovery_warning: None,
            history,
            merge_editors: HashMap::new(),
            data_root,
            external_editors,
        };
        dialog.discover_conflict_drafts(cx);
        dialog.reload(false, cx);
        dialog.start_refresh_loop(cx);
        dialog
    }

    pub(crate) fn request_close(&mut self, cx: &mut Context<Self>) -> bool {
        if self.active_mutation.is_some() || !self.mutation_queue.is_empty() {
            self.notice = Some(OperationNotice {
                message: "Wait for the active Git operation to finish before closing.".into(),
                error: false,
            });
            cx.notify();
            false
        } else {
            true
        }
    }

    fn commit_completed(&mut self, submitted: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.commit_message.read(cx).value().as_ref() == submitted {
            self.commit_message
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.publish_commit_draft(cx);
    }

    fn publish_commit_draft(&self, cx: &mut Context<Self>) {
        cx.emit(CommitDraftChanged {
            checkout: self.checkout.clone(),
            message: self.commit_message.read(cx).value().to_string(),
        });
    }

    pub(crate) fn restore_commit_message(
        &mut self,
        message: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_message
            .update(cx, |input, cx| input.set_value(message, window, cx));
    }

    fn start_refresh_loop(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH_INTERVAL).await;
                if this
                    .update(cx, |this, cx| this.refresh_if_changed(cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    fn refresh_if_changed(&mut self, cx: &mut Context<Self>) {
        if self.active_mutation.is_some() {
            cx.notify();
            return;
        }
        if !self.load_in_flight && self.file_changes.take_changed() {
            self.reload(true, cx);
        }
    }

    fn reload(&mut self, background: bool, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        self.load_in_flight = true;
        self.file_changes.take_changed();
        if !background || !matches!(self.state, LoadState::Loaded(_)) {
            self.state = LoadState::Loading;
            cx.notify();
        }
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { load_snapshot(&checkout) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.load_in_flight = false;
                match result {
                    Ok(snapshot) => {
                        let changed = !matches!(&this.state, LoadState::Loaded(current) if current.as_ref() == &snapshot);
                        let status = snapshot.header_status();
                        if this.check_conflicts {
                            this.check_conflicts = false;
                            if !snapshot.conflicts.is_empty() || snapshot.operation.is_some() {
                                this.mode = GitMode::Changes;
                                this.selection = None;
                            }
                        }
                        if this.selected_ref.as_ref().is_some_and(|reference| !snapshot.branches.iter().any(|branch| &branch.reference == reference)) {
                            this.selected_ref = None;
                        }
                        let history_changed = !matches!(&this.state, LoadState::Loaded(current) if current.oid == snapshot.oid && current.branches == snapshot.branches && current.tag_state == snapshot.tag_state);
                        let selection = this.selection.take();
                        let preserve = selection.as_ref().is_some_and(|target| target.area==ChangeArea::Conflict && this.merge_editors.get(&target.change.path).is_some_and(|editor| editor.read(cx).has_draft(cx)));
                        this.selection = if preserve {selection} else {reconcile_selection(selection, &snapshot)};
                        this.state = LoadState::Loaded(Box::new(snapshot));
                        this.load_selected_diff(cx);
                        if history_changed { this.reset_history(cx); }
                        if changed {
                            cx.emit(SnapshotChanged {
                                checkout: this.checkout.clone(),
                                status,
                            });
                        }
                    }
                    Err(LoadError::NotRepository) => this.fail_load(LoadState::NotRepository, cx),
                    Err(LoadError::GitUnavailable) => {
                        this.fail_load(LoadState::GitUnavailable, cx)
                    }
                    Err(LoadError::Failed(message)) => {
                        this.fail_load(LoadState::Failed(message), cx)
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn fail_load(&mut self, state: LoadState, cx: &mut Context<Self>) {
        self.state = state;
        self.reset_history(cx);
        self.selection = None;
        self.diff_generation += 1;
        self.diff_viewer.update(cx, |viewer, cx| viewer.clear(cx));
    }

    fn select_change(&mut self, target: DiffTarget, cx: &mut Context<Self>) {
        if self.selection.as_ref() == Some(&target) {
            return;
        }
        self.selection = Some(target);
        self.confirmation = None;
        self.load_selected_diff(cx);
        cx.notify();
    }

    fn load_selected_diff(&mut self, cx: &mut Context<Self>) {
        self.diff_generation += 1;
        let generation = self.diff_generation;
        let Some(target) = self.selection.clone() else {
            self.diff_viewer.update(cx, |viewer, cx| viewer.clear(cx));
            return;
        };
        if target.area == ChangeArea::Conflict {
            return;
        }
        let label = target.label();
        self.diff_viewer
            .update(cx, |viewer, cx| viewer.set_loading(label.clone(), cx));
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { load_change_diff(&checkout, &target) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.diff_generation != generation {
                    return;
                }
                match result {
                    Ok(diff) => this
                        .diff_viewer
                        .update(cx, |viewer, cx| viewer.set_loaded(diff, cx)),
                    Err(message) => this.diff_viewer.update(cx, |viewer, cx| {
                        viewer.set_error(label.clone(), message, cx)
                    }),
                }
            });
        })
        .detach();
    }

    fn enqueue_mutation(
        &mut self,
        mutation: Mutation,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(mutation, Mutation::Continue { .. })
            && self
                .merge_editors
                .values()
                .any(|editor| editor.read(cx).dirty(cx))
        {
            self.notice = Some(OperationNotice {
                message: "Save or discard unsaved merge Results before changing the Git operation."
                    .into(),
                error: true,
            });
            cx.notify();
            return;
        }
        if matches!(mutation, Mutation::Continue { .. }) && !self.dirty_editor_paths.is_empty() {
            self.notice = Some(OperationNotice { message: "Save or discard unsaved editor drafts before continuing, aborting, or skipping a Git operation.".into(), error: true });
            cx.notify();
            return;
        }
        if let Mutation::Resolve { source, .. } = &mutation
            && self.is_dirty_in_editor(&source.index.path)
        {
            self.notice = Some(OperationNotice {
                message:
                    "Save or discard this file's built-in editor draft before resolving it in Git."
                        .into(),
                error: true,
            });
            cx.notify();
            return;
        }
        if let Mutation::Remote { action, .. } = &mutation
            && action.changes_worktree()
            && !self.dirty_editor_paths.is_empty()
        {
            self.notice = Some(OperationNotice {
                message: "Save or discard unsaved editor drafts before changing the working tree."
                    .into(),
                error: true,
            });
            cx.notify();
            return;
        }
        self.confirmation = None;
        self.mutation_queue.push_back(mutation);
        self.start_next_mutation(window, cx);
    }

    fn start_next_mutation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_mutation.is_some() {
            return;
        }
        let Some(mutation) = self.mutation_queue.pop_front() else {
            return;
        };
        self.mutation_generation += 1;
        let generation = self.mutation_generation;
        self.notice = Some(OperationNotice {
            message: mutation.progress(),
            error: false,
        });
        self.active_mutation = Some(mutation.clone());
        self.active_started = Some(Instant::now());
        let checkout = self.checkout.clone();
        cx.spawn_in(window, async move |this, cx| {
            let task_mutation = mutation.clone();
            let result = cx
                .background_spawn(async move { execute(&checkout, &task_mutation) })
                .await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    if this.mutation_generation != generation {
                        return;
                    }
                    this.active_mutation = None;
                    this.active_started = None;
                    this.check_conflicts = matches!(
                        mutation,
                        Mutation::Remote { .. } | Mutation::Continue { .. }
                    );
                    if let Mutation::Resolve { source, choice } = &mutation
                        && let Some(editor) = this.merge_editors.get(&source.index.path)
                    {
                        editor.update(cx, |editor, cx| {
                            editor.completed(choice.clone(), result.is_ok(), window, cx)
                        });
                    }
                    match result {
                        Ok(message) => {
                            if let Mutation::Commit { message: submitted } = &mutation {
                                this.commit_completed(submitted, window, cx);
                            }
                            this.notice = Some(OperationNotice {
                                message,
                                error: false,
                            });
                            this.remote_form = None;
                        }
                        Err(message) => {
                            this.notice = Some(OperationNotice {
                                message,
                                error: true,
                            });
                        }
                    }
                    // A failed pull/rebase may still fetch refs or enter conflict state.
                    this.file_changes.mark_changed();
                    this.reload(true, cx);
                    this.start_next_mutation(window, cx);
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn stage_change(&mut self, change: &Change, window: &mut Window, cx: &mut Context<Self>) {
        self.enqueue_mutation(
            Mutation::Stage {
                paths: change_paths(change),
            },
            window,
            cx,
        );
    }

    fn unstage_change(&mut self, change: &Change, window: &mut Window, cx: &mut Context<Self>) {
        let unborn = matches!(&self.state, LoadState::Loaded(snapshot) if snapshot.unborn);
        self.enqueue_mutation(
            Mutation::Unstage {
                paths: change_paths(change),
                unborn,
            },
            window,
            cx,
        );
    }

    fn request_restore(&mut self, change: Change, cx: &mut Context<Self>) {
        let dirty = change_paths(&change)
            .into_iter()
            .find(|path| self.is_dirty_in_editor(path));
        if let Some(path) = dirty {
            self.notice = Some(OperationNotice {
                message: format!(
                    "{} has an unsaved editor draft. Save or discard that draft before replacing the working-tree file.",
                    path.display()
                ),
                error: true,
            });
        } else {
            match capture_restore_target(&self.checkout, &change.path) {
                Ok(fingerprint) => {
                    self.confirmation = Some(Confirmation::Restore {
                        change,
                        fingerprint,
                    });
                }
                Err(message) => {
                    self.notice = Some(OperationNotice {
                        message,
                        error: true,
                    });
                }
            }
        }
        cx.notify();
    }

    fn request_trash(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.is_dirty_in_editor(&path) {
            self.notice = Some(OperationNotice {
                message: format!(
                    "{} has an unsaved editor draft. Save or discard that draft before moving the file to Trash.",
                    path.display()
                ),
                error: true,
            });
        } else {
            match capture_trash_target(&self.checkout, &path) {
                Ok(fingerprint) => {
                    self.confirmation = Some(Confirmation::Trash { path, fingerprint });
                }
                Err(message) => {
                    self.notice = Some(OperationNotice {
                        message,
                        error: true,
                    });
                }
            }
        }
        cx.notify();
    }

    fn confirm_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(confirmation) = self.confirmation.take() else {
            return;
        };
        let mutation = match confirmation {
            Confirmation::Remote { mutation, .. } => mutation,
            Confirmation::Restore {
                change,
                fingerprint,
            } => Mutation::Restore {
                change,
                fingerprint,
            },
            Confirmation::Trash { path, fingerprint } => Mutation::Trash { path, fingerprint },
        };
        if let Mutation::Resolve {
            source,
            choice: super::conflicts::Resolution::Text { result, .. },
        } = &mutation
            && self
                .merge_editors
                .get(&source.index.path)
                .is_some_and(|editor| !editor.read(cx).matches_result(result, cx))
        {
            self.notice = Some(OperationNotice { message: "Result changed after the confirmation opened. Review it and mark it resolved again.".into(), error: true });
            cx.notify();
            return;
        }
        self.enqueue_mutation(mutation, window, cx);
    }

    fn is_dirty_in_editor(&self, path: &Path) -> bool {
        let absolute = self.checkout.join(path);
        self.dirty_editor_paths.contains(&absolute)
            || absolute
                .canonicalize()
                .is_ok_and(|canonical| self.dirty_editor_paths.contains(&canonical))
    }

    fn open_file(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if !self.request_close(cx) {
            return;
        }
        if !self.checkout.join(&path).is_file() {
            self.notice = Some(OperationNotice {
                message: format!("{} is unavailable in the working tree.", path.display()),
                error: true,
            });
            cx.notify();
            return;
        }
        cx.emit(OpenGitFile {
            checkout: self.checkout.clone(),
            path,
        });
        window.close_dialog(cx);
    }

    fn summary(&self) -> String {
        match &self.state {
            LoadState::Loaded(snapshot) => {
                let mut parts = vec![snapshot.branch_label()];
                if snapshot.ahead > 0 {
                    parts.push(format!("↑{}", snapshot.ahead));
                }
                if snapshot.behind > 0 {
                    parts.push(format!("↓{}", snapshot.behind));
                }
                if let Some(operation) = snapshot.operation {
                    parts.push(operation.label().to_owned());
                }
                parts.join("  ·  ")
            }
            _ => String::new(),
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(44.))
            .flex_none()
            .px_3()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex().gap_1().children(
                    [
                        (GitMode::Changes, "Changes"),
                        (GitMode::Branches, "Branches"),
                        (GitMode::History, "History"),
                    ]
                    .into_iter()
                    .map(|(mode, label)| {
                        Button::new(("git-dialog-mode", mode as usize))
                            .small()
                            .ghost()
                            .label(label)
                            .selected(self.mode == mode)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mode = mode;
                                if mode == GitMode::History && this.history.commits.is_empty() {
                                    this.load_history_page(cx);
                                }
                                cx.notify();
                            }))
                    }),
                ),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.summary()),
            )
            .child(
                Button::new("git-dialog-refresh")
                    .small()
                    .ghost()
                    .icon(IconName::RotateCw)
                    .tooltip("Refresh repository")
                    .accessibility_label("Refresh repository")
                    .disabled(self.active_mutation.is_some())
                    .loading(self.load_in_flight)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.file_changes.mark_changed();
                        this.reload(true, cx);
                        this.reset_history(cx);
                    })),
            )
    }

    fn render_loaded(&self, snapshot: &RepositorySnapshot, cx: &mut Context<Self>) -> AnyElement {
        match self.mode {
            GitMode::Changes => self.render_changes(snapshot, cx),
            GitMode::Branches => self.render_branches(snapshot, cx),
            GitMode::History => self.render_history(snapshot, cx),
        }
    }

    fn render_changes(&self, snapshot: &RepositorySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.active_mutation.is_some();
        let has_conflicts = !snapshot.conflicts.is_empty();
        h_flex()
            .size_full()
            .min_h_0()
            .min_w_0()
            .child(
                v_flex()
                    .w(px(390.))
                    .h_full()
                    .min_h_0()
                    .flex_none()
                    .border_r_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .h(px(40.))
                            .flex_none()
                            .items_center()
                            .gap_2()
                            .px_3()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .flex_1()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("Changes · {}", snapshot.change_count())),
                            )
                            .child(
                                Button::new("git-stage-all")
                                    .small()
                                    .ghost()
                                    .label("Stage All")
                                    .disabled(
                                        busy
                                            || has_conflicts
                                            || (snapshot.unstaged.is_empty()
                                                && snapshot.untracked.is_empty()),
                                    )
                                    .tooltip(if has_conflicts {
                                        "Resolve merge changes before staging all files"
                                    } else {
                                        "Stage every changed file"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.enqueue_mutation(Mutation::StageAll, window, cx)
                                    })),
                            )
                            .child(
                                Button::new("git-unstage-all")
                                    .small()
                                    .ghost()
                                    .label("Unstage All")
                                    .disabled(
                                        busy || has_conflicts || snapshot.staged.is_empty(),
                                    )
                                    .tooltip(if has_conflicts {
                                        "Resolve merge changes before unstaging all files"
                                    } else {
                                        "Unstage every staged file"
                                    })
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        let unborn = matches!(&this.state, LoadState::Loaded(snapshot) if snapshot.unborn);
                                        this.enqueue_mutation(
                                            Mutation::UnstageAll { unborn },
                                            window,
                                            cx,
                                        )
                                    })),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scrollbar()
                            .child(
                                v_flex()
                                    .w_full()
                                    .p_2()
                                    .gap_3()
                                    .child(self.render_change_section(
                                        "Merge Changes",
                                        ChangeArea::Conflict,
                                        &snapshot.conflicts,
                                        0xf87171,
                                        busy,
                                        cx,
                                    ))
                                    .child(self.render_orphaned_results(snapshot, cx))
                                    .child(self.render_change_section(
                                        "Staged Changes",
                                        ChangeArea::Staged,
                                        &snapshot.staged,
                                        0x4ade80,
                                        busy,
                                        cx,
                                    ))
                                    .child(self.render_change_section(
                                        "Changes",
                                        ChangeArea::Unstaged,
                                        &snapshot.unstaged,
                                        0xfbbf24,
                                        busy,
                                        cx,
                                    ))
                                    .child(self.render_change_section(
                                        "Untracked Files",
                                        ChangeArea::Untracked,
                                        &snapshot.untracked,
                                        0x60a5fa,
                                        busy,
                                        cx,
                                    )),
                            ),
                    )
                    .child(self.render_commit_panel(snapshot, cx)),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .min_h_0()
                    .child(self.render_diff_header(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .min_w_0()
                            .child(self.selected_change_view()),
                    ),
            )
            .into_any_element()
    }

    fn render_change_section(
        &self,
        title: &str,
        area: ChangeArea,
        changes: &[Change],
        color: u32,
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible = changes.len().min(self.visible_changes[area as usize]);
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_1()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(title.to_owned())
                    .child(changes.len().to_string()),
            )
            .children(
                changes
                    .iter()
                    .take(visible)
                    .cloned()
                    .enumerate()
                    .map(|(index, change)| {
                        let target = DiffTarget {
                            area,
                            change: change.clone(),
                        };
                        let selected = self.selection.as_ref() == Some(&target);
                        let select_target = target.clone();
                        let path = change.path.to_string_lossy().into_owned();
                        let mut row = h_flex()
                            .id(("git-change-row", index * 4 + area as usize))
                            .w_full()
                            .min_h(px(40.))
                            .px_2()
                            .py_1()
                            .gap_2()
                            .items_center()
                            .rounded_sm()
                            .cursor_pointer()
                            .when(selected, |row| row.bg(rgb(0x203442)))
                            .hover(|style| style.bg(rgb(0x202222)))
                            .child(
                                div()
                                    .w(px(16.))
                                    .flex_none()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(rgb(color))
                                    .child(change.kind.label()),
                            )
                            .child(
                                Button::new(("git-select-file", index * 4 + area as usize))
                                    .ghost()
                                    .flex_1()
                                    .min_w_0()
                                    .label(if let Some(original) = &change.original_path {
                                        format!("{path} (from {})", original.to_string_lossy())
                                    } else {
                                        path
                                    })
                                    .selected(selected)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_change(select_target.clone(), cx);
                                    })),
                            );
                        row = match area {
                            ChangeArea::Staged => {
                                let action = change.clone();
                                row.child(
                                    Button::new(("git-unstage-file", index))
                                        .xsmall()
                                        .ghost()
                                        .label("Unstage")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.unstage_change(&action, window, cx)
                                        })),
                                )
                            }
                            ChangeArea::Unstaged => {
                                let stage = change.clone();
                                let discard = change.clone();
                                row.child(
                                    Button::new(("git-stage-file", index))
                                        .xsmall()
                                        .ghost()
                                        .label("Stage")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.stage_change(&stage, window, cx)
                                        })),
                                )
                                .when(
                                    discard_supported(&discard),
                                    |row| {
                                        row.child(
                                            Button::new(("git-discard-file", index))
                                                .xsmall()
                                                .ghost()
                                                .label("Discard")
                                                .disabled(busy)
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.request_restore(discard.clone(), cx)
                                                })),
                                        )
                                    },
                                )
                            }
                            ChangeArea::Untracked => {
                                let stage = change.clone();
                                let trash = change.path.clone();
                                row.child(
                                    Button::new(("git-stage-untracked", index))
                                        .xsmall()
                                        .ghost()
                                        .label("Stage")
                                        .disabled(busy)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.stage_change(&stage, window, cx)
                                        })),
                                )
                                .when(trash_available(), |row| {
                                    row.child(
                                        Button::new(("git-trash-file", index))
                                            .xsmall()
                                            .ghost()
                                            .label("Trash")
                                            .disabled(busy)
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.request_trash(trash.clone(), cx)
                                            })),
                                    )
                                })
                            }
                            ChangeArea::Conflict => row,
                        };
                        row
                    }),
            )
            .when(changes.is_empty(), |view| {
                view.child(
                    div()
                        .px_2()
                        .py_1()
                        .text_xs()
                        .text_color(rgb(0x555a5a))
                        .child("No files"),
                )
            })
            .when(changes.len() > visible, |view| {
                view.child(
                    Button::new(("git-more-files", area as usize))
                        .small()
                        .ghost()
                        .label(format!("Show more · {} remaining", changes.len() - visible))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.visible_changes[area as usize] += MAX_VISIBLE_CHANGES;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    fn render_commit_panel(
        &self,
        snapshot: &RepositorySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let message = self.commit_message.read(cx).value().to_string();
        let blank = message.trim().is_empty();
        let no_staged = snapshot.staged.is_empty();
        let conflicts = !snapshot.conflicts.is_empty();
        let disabled = self.active_mutation.is_some() || blank || no_staged || conflicts;
        let reason = if conflicts {
            "Resolve merge changes before committing"
        } else if no_staged {
            "Stage at least one file before committing"
        } else if blank {
            "Enter a commit message"
        } else {
            "Create commit"
        };
        v_flex()
            .flex_none()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child("Commit"),
            )
            .child(Textarea::new(&self.commit_message))
            .child(
                Button::new("git-commit")
                    .w_full()
                    .primary()
                    .label(if self.active_mutation.is_some() {
                        "Working…"
                    } else {
                        "Commit"
                    })
                    .tooltip(reason)
                    .disabled(disabled)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let message = this.commit_message.read(cx).value().to_string();
                        this.enqueue_mutation(Mutation::Commit { message }, window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_confirmation(
        &self,
        confirmation: &Confirmation,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, detail, action) = match confirmation {
            Confirmation::Remote { title, detail, .. } => {
                (title.clone(), detail.as_str(), "Confirm")
            }
            Confirmation::Restore { change, .. } => {
                let renamed = change.original_path.is_some();
                (
                    format!("Discard changes to {}?", change.path.display()),
                    if renamed {
                        "The renamed file will move to Trash and its original path will be restored from the index."
                    } else {
                        "The working-tree file will be replaced with the staged index version. This cannot be undone from Devcroft."
                    },
                    "Discard",
                )
            }
            Confirmation::Trash { path, .. } => (
                format!("Move {} to Trash?", path.display()),
                "This untracked file will leave the checkout and can be recovered from macOS Trash.",
                "Move to Trash",
            ),
        };
        h_flex()
            .flex_none()
            .min_h(px(58.))
            .px_3()
            .py_2()
            .gap_3()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(rgb(0x332a16))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_semibold().child(title))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail.to_owned()),
                    ),
            )
            .child(
                Button::new("git-cancel-confirmation")
                    .small()
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.confirmation = None;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("git-confirm-destructive")
                    .small()
                    .danger()
                    .label(action)
                    .disabled(self.active_mutation.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.confirm_pending(window, cx))),
            )
            .into_any_element()
    }

    fn render_diff_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(target) = &self.selection else {
            return div().h(px(0.)).into_any_element();
        };
        let path = target.change.path.clone();
        let label = path.to_string_lossy().into_owned();
        let can_open = self.checkout.join(&path).is_file();
        h_flex()
            .h(px(40.))
            .flex_none()
            .items_center()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .child(label),
            )
            .child(
                Button::new("git-open-selected-file")
                    .small()
                    .ghost()
                    .label("Open in Editor")
                    .disabled(!can_open)
                    .tooltip(if can_open {
                        "Open this working-tree file in the built-in editor"
                    } else {
                        "This path is not a regular working-tree file"
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_file(path.clone(), window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_detail(&self, label: &str, value: String, cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(label.to_owned()),
            )
            .child(div().text_sm().child(value))
            .into_any_element()
    }

    fn render_empty(&self, title: &str, detail: &str, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_6()
            .child(div().text_sm().font_semibold().child(title.to_owned()))
            .child(
                div()
                    .max_w(px(520.))
                    .text_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(detail.to_owned()),
            )
    }
}

impl Focusable for GitDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_merge_editor(window, cx);
        let body = match &self.state {
            LoadState::Loading => self
                .render_empty(
                    "Loading repository…",
                    "Reading Git status for this checkout.",
                    cx,
                )
                .into_any_element(),
            LoadState::Loaded(snapshot) => self.render_loaded(snapshot, cx),
            LoadState::NotRepository => self
                .render_empty(
                    "This folder is not a Git repository",
                    "Open a repository checkout, then open Git again.",
                    cx,
                )
                .into_any_element(),
            LoadState::GitUnavailable => self
                .render_empty(
                    "Git is unavailable",
                    "Install Git or make it available on the application PATH.",
                    cx,
                )
                .into_any_element(),
            LoadState::Failed(message) => self
                .render_empty("Could not load repository", message, cx)
                .into_any_element(),
        };
        v_flex()
            .size_full()
            .min_h_0()
            .track_focus(&self.focus_handle)
            .tab_group()
            .child(self.render_toolbar(cx))
            .when(matches!(self.state, LoadState::Loaded(_)), |view| {
                view.child(self.render_remote_toolbar(cx))
            })
            .when(matches!(&self.state, LoadState::Loaded(snapshot) if snapshot.operation_details.is_some()), |view| view.child(self.render_operation_banner(cx)))
            .when_some(self.confirmation.as_ref(), |view, confirmation| {
                view.child(self.render_confirmation(confirmation, cx))
            })
            .when_some(self.notice.as_ref(), |view, notice| {
                let elapsed = self
                    .active_started
                    .map(|started| format!(" · {}s", started.elapsed().as_secs()))
                    .unwrap_or_default();
                view.child(
                    div()
                        .flex_none()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(if notice.error {
                            rgb(0xf87171).into()
                        } else {
                            cx.theme().muted_foreground
                        })
                        .child(format!("{}{elapsed}", notice.message)),
                )
            })
            .when(self.remote_form.is_some(), |view| {
                view.child(self.render_remote_form(cx))
            })
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}

fn change_paths(change: &Change) -> Vec<PathBuf> {
    let mut paths = vec![change.path.clone()];
    if let Some(original) = &change.original_path
        && original != &change.path
    {
        paths.push(original.clone());
    }
    paths
}

fn discard_supported(change: &Change) -> bool {
    change.original_path.is_none() || trash_available()
}

fn reconcile_selection(
    current: Option<DiffTarget>,
    snapshot: &RepositorySnapshot,
) -> Option<DiffTarget> {
    current
        .and_then(|target| {
            changes(snapshot, target.area)
                .iter()
                .find(|change| change.path == target.change.path)
                .cloned()
                .map(|change| DiffTarget {
                    area: target.area,
                    change,
                })
        })
        .or_else(|| {
            [
                ChangeArea::Conflict,
                ChangeArea::Staged,
                ChangeArea::Unstaged,
                ChangeArea::Untracked,
            ]
            .into_iter()
            .find_map(|area| {
                changes(snapshot, area)
                    .first()
                    .cloned()
                    .map(|change| DiffTarget { area, change })
            })
        })
}

fn changes(snapshot: &RepositorySnapshot, area: ChangeArea) -> &[Change] {
    match area {
        ChangeArea::Conflict => &snapshot.conflicts,
        ChangeArea::Staged => &snapshot.staged,
        ChangeArea::Unstaged => &snapshot.unstaged,
        ChangeArea::Untracked => &snapshot.untracked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::model::ChangeKind;

    fn change(path: &str) -> Change {
        Change {
            path: path.into(),
            original_path: None,
            kind: ChangeKind::Modified,
            index_mode: None,
            index_oid: None,
        }
    }

    #[gpui_kit::test]
    fn mode_buttons_activate_through_dialog_tab_traversal(cx: &mut gpui_kit::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let dialog =
                cx.new(|cx| GitDialog::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            dialog.update(cx, |dialog, cx| {
                dialog.generation += 1;
                dialog.load_in_flight = false;
                dialog.state = LoadState::Loaded(Box::default());
                dialog.focus_handle.focus(window, cx);
            });
            *slot.borrow_mut() = Some(dialog.clone());
            gpui_kit::component::Root::new(dialog, window, cx)
        });
        let dialog = holder.borrow().clone().unwrap();
        test_cx.simulate_keystrokes("tab tab enter");
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("enter").unwrap(),
        });
        assert_eq!(
            dialog.read_with(test_cx, |dialog, _| dialog.mode),
            GitMode::Branches
        );
        test_cx.update(|window, cx| dialog.focus_handle(cx).focus(window, cx));
        test_cx.simulate_keystrokes("tab tab tab enter");
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("enter").unwrap(),
        });
        assert_eq!(
            dialog.read_with(test_cx, |dialog, _| dialog.mode),
            GitMode::History
        );
        test_cx.update(|window, cx| dialog.focus_handle(cx).focus(window, cx));
        test_cx.simulate_keystrokes("tab space");
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("space").unwrap(),
        });
        assert_eq!(
            dialog.read_with(test_cx, |dialog, _| dialog.mode),
            GitMode::Changes
        );
    }

    #[gpui_kit::test]
    fn tab_traversal_leaves_merge_results_unchanged(cx: &mut gpui_kit::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        let directory = crate::git::conflicts::tests::setup();
        crate::git::conflicts::tests::text_conflict(directory.path(), Path::new("file.txt"));
        let snapshot = load_snapshot(directory.path()).unwrap();
        let checkout = directory.path().to_owned();
        cx.update(gpui_kit::init);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let dialog =
                cx.new(|cx| GitDialog::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            dialog.update(cx, |dialog, cx| {
                dialog.generation += 1;
                dialog.load_in_flight = false;
                dialog.selection = reconcile_selection(None, &snapshot);
                dialog.state = LoadState::Loaded(Box::new(snapshot));
                dialog.focus_handle.focus(window, cx);
            });
            *slot.borrow_mut() = Some(dialog.clone());
            gpui_kit::component::Root::new(dialog, window, cx)
        });
        let dialog = holder.borrow().clone().unwrap();
        test_cx.run_until_parked();
        dialog.read_with(test_cx, |dialog, cx| {
            assert_eq!(dialog.merge_editors.len(), 1);
            assert!(
                !dialog
                    .merge_editors
                    .values()
                    .next()
                    .unwrap()
                    .read(cx)
                    .dirty(cx)
            );
        });
        // Traverse through Current, Incoming, Result, and the operation controls
        // several times. No Tab may insert indentation into Result or Commit.
        for _ in 0..60 {
            test_cx.simulate_keystrokes("tab");
        }
        for _ in 0..60 {
            test_cx.simulate_keystrokes("shift-tab");
        }
        dialog.read_with(test_cx, |dialog, cx| {
            assert!(
                !dialog
                    .merge_editors
                    .values()
                    .next()
                    .unwrap()
                    .read(cx)
                    .dirty(cx)
            );
            assert!(dialog.commit_message.read(cx).value().is_empty());
        });
    }

    #[test]
    fn selection_survives_refresh_and_falls_back_in_stable_section_order() {
        let snapshot = RepositorySnapshot {
            staged: vec![change("staged")],
            unstaged: vec![change("worktree")],
            ..RepositorySnapshot::default()
        };
        let current = DiffTarget {
            area: ChangeArea::Unstaged,
            change: change("worktree"),
        };
        assert_eq!(
            reconcile_selection(Some(current.clone()), &snapshot),
            Some(current)
        );

        let fallback = reconcile_selection(
            Some(DiffTarget {
                area: ChangeArea::Untracked,
                change: change("gone"),
            }),
            &snapshot,
        )
        .unwrap();
        assert_eq!(fallback.area, ChangeArea::Staged);
        assert_eq!(fallback.change.path, PathBuf::from("staged"));
    }

    #[test]
    fn selection_keeps_its_path_and_refreshes_index_identity() {
        let mut current_change = change("tracked.txt");
        current_change.index_oid = Some("old".into());
        let mut refreshed_change = current_change.clone();
        refreshed_change.index_oid = Some("new".into());
        let snapshot = RepositorySnapshot {
            unstaged: vec![refreshed_change.clone()],
            ..RepositorySnapshot::default()
        };

        let selected = reconcile_selection(
            Some(DiffTarget {
                area: ChangeArea::Unstaged,
                change: current_change,
            }),
            &snapshot,
        )
        .unwrap();

        assert_eq!(selected.area, ChangeArea::Unstaged);
        assert_eq!(selected.change, refreshed_change);
    }

    #[test]
    fn rename_actions_include_both_repository_paths() {
        let change = Change {
            path: "new name.txt".into(),
            original_path: Some("old name.txt".into()),
            kind: ChangeKind::Renamed,
            index_mode: None,
            index_oid: None,
        };
        assert_eq!(
            change_paths(&change),
            [PathBuf::from("new name.txt"), PathBuf::from("old name.txt")]
        );
    }
}
