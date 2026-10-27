mod commit_ui;
mod conflict_ui;
mod history_ui;
mod remote_ui;

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    time::Duration,
};

use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{Confirm, DialogFooter},
    h_flex,
    input::{Input, InputState, TextareaState},
    popover::Popover,
    scroll::ScrollableElement as _,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, ParentElement, Render, StatefulInteractiveElement as _,
    Styled, Subscription, TestSupportExt as _, Window, div, px, relative, rgb,
};

use crate::{
    diff::kit_viewer::{KitDiffViewer, PreparedKitDiff},
    git_status::GitStatus,
};

use super::{
    diff::{ChangeArea, DiffTarget, load_change_diff},
    model::{Change, RepositorySnapshot},
    operations::{
        Mutation, TargetFingerprint, capture_restore_target, capture_trash_target, execute,
        trash_available,
    },
    remote::Action,
    repository::{LoadError, load_snapshot},
    watch::FileChanges,
};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const MAX_VISIBLE_CHANGES: usize = 250;
const MAX_SWITCHER_BRANCHES: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GitMode {
    Changes,
    History,
}

impl GitMode {
    pub(crate) const ALL: [Self; 2] = [Self::Changes, Self::History];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Changes => "Changes",
            Self::History => "History",
        }
    }
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
pub(crate) enum BranchPickerAction {
    OpenGit,
    Switch(String),
    NewBranch,
}

#[derive(Clone, Debug)]
pub(crate) struct BranchPickerRequested {
    pub(crate) checkout: PathBuf,
    pub(crate) action: BranchPickerAction,
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

pub(crate) struct GitView {
    focus_handle: FocusHandle,
    branch_picker_focus: FocusHandle,
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
    diff_viewer: Entity<KitDiffViewer>,
    commit_message: Entity<TextareaState>,
    commit_dialog_open: bool,
    commit_error: Option<String>,
    mutation_queue: VecDeque<Mutation>,
    active_mutation: Option<Mutation>,
    mutation_generation: u64,
    notice: Option<OperationNotice>,
    confirmation: Option<Confirmation>,
    dirty_editor_paths: HashSet<PathBuf>,
    branch_query: Entity<InputState>,
    branch_switcher_query: Entity<InputState>,
    new_branch_name: Entity<InputState>,
    push_branch: Entity<InputState>,
    selected_remote: Option<String>,
    push_form_open: bool,
    branch_switcher_open: bool,
    active: bool,
    page_visible: bool,
    check_conflicts: bool,
    continuation_loading: bool,
    recovery_paths: HashSet<PathBuf>,
    recovery_warning: Option<String>,
    history: history_ui::HistoryState,
    merge_editors: HashMap<PathBuf, Entity<super::merge_editor::MergeEditor>>,
    data_root: Option<PathBuf>,
    external_editors: Vec<crate::editor::ExternalEditorKind>,
}

impl EventEmitter<SnapshotChanged> for GitView {}
impl EventEmitter<OpenGitFile> for GitView {}
impl EventEmitter<CommitDraftChanged> for GitView {}
impl EventEmitter<BranchPickerRequested> for GitView {}

impl GitView {
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
                .auto_grow(8, 12)
                .placeholder("Describe your changes…")
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
        let branch_switcher_query =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search branches…"));
        let new_branch_name =
            cx.new(|cx| InputState::new(window, cx).placeholder("New local branch name"));
        let push_branch =
            cx.new(|cx| InputState::new(window, cx).placeholder("Remote branch name"));
        for input in [
            &branch_query,
            &branch_switcher_query,
            &new_branch_name,
            &push_branch,
        ] {
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
        cx.observe(&history.viewer, |_, _, cx| cx.notify()).detach();
        let focus_handle = cx.focus_handle();
        let branch_picker_focus = cx.focus_handle();
        let tab_focus = focus_handle.clone();
        let picker_focus = branch_picker_focus.clone();
        let target_window = window.window_handle();
        // Intercept before input bindings resolve: code editors otherwise use
        // Tab for indentation, trapping keyboard users inside Result.
        let tab_interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            let key = &event.keystroke;
            let modifiers = &key.modifiers;
            if window.window_handle() == target_window
                && (tab_focus.contains_focused(window, cx)
                    || picker_focus.contains_focused(window, cx))
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
            branch_picker_focus,
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
            diff_viewer: cx.new(|cx| KitDiffViewer::new(cx).with_external_header()),
            commit_message,
            commit_dialog_open: false,
            commit_error: None,
            mutation_queue: VecDeque::new(),
            active_mutation: None,
            mutation_generation: 0,
            notice: None,
            confirmation: None,
            dirty_editor_paths,
            branch_query,
            branch_switcher_query,
            new_branch_name,
            push_branch,
            selected_remote: None,
            push_form_open: false,
            branch_switcher_open: false,
            active: true,
            page_visible: true,
            check_conflicts: false,
            continuation_loading: false,
            recovery_paths: HashSet::new(),
            recovery_warning: None,
            history,
            merge_editors: HashMap::new(),
            data_root,
            external_editors,
        };
        cx.observe(&dialog.diff_viewer, |_, _, cx| cx.notify())
            .detach();
        dialog.discover_conflict_drafts(cx);
        dialog.reload(false, cx);
        dialog.start_refresh_loop(cx);
        dialog
    }

    pub(crate) fn set_active(&mut self, active: bool, cx: &mut Context<Self>) {
        if self.active == active {
            return;
        }
        self.active = active;
        if active {
            if let LoadState::Loaded(snapshot) = &self.state {
                cx.emit(SnapshotChanged {
                    checkout: self.checkout.clone(),
                    status: snapshot.header_status(),
                });
            }
            self.file_changes.mark_changed();
            self.refresh_if_changed(cx);
        } else {
            self.branch_switcher_open = false;
        }
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn busy(&self) -> bool {
        self.active_mutation.is_some() || !self.mutation_queue.is_empty()
    }

    pub(crate) fn set_page_visible(&mut self, visible: bool) {
        self.page_visible = visible;
    }

    pub(crate) fn mode(&self) -> GitMode {
        self.mode
    }

    pub(crate) fn select_mode(&mut self, mode: GitMode, cx: &mut Context<Self>) {
        self.mode = mode;
        if mode == GitMode::History && self.history.commits.is_empty() {
            self.load_history_page(cx);
        }
        cx.notify();
    }

    pub(crate) fn operation_notice(&self) -> Option<(&str, bool)> {
        self.notice
            .as_ref()
            .map(|notice| (notice.message.as_str(), notice.error))
    }

    pub(crate) fn set_external_editors(
        &mut self,
        editors: Vec<crate::editor::ExternalEditorKind>,
        cx: &mut Context<Self>,
    ) {
        self.external_editors = editors;
        cx.notify();
    }

    pub(crate) fn set_dirty_editor_paths(&mut self, paths: HashSet<PathBuf>) {
        self.dirty_editor_paths = paths;
    }

    fn commit_completed(&mut self, submitted: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_error = None;
        if self.commit_message.read(cx).value().as_ref() == submitted {
            self.commit_message
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.publish_commit_draft(cx);
        if self.commit_dialog_open {
            self.commit_dialog_open = false;
            window.close_dialog(cx);
        }
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
        if !self.active {
            return;
        }
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
                this.complete_change_diff(generation, label, result, cx);
            });
        })
        .detach();
    }

    fn complete_change_diff(
        &mut self,
        generation: u64,
        label: String,
        result: Result<PreparedKitDiff, String>,
        cx: &mut Context<Self>,
    ) {
        if self.diff_generation != generation {
            return;
        }
        self.diff_viewer.update(cx, |viewer, cx| match result {
            Ok(diff) => viewer.set_loaded(diff, cx),
            Err(message) => viewer.set_error(label, message, cx),
        });
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
                            this.push_form_open = false;
                        }
                        Err(message) => {
                            if matches!(mutation, Mutation::Commit { .. }) {
                                this.commit_error = Some(message.clone());
                            }
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

    fn open_file(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        if self.busy() {
            self.notice = Some(OperationNotice {
                message: "Wait for the active Git operation before opening a file in the editor."
                    .into(),
                error: false,
            });
            cx.notify();
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

    fn on_branch_switcher_open_change(
        &mut self,
        open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.branch_switcher_open = open;
        if open {
            self.branch_switcher_query
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.branch_switcher_query
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx);
        }
        cx.notify();
    }

    pub(crate) fn switch_to_local_branch(
        &mut self,
        reference: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.branch_switcher_open = false;
        cx.notify();
        self.request_remote(Action::Switch { reference }, window, cx);
    }

    pub(crate) fn open_new_branch_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let LoadState::Loaded(snapshot) = &self.state else {
            return;
        };
        if self.busy() || snapshot.unborn || snapshot.operation.is_some() {
            return;
        }
        let start_oid = snapshot.oid.clone().unwrap_or_default();
        if start_oid.is_empty() {
            return;
        }
        let start_label = snapshot.branch_label();
        self.branch_switcher_open = false;
        self.new_branch_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        let git = cx.entity().downgrade();
        let name_input = self.new_branch_name.clone();
        cx.notify();
        // Focus the page root first so the create dialog restores focus to a
        // live handle when it closes (the popover search input unmounts with
        // the switcher).
        if self.page_visible {
            self.focus_handle.focus(window, cx);
        } else {
            self.branch_picker_focus.focus(window, cx);
        }
        window.open_dialog(cx, move |dialog, _, cx| {
            let git = git.clone();
            let name_input = name_input.clone();
            let start_oid = start_oid.clone();
            dialog
                .title("Create new branch")
                .w(px(420.))
                .child(
                    v_flex()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("Starting from {start_label}")),
                        )
                        .child(div().text_xs().child("Branch name"))
                        .child(Input::new(&name_input)),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("cancel-new-branch")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-new-branch")
                                .primary()
                                .label("Create and switch")
                                .on_click(|_, window, cx| {
                                    window
                                        .dispatch_action(Box::new(Confirm { secondary: false }), cx)
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name = name_input.read(cx).value().to_string();
                    let name = name.trim().to_owned();
                    if name.is_empty() {
                        window.push_notification("Enter a branch name", cx);
                        return false;
                    }
                    let _ = git.update(cx, |this, cx| {
                        this.request_remote(
                            Action::Create {
                                name,
                                start: "HEAD".into(),
                                start_oid: start_oid.clone(),
                                track: false,
                            },
                            window,
                            cx,
                        );
                    });
                    true
                })
        });
        self.new_branch_name
            .read(cx)
            .focus_handle(cx)
            .focus(window, cx);
    }

    fn request_picker_action(
        &mut self,
        action: BranchPickerAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.branch_switcher_open = false;
        self.branch_picker_focus.focus(window, cx);
        cx.emit(BranchPickerRequested {
            checkout: self.checkout.clone(),
            action,
        });
        cx.notify();
    }

    pub(crate) fn render_branch_picker(
        &self,
        status: &GitStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self.branch_switcher_query.read(cx).value().to_lowercase();
        let busy = self.busy();
        let (switchable, can_create) = match &self.state {
            LoadState::Loaded(snapshot) => (
                !busy && snapshot.operation.is_none(),
                !busy && !snapshot.unborn && snapshot.operation.is_none(),
            ),
            _ => (false, false),
        };
        let (branches, branches_truncated): (Vec<(String, String, String, bool)>, bool) =
            match &self.state {
                LoadState::Loaded(snapshot) => switcher_branches(snapshot, &query),
                _ => (Vec::new(), false),
            };
        let empty_label = if matches!(self.state, LoadState::Loading) {
            "Loading branches…"
        } else if query.is_empty() {
            "No branches yet"
        } else {
            "No matching branches"
        };
        let muted = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let git = cx.entity().downgrade();
        let search_input = self.branch_switcher_query.clone();
        let summary = self.summary();
        let upstream = match &self.state {
            LoadState::Loaded(snapshot) => snapshot
                .upstream
                .as_ref()
                .map(|name| format!("Tracking {name}"))
                .unwrap_or_else(|| "No upstream configured".into()),
            _ => String::new(),
        };
        let checkout_detail = format!(
            "Switch branch · {}\n{}\n{upstream}",
            summary,
            self.checkout.display()
        );
        div().track_focus(&self.branch_picker_focus).min_w_0().child(
                    Popover::new("git-branch-switcher")
                        .open(self.branch_switcher_open)
                        .track_focus(&self.branch_switcher_query.read(cx).focus_handle(cx))
                        .on_open_change(cx.listener(|this, open, window, cx| {
                            this.on_branch_switcher_open_change(*open, window, cx);
                        }))
                        .trigger(
                            Button::new("git-branch-switcher")
                                .small()
                                .ghost()
                                .icon(if status.detached { gpui_kit::assets::IconName::GitCommitHorizontal } else { gpui_kit::assets::IconName::GitBranch })
                                .label(status.branch.clone().unwrap_or_else(|| "Git".into()))
                                .accessibility_label("Switch branch")
                                .h(px(28.))
                                .when(status.dirty, |button| button.child(div().text_xs().text_color(rgb(0xeab308)).child("●")))
                                .when_some(status.ahead_label(), |button, ahead| button.child(div().text_xs().text_color(muted).child(ahead)))
                                .when_some(status.behind_label(), |button, behind| button.child(div().text_xs().text_color(muted).child(behind)))
                                .dropdown_caret(true)
                                .tooltip(checkout_detail)
                                .max_w(px(260.)),
                        )
                        .content(move |_, _, _| {
                            let git_open = git.clone();
                            let git_list = git.clone();
                            let git_new = git.clone();
                            let search_input = search_input.clone();
                            v_flex()
                                .min_w(px(300.))
                                .gap_1()
                                .child(Button::new("git-picker-open-page").small().ghost().w_full().label("Open Git").on_click(move |_, window, cx| {
                                    let _ = git_open.update(cx, |this, cx| this.request_picker_action(BranchPickerAction::OpenGit, window, cx));
                                }))
                                .child(div().text_xs().text_color(muted).child("Local branches"))
                                .child(div().child(Input::new(&search_input).small()))
                                .child(
                                    div().max_h(px(300.)).overflow_y_scrollbar().child(
                                        v_flex().gap_1()
                                            .when(branches.is_empty(), |list| {
                                                list.child(
                                                    div()
                                                        .px_2()
                                                        .py_2()
                                                        .text_xs()
                                                        .text_color(muted)
                                                        .child(empty_label),
                                                )
                                            })
                                            .children(branches.iter().enumerate().map(
                                                |(
                                                    index,
                                                    (reference, name, subject, current),
                                                )| {
                                                    let reference = reference.clone();
                                                    let git = git_list.clone();
                                                    Button::new((
                                                        "git-branch-switcher-item",
                                                        index,
                                                    ))
                                                    .ghost()
                                                    .w_full()
                                                    .child(
                                                        h_flex()
                                                            .w_full()
                                                            .min_w_0()
                                                            .gap_2()
                                                            .items_center()
                                                            .child(
                                                                Icon::new(
                                                                    gpui_kit::assets::IconName::GitBranch,
                                                                )
                                                                .size(px(14.))
                                                                .text_color(muted),
                                                            )
                                                            .child(
                                                                v_flex()
                                                                    .flex_1()
                                                                    .min_w_0()
                                                                    .child(
                                                                        div()
                                                                            .truncate()
                                                                            .text_sm()
                                                                            .child(name.clone()),
                                                                    )
                                                                    .when(!subject.is_empty(), |row| {
                                                                        row.child(
                                                                            div()
                                                                                .truncate()
                                                                                .text_xs()
                                                                                .text_color(muted)
                                                                                .child(subject.clone()),
                                                                        )
                                                                    }),
                                                            )
                                                            .when(*current, |row| {
                                                                row.child(
                                                                    Icon::new(IconName::Check)
                                                                        .size(px(14.))
                                                                        .text_color(success),
                                                                )
                                                            }),
                                                    )
                                                    .selected(*current)
                                                    .disabled(!switchable || *current)
                                                    .tooltip(name.clone())
                                                    .accessibility_label(name.clone())
                                                    .on_click(move |_, window, cx| {
                                                        let _ = git.update(cx, |this, cx| {
                                                            this.request_picker_action(
                                                                BranchPickerAction::Switch(reference.clone()),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    })
                                                },
                                            )),
                                    ),
                                )
                                .when(branches_truncated, |view| {
                                    view.child(
                                        div()
                                            .px_2()
                                            .text_xs()
                                            .text_color(muted)
                                            .child(format!(
                                                "Showing first {MAX_SWITCHER_BRANCHES}. Type to narrow the search."
                                            )),
                                    )
                                })
                                .child(
                                    Button::new("git-branch-switcher-new")
                                        .small()
                                        .w_full()
                                        .icon(IconName::Plus)
                                        .label("New branch")
                                        .disabled(!can_create)
                                        .tooltip(if can_create {
                                            "Create and switch to a new branch"
                                        } else {
                                            "Create a first commit and finish any active Git operation before creating a branch"
                                        })
                                        .on_click(move |_, window, cx| {
                                            let _ = git_new.update(cx, |this, cx| {
                                                this.request_picker_action(BranchPickerAction::NewBranch, window, cx);
                                            });
                                        }),
                                )
                        }),        ).into_any_element()
    }

    fn render_loaded(&self, snapshot: &RepositorySnapshot, cx: &mut Context<Self>) -> AnyElement {
        match self.mode {
            GitMode::Changes => self.render_changes(snapshot, cx),
            GitMode::History => self.render_history(snapshot, cx),
        }
    }

    fn render_changes(&self, snapshot: &RepositorySnapshot, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.active_mutation.is_some();
        h_flex().size_full().min_h_0().min_w_0()
            .child(
                v_flex().w(px(340.)).max_w(relative(0.4)).h_full().min_h_0().flex_none()
                    .bg(cx.theme().sidebar).border_r_1().border_color(cx.theme().border)
                    .child(
                        div().flex_1().min_h_0().overflow_y_scrollbar().child(
                            v_flex().w_full().px_2().pt_3().pb_3().gap_3()
                                .when(!snapshot.conflicts.is_empty(), |view| view.child(self.render_change_section("Merge changes", ChangeArea::Conflict, &snapshot.conflicts, busy, cx)))
                                .child(self.render_orphaned_results(snapshot, cx))
                                .child(self.render_change_section("Staged", ChangeArea::Staged, &snapshot.staged, busy, cx))
                                .child(self.render_change_section("Unstaged", ChangeArea::Unstaged, &snapshot.unstaged, busy, cx))
                                .when(!snapshot.untracked.is_empty(), |view| view.child(self.render_change_section("Untracked", ChangeArea::Untracked, &snapshot.untracked, busy, cx)))
                        )
                    )
            )
            .child(
                v_flex().flex_1().min_w_0().h_full().min_h_0()
                    .child(self.render_diff_header(cx))
                    .child(div().flex_1().min_h_0().min_w_0()
                        .child(if self.selection.is_some() {
                            self.selected_change_view()
                        } else {
                            self.render_empty("Working tree clean", "New changes will appear here automatically. Explore your commits in History.", cx).into_any_element()
                        }))
            ).into_any_element()
    }

    fn render_count(&self, count: usize, cx: &Context<Self>) -> impl IntoElement {
        div()
            .min_w(px(22.))
            .px_1p5()
            .py(px(2.))
            .rounded_md()
            .bg(cx.theme().muted)
            .text_color(cx.theme().muted_foreground)
            .text_xs()
            .text_center()
            .child(count.to_string())
    }

    fn render_change_section(
        &self,
        title: &str,
        area: ChangeArea,
        changes: &[Change],
        busy: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible = changes.len().min(self.visible_changes[area as usize]);
        let (has_conflicts, has_untracked) = match &self.state {
            LoadState::Loaded(snapshot) => (
                !snapshot.conflicts.is_empty(),
                !snapshot.untracked.is_empty(),
            ),
            _ => (false, false),
        };
        let color = match area {
            ChangeArea::Conflict => cx.theme().danger,
            ChangeArea::Staged => cx.theme().success,
            ChangeArea::Unstaged => cx.theme().warning,
            ChangeArea::Untracked => cx.theme().ring,
        };
        v_flex()
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .h(px(28.))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(div().size(px(6.)).rounded_full().bg(color))
                    .child(div().flex_1().child(title.to_owned()))
                    .when(area == ChangeArea::Staged, |header| {
                        header.child(
                            Button::new("git-unstage-all")
                                .xsmall().ghost().h(px(24.)).px_1()
                                .label("Unstage all")
                                .disabled(busy || has_conflicts || changes.is_empty())
                                .tooltip(if has_conflicts { "Resolve merge changes before unstaging all files" } else { "Unstage every staged file" })
                                .on_click(cx.listener(|this, _, window, cx| {
                                    let unborn = matches!(&this.state, LoadState::Loaded(snapshot) if snapshot.unborn);
                                    this.enqueue_mutation(Mutation::UnstageAll { unborn }, window, cx)
                                }))
                        )
                    })
                    .when(area == ChangeArea::Unstaged, |header| {
                        header.child(
                            Button::new("git-stage-all")
                                .xsmall().ghost().h(px(24.)).px_1()
                                .label("Stage all")
                                .disabled(busy || has_conflicts || (changes.is_empty() && !has_untracked))
                                .tooltip(if has_conflicts { "Resolve merge changes before staging all files" } else { "Stage all unstaged and untracked files" })
                                .on_click(cx.listener(|this, _, window, cx| this.enqueue_mutation(Mutation::StageAll, window, cx)))
                        )
                    })
                    .child(self.render_count(changes.len(), cx)),
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
                        let name = change
                            .path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();
                        let parent = change
                            .original_path
                            .as_ref()
                            .map(|original| format!("← {}", original.display()))
                            .unwrap_or_else(|| {
                                change
                                    .path
                                    .parent()
                                    .unwrap_or_else(|| Path::new(""))
                                    .to_string_lossy()
                                    .into_owned()
                            });
                        let accessible = if parent.is_empty() {
                            path.clone()
                        } else {
                            format!("{path} · {parent}")
                        };
                        let mut row = h_flex()
                            .id(("git-change-row", index * 4 + area as usize))
                            .w_full()
                            .min_w_0()
                            .h(px(48.))
                            .pr_1()
                            .gap_1()
                            .rounded_md()
                            .when(selected, |row| row.bg(cx.theme().accent))
                            .child(
                                Button::new(("git-select-file", index * 4 + area as usize))
                                    .ghost()
                                    .flex_1()
                                    .min_w_0()
                                    .h_full()
                                    .px_2()
                                    .accessibility_label(accessible.clone())
                                    .selected(selected)
                                    .tooltip(accessible)
                                    .child(
                                        h_flex()
                                            .w_full()
                                            .min_w_0()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(20.))
                                                    .flex_none()
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_center()
                                                    .text_color(color)
                                                    .child(change.kind.label()),
                                            )
                                            .child(
                                                v_flex()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .gap(px(2.))
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .truncate()
                                                            .text_sm()
                                                            .child(name),
                                                    )
                                                    .when(!parent.is_empty(), |view| {
                                                        view.child(
                                                            div()
                                                                .w_full()
                                                                .truncate()
                                                                .text_xs()
                                                                .text_color(
                                                                    cx.theme().muted_foreground,
                                                                )
                                                                .child(parent),
                                                        )
                                                    }),
                                            ),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_change(select_target.clone(), cx)
                                    })),
                            );
                        row = match area {
                            ChangeArea::Staged => {
                                let action = change.clone();
                                row.child(
                                    Button::new(("git-unstage-file", index))
                                        .xsmall()
                                        .ghost()
                                        .icon(IconName::Minus)
                                        .tooltip("Unstage file")
                                        .accessibility_label(format!("Unstage {path}"))
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
                                        .icon(IconName::Plus)
                                        .tooltip("Stage file")
                                        .accessibility_label(format!("Stage {path}"))
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
                                                .icon(IconName::Undo2)
                                                .tooltip("Discard changes…")
                                                .accessibility_label(format!(
                                                    "Discard changes in {path}"
                                                ))
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
                                        .icon(IconName::Plus)
                                        .tooltip("Stage file")
                                        .accessibility_label(format!("Stage {path}"))
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
                                            .icon(gpui_kit::assets::IconName::Trash)
                                            .tooltip("Move to Trash…")
                                            .accessibility_label(format!("Move {path} to Trash"))
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
                        .text_color(cx.theme().muted_foreground)
                        .child(if area == ChangeArea::Staged {
                            "Stage files to include them in a commit."
                        } else {
                            "No unstaged changes."
                        }),
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
            return div().into_any_element();
        };
        let path = target.change.path.clone();
        let label = match &target.change.original_path {
            Some(original) if original != &path => {
                format!("{} → {}", original.display(), path.display())
            }
            _ => path.to_string_lossy().into_owned(),
        };
        let comparison = match target.area {
            ChangeArea::Staged => "Staged · HEAD → index",
            ChangeArea::Unstaged => "Unstaged · index → working tree",
            ChangeArea::Untracked => "Untracked · new file",
            ChangeArea::Conflict => "Merge conflict",
        };
        let detail = format!("{label}\n{comparison}");
        let busy = self.busy();
        let can_open = self.checkout.join(&path).is_file();
        h_flex()
            .id("git-file-header")
            .test_support()
            .h(px(40.))
            .min_w_0()
            .flex_none()
            .gap_2()
            .px_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .id("git-diff-path")
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_sm()
                    .font_semibold()
                    .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx))
                    .child(label),
            )
            .when(target.area != ChangeArea::Conflict, |header| {
                header.child(
                    self.diff_viewer
                        .update(cx, |viewer, cx| viewer.render_header_controls(cx)),
                )
            })
            .child(
                Button::new("git-open-selected-file")
                    .small()
                    .ghost()
                    .icon(IconName::ExternalLink)
                    .accessibility_label("Open in editor")
                    .disabled(busy || !can_open)
                    .tooltip(if busy {
                        "Wait for the active Git operation"
                    } else if can_open {
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
            .gap_3()
            .p_6()
            .child(
                h_flex()
                    .size(px(52.))
                    .justify_center()
                    .rounded_lg()
                    .bg(cx.theme().muted)
                    .child(
                        Icon::new(gpui_kit::assets::IconName::GitBranch)
                            .size(px(24.))
                            .text_color(cx.theme().muted_foreground),
                    ),
            )
            .child(div().text_base().font_semibold().child(title.to_owned()))
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

impl Focusable for GitView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitView {
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
            .bg(cx.theme().background)
            .when(matches!(&self.state, LoadState::Loaded(snapshot) if snapshot.operation_details.is_some()), |view| view.child(self.render_operation_banner(cx)))
            .when_some(self.confirmation.as_ref(), |view, confirmation| {
                view.child(self.render_confirmation(confirmation, cx))
            })
            .when(self.push_form_open, |view| {
                view.child(self.render_remote_form(cx))
            })
            .child(div().flex_1().min_h_0().w_full().child(body))
    }
}

fn switcher_branches(
    snapshot: &RepositorySnapshot,
    query: &str,
) -> (Vec<(String, String, String, bool)>, bool) {
    let query = query.to_lowercase();
    let mut matching = snapshot
        .branches
        .iter()
        .filter(|branch| !branch.remote)
        .filter(|branch| {
            query.is_empty()
                || format!("{} {}", branch.name, branch.subject)
                    .to_lowercase()
                    .contains(&query)
        })
        .collect::<Vec<_>>();
    // Preserve the repository's alphabetical ordering within each group.
    matching.sort_by_key(|branch| !branch.current);
    let truncated = matching.len() > MAX_SWITCHER_BRANCHES;
    let branches = matching
        .into_iter()
        .take(MAX_SWITCHER_BRANCHES)
        .map(|branch| {
            (
                branch.reference.clone(),
                branch.name.clone(),
                branch.subject.clone(),
                branch.current,
            )
        })
        .collect();
    (branches, truncated)
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

    fn switcher_branch(name: &str, remote: bool, current: bool) -> super::super::branches::Branch {
        super::super::branches::Branch {
            reference: if remote {
                format!("refs/remotes/origin/{name}")
            } else {
                format!("refs/heads/{name}")
            },
            name: if remote {
                format!("origin/{name}")
            } else {
                name.into()
            },
            remote,
            current,
            oid: "abc123".into(),
            subject: format!("{name} subject"),
            upstream: String::new(),
            tracking: String::new(),
            push_remote: String::new(),
            push_ref: String::new(),
        }
    }

    #[test]
    fn switcher_lists_local_branches_and_filters_by_query() {
        let snapshot = RepositorySnapshot {
            branches: vec![
                switcher_branch("main", false, true),
                switcher_branch("feature", false, false),
                switcher_branch("main", true, false),
            ],
            ..RepositorySnapshot::default()
        };
        let (all, truncated) = switcher_branches(&snapshot, "");
        assert!(!truncated);
        assert_eq!(all.len(), 2);
        assert!(
            all.iter()
                .all(|(reference, _, _, _)| reference.starts_with("refs/heads/"))
        );
        assert!(
            all.iter()
                .any(|(_, name, _, current)| name == "main" && *current)
        );

        let (filtered, _) = switcher_branches(&snapshot, "FEAT");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, "feature");

        let (subject_match, _) = switcher_branches(&snapshot, "main subject");
        assert_eq!(subject_match.len(), 1);
        assert_eq!(subject_match[0].1, "main");

        let (none, _) = switcher_branches(&snapshot, "zzz-no-such-branch");
        assert!(none.is_empty());
    }

    #[test]
    fn switcher_reports_truncation_beyond_the_display_limit() {
        let branches = (0..=MAX_SWITCHER_BRANCHES)
            .map(|index| switcher_branch(&format!("branch-{index:03}"), false, index == 0))
            .collect::<Vec<_>>();
        let snapshot = RepositorySnapshot {
            branches,
            ..RepositorySnapshot::default()
        };
        let (listed, truncated) = switcher_branches(&snapshot, "");
        assert_eq!(listed.len(), MAX_SWITCHER_BRANCHES);
        assert!(truncated);

        let exact = RepositorySnapshot {
            branches: (0..MAX_SWITCHER_BRANCHES)
                .map(|index| switcher_branch(&format!("branch-{index:03}"), false, false))
                .collect(),
            ..RepositorySnapshot::default()
        };
        let (listed, truncated) = switcher_branches(&exact, "");
        assert_eq!(listed.len(), MAX_SWITCHER_BRANCHES);
        assert!(!truncated);
    }

    #[test]
    fn switcher_pins_current_branch_before_the_limit_without_bypassing_search() {
        let snapshot = RepositorySnapshot {
            branches: (0..=MAX_SWITCHER_BRANCHES)
                .map(|index| {
                    switcher_branch(
                        &format!("branch-{index:03}"),
                        false,
                        index == MAX_SWITCHER_BRANCHES,
                    )
                })
                .collect(),
            ..RepositorySnapshot::default()
        };
        let (listed, truncated) = switcher_branches(&snapshot, "");
        assert!(truncated);
        assert_eq!(listed.len(), MAX_SWITCHER_BRANCHES);
        assert!(listed[0].3);
        assert_eq!(listed[0].1, "branch-100");
        let (filtered, _) = switcher_branches(&snapshot, "branch-000");
        assert_eq!(filtered.len(), 1);
        assert!(!filtered[0].3);
    }

    #[gpui_kit::test]
    fn branch_modal_cancel_preserves_push_draft_and_page(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let directory = crate::git::conflicts::tests::setup();
        let snapshot = load_snapshot(directory.path()).unwrap();
        let checkout = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view =
                cx.new(|cx| GitView::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            view.update(cx, |view, cx| {
                view.generation += 1;
                view.load_in_flight = false;
                view.state = LoadState::Loaded(Box::new(snapshot));
                view.mode = GitMode::History;
                view.push_form_open = true;
                view.push_branch
                    .update(cx, |input, cx| input.set_value("push-draft", window, cx));
            });
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            assert!(!window.has_active_dialog(cx));
            view.update(cx, |view, cx| view.open_new_branch_dialog(window, cx));
            window.render_frame(cx);
            assert!(window.has_active_dialog(cx));
            window.click("cancel-new-branch", cx);
            window.render_frame(cx);
            assert!(!window.has_active_dialog(cx));
            assert!(view.focus_handle(cx).contains_focused(window, cx));
        });
        view.read_with(test_cx, |view, cx| {
            assert_eq!(view.push_branch.read(cx).value().as_ref(), "push-draft");
            assert!(view.push_form_open);
            assert_eq!(view.mode, GitMode::History);
        });
    }

    #[gpui_kit::test]
    fn changes_diff_rejects_stale_results_and_refreshes_source_without_editing_draft(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let git = |args: &[&str]| {
            let result = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Test"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("tracked.txt"), "before\r\ncontext\r\n").unwrap();
        git(&["add", "tracked.txt"]);
        git(&["commit", "-m", "initial"]);
        std::fs::write(root.join("tracked.txt"), "staged\r\ncontext\r\n").unwrap();
        git(&["add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "worktree\r\ncontext\r\n").unwrap();
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        let snapshot = load_snapshot(root).unwrap();
        let selected = DiffTarget {
            area: ChangeArea::Unstaged,
            change: snapshot.unstaged[0].clone(),
        };
        let obsolete = DiffTarget {
            area: ChangeArea::Untracked,
            change: snapshot.untracked[0].clone(),
        };
        let checkout = root.to_owned();
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let dialog =
                cx.new(|cx| GitView::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            dialog.update(cx, |dialog, cx| {
                dialog.generation += 1;
                dialog.load_in_flight = false;
                dialog.state = LoadState::Loaded(Box::new(snapshot));
                dialog.restore_commit_message("keep this draft", window, cx);
            });
            *slot.borrow_mut() = Some(dialog.clone());
            gpui_kit::component::Root::new(dialog, window, cx)
        });
        let dialog = holder.borrow().clone().unwrap();
        let obsolete_generation = dialog.update(test_cx, |dialog, cx| {
            dialog.select_change(obsolete.clone(), cx);
            let generation = dialog.diff_generation;
            dialog.select_change(selected.clone(), cx);
            assert!(dialog.diff_generation > generation);
            generation
        });
        test_cx.run_until_parked();
        let stale = load_change_diff(root, &obsolete).unwrap();
        dialog.update(test_cx, |dialog, cx| {
            dialog.complete_change_diff(obsolete_generation, obsolete.label(), Ok(stale), cx);
            dialog.complete_change_diff(
                obsolete_generation,
                obsolete.label(),
                Err("obsolete error".into()),
                cx,
            );
        });
        test_cx.simulate_resize(gpui_kit::size(px(700.), px(600.)));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            let header = window.find("git-file-header").bounds();
            assert_eq!(header.size.height, px(40.));
            for control in [
                "git-diff-unified",
                "git-diff-split",
                "git-diff-previous",
                "git-diff-next",
                "git-open-selected-file",
            ] {
                let bounds = window.find(control).bounds();
                assert!(
                    bounds.left() >= header.left() && bounds.right() <= header.right(),
                    "{control} must stay inside the compact header"
                );
                assert!(
                    bounds.top() >= header.top() && bounds.bottom() <= header.bottom(),
                    "{control} must stay on the single header row"
                );
            }
        });
        test_cx.simulate_resize(gpui_kit::size(px(1024.), px(768.)));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find("diff-body")
                    .label()
                    .unwrap()
                    .contains("tracked.txt")
            );
            window.click("git-diff-split", cx);
            window.press("home", cx);
            window.press("shift-down", cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-c"
                } else {
                    "ctrl-c"
                },
                cx,
            );
        });
        assert_eq!(
            test_cx
                .read(|cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some("worktree\r\n")
        );
        dialog.read_with(test_cx, |dialog, cx| {
            assert_eq!(dialog.selection.as_ref(), Some(&selected));
            assert_eq!(
                dialog.commit_message.read(cx).value().as_ref(),
                "keep this draft"
            );
        });

        // Replacing the index changes the old side as well as the status identity.
        // The selected area/path and display settings should survive the reload.
        std::fs::write(root.join("tracked.txt"), "new index\r\ncontext\r\n").unwrap();
        git(&["add", "tracked.txt"]);
        std::fs::write(
            root.join("tracked.txt"),
            "refreshed worktree\r\ncontext\r\n",
        )
        .unwrap();
        let before_refresh = dialog.update(test_cx, |dialog, cx| {
            let generation = dialog.diff_generation;
            dialog.reload(true, cx);
            generation
        });
        test_cx.run_until_parked();
        let stale = load_change_diff(root, &obsolete).unwrap();
        dialog.update(test_cx, |dialog, cx| {
            assert!(dialog.diff_generation > before_refresh);
            assert_eq!(
                dialog.selection.as_ref().unwrap().area,
                ChangeArea::Unstaged
            );
            assert_eq!(
                dialog.selection.as_ref().unwrap().change.path,
                Path::new("tracked.txt")
            );
            assert_ne!(
                dialog.selection.as_ref().unwrap().change.index_oid,
                selected.change.index_oid
            );
            dialog.complete_change_diff(before_refresh, obsolete.label(), Ok(stale), cx);
        });
        dialog.read_with(test_cx, |dialog, cx| {
            assert_eq!(
                dialog.diff_viewer.read(cx).display_settings(cx),
                (gpui_kit::component::diff::DiffMode::Split, true)
            );
        });
        test_cx.update(|window, cx| {
            window.click("git-diff-next", cx);
            window.press("home", cx);
            window.press("shift-down", cx);
            window.press(
                if cfg!(target_os = "macos") {
                    "cmd-c"
                } else {
                    "ctrl-c"
                },
                cx,
            );
        });
        assert_eq!(
            test_cx
                .read(|cx| cx.read_from_clipboard())
                .and_then(|item| item.text())
                .as_deref(),
            Some("refreshed worktree\r\n")
        );
        dialog.read_with(test_cx, |dialog, cx| {
            assert_eq!(
                dialog.commit_message.read(cx).value().as_ref(),
                "keep this draft"
            )
        });

        // A failed repository refresh must also invalidate any outstanding diff.
        let stale = load_change_diff(root, &obsolete).unwrap();
        dialog.update(test_cx, |dialog, cx| {
            let generation = dialog.diff_generation;
            dialog.fail_load(LoadState::Failed("repository error".into()), cx);
            dialog.complete_change_diff(generation, obsolete.label(), Ok(stale), cx);
            assert!(dialog.selection.is_none());
            dialog.state = LoadState::Loaded(Box::default());
            cx.notify();
        });
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("diff-body").is_none());
        });
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
                cx.new(|cx| GitView::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
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
