use super::*;
use crate::diff::kit_viewer::{KitDiffViewer, PreparedKitDiff};
use crate::git::{
    history::{self, Commit, HistoryFile},
    remote::{Action, Source},
};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::{
    ClipboardItem, KeyDownEvent, ListAlignment, ListState, StatefulInteractiveElement as _, list,
    uniform_list,
};
use std::cell::RefCell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HistoryRow {
    Commit(usize),
    FilesSummary,
    File(usize),
    FilesError,
}

pub(super) struct HistoryState {
    pub query: Entity<InputState>,
    pub branch_name: Entity<InputState>,
    pub commits: Vec<Commit>,
    focus: FocusHandle,
    scroll: ListState,
    rows: RefCell<Vec<HistoryRow>>,
    cursor: Option<HistoryRow>,
    expanded: bool,
    scope: Option<String>,
    tip: Option<String>,
    lanes: Vec<String>,
    more: bool,
    loading: bool,
    error: Option<String>,
    generation: u64,
    cancel: Arc<AtomicBool>,
    detail_generation: u64,
    detail_cancel: Arc<AtomicBool>,
    selected: Option<usize>,
    files: Vec<HistoryFile>,
    files_loading: bool,
    files_error: Option<String>,
    selected_file: Option<usize>,
    diff_generation: u64,
    diff_cancel: Arc<AtomicBool>,
    pub(super) viewer: Entity<KitDiffViewer>,
    choose_scope: bool,
    create_branch: bool,
    show_metadata: bool,
}
impl HistoryState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<GitView>) -> Self {
        Self {
            query: cx.new(|cx| InputState::new(window, cx).placeholder("Filter loaded commits…")),
            branch_name: cx.new(|cx| InputState::new(window, cx).placeholder("New branch name")),
            commits: vec![],
            focus: cx.focus_handle().tab_stop(true),
            scroll: ListState::new(0, ListAlignment::Top, px(300.)),
            rows: RefCell::new(vec![]),
            cursor: None,
            expanded: false,
            scope: None,
            tip: None,
            lanes: vec![],
            more: true,
            loading: false,
            error: None,
            generation: 0,
            cancel: Arc::new(AtomicBool::new(false)),
            detail_generation: 0,
            detail_cancel: Arc::new(AtomicBool::new(false)),
            selected: None,
            files: vec![],
            files_loading: false,
            files_error: None,
            selected_file: None,
            diff_generation: 0,
            diff_cancel: Arc::new(AtomicBool::new(false)),
            viewer: cx.new(|cx| KitDiffViewer::new(cx).with_external_header()),
            choose_scope: false,
            create_branch: false,
            show_metadata: false,
        }
    }
    fn cancel_page(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        self.generation += 1;
        self.loading = false;
    }
    fn cancel_detail(&mut self) {
        self.detail_cancel.store(true, Ordering::Relaxed);
        self.detail_cancel = Arc::new(AtomicBool::new(false));
        self.detail_generation += 1;
        self.cancel_diff();
    }
    fn cancel_diff(&mut self) {
        self.diff_cancel.store(true, Ordering::Relaxed);
        self.diff_cancel = Arc::new(AtomicBool::new(false));
        self.diff_generation += 1;
    }
}
impl Drop for HistoryState {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.detail_cancel.store(true, Ordering::Relaxed);
        self.diff_cancel.store(true, Ordering::Relaxed);
    }
}

impl GitView {
    pub(super) fn reset_history(&mut self, cx: &mut Context<Self>) {
        self.history.cancel_page();
        self.history.cancel_detail();
        self.confirmation = None;
        self.history.commits.clear();
        self.history.lanes.clear();
        self.history.selected = None;
        self.history.expanded = false;
        self.history.cursor = None;
        self.history.rows.borrow_mut().clear();
        self.history.scroll.reset(0);
        self.history.files.clear();
        self.history.selected_file = None;
        self.history.files_loading = false;
        self.history.files_error = None;
        self.history.error = None;
        self.history.more = true;
        self.history.create_branch = false;
        self.history
            .viewer
            .update(cx, |viewer, cx| viewer.clear(cx));
        self.history.tip = match &self.state {
            LoadState::Loaded(snapshot) => {
                if let Some(reference) = &self.history.scope {
                    if let Some(branch) =
                        snapshot.branches.iter().find(|b| &b.reference == reference)
                    {
                        Some(branch.oid.clone())
                    } else {
                        self.history.scope = None;
                        snapshot.oid.clone()
                    }
                } else {
                    snapshot.oid.clone()
                }
            }
            _ => None,
        };
        if self.mode == GitMode::History {
            self.load_history_page(cx);
        }
    }

    pub(super) fn history_filter_changed(&mut self, cx: &mut Context<Self>) {
        // Keep loaded results and resume the obsolete page under the new query.
        let resume = self.history.loading;
        self.history.cancel_page();
        if resume {
            self.load_history_page(cx);
        }
        cx.notify();
    }

    pub(super) fn load_history_page(&mut self, cx: &mut Context<Self>) {
        if self.history.loading || !self.history.more {
            return;
        }
        let Some(tip) = self.history.tip.clone() else {
            return;
        };
        self.history.loading = true;
        self.history.error = None;
        let generation = self.history.generation;
        let cancel = self.history.cancel.clone();
        let skip = self.history.commits.len();
        let lanes = self.history.lanes.clone();
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    history::load_page(&checkout, &tip, skip, lanes, &cancel)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.history.generation != generation {
                    return;
                }
                this.history.loading = false;
                match result {
                    Ok(page) => {
                        this.history.commits.extend(page.commits);
                        this.history.lanes = page.lanes;
                        this.history.more = page.more;
                    }
                    Err(error) => this.history.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(commit) = self.history.commits.get(index).cloned() else {
            return;
        };
        self.history.cancel_detail();
        self.confirmation = None;
        self.history.selected = Some(index);
        self.history.expanded = true;
        self.history.cursor = Some(HistoryRow::Commit(index));
        self.history.files.clear();
        self.history.selected_file = None;
        self.history.files_loading = true;
        self.history.files_error = None;
        self.history.create_branch = false;
        self.history
            .viewer
            .update(cx, |viewer, cx| viewer.clear(cx));
        let generation = self.history.detail_generation;
        let cancel = self.history.detail_cancel.clone();
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { history::load_files(&checkout, &commit, &cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.history.detail_generation != generation {
                    return;
                }
                this.history.files_loading = false;
                match result {
                    Ok(files) => {
                        this.history.files = files;
                        if !this.history.files.is_empty() {
                            this.select_history_file(0, cx);
                        }
                    }
                    Err(error) => this.history.files_error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn select_history_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(file) = self.history.files.get(index).cloned() else {
            return;
        };
        self.history.selected_file = Some(index);
        self.history.cancel_diff();
        let generation = self.history.diff_generation;
        let cancel = self.history.diff_cancel.clone();
        let label = file.change.path.to_string_lossy().into_owned();
        self.history
            .viewer
            .update(cx, |viewer, cx| viewer.set_loading(label.clone(), cx));
        let checkout = self.checkout.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    crate::git::diff::load_history_diff(&checkout, &file, &cancel)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.complete_history_diff(generation, label, result, cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn complete_history_diff(
        &mut self,
        generation: u64,
        label: String,
        result: Result<PreparedKitDiff, String>,
        cx: &mut Context<Self>,
    ) {
        if self.history.diff_generation != generation {
            return;
        }
        self.history.viewer.update(cx, |viewer, cx| match result {
            Ok(diff) => viewer.set_loaded(diff, cx),
            Err(error) => viewer.set_error(label, error, cx),
        });
    }

    fn request_detach(&mut self, oid: String, cx: &mut Context<Self>) {
        let LoadState::Loaded(snapshot) = &self.state else {
            return;
        };
        self.confirmation = Some(Confirmation::Remote {
            mutation: Mutation::Remote { source: Source::new(snapshot), action: Action::Detach { oid: oid.clone() } },
            title: format!("Check out {}?", &oid[..12]),
            detail: "This enters detached HEAD: you will be viewing this commit without a current branch. Create or switch to a branch before making commits you want to keep. Git will refuse changes that overwrite local work.".into(),
        });
        cx.notify();
    }

    fn toggle_history_commit(&mut self, index: usize, cx: &mut Context<Self>) {
        self.history.cursor = Some(HistoryRow::Commit(index));
        if self.history.selected == Some(index) {
            self.history.expanded = !self.history.expanded;
            cx.notify();
        } else {
            self.select_commit(index, cx);
        }
    }

    // Flatten the accordion so commits and files share one virtualized scroll area.
    fn history_rows(&self, cx: &App) -> Vec<HistoryRow> {
        let query = self.history.query.read(cx).value().to_lowercase();
        let mut rows = Vec::new();
        for (index, commit) in self.history.commits.iter().enumerate() {
            if !commit.matches(&query) {
                continue;
            }
            rows.push(HistoryRow::Commit(index));
            if self.history.selected == Some(index) && self.history.expanded {
                rows.push(HistoryRow::FilesSummary);
                if self.history.files_error.is_some() {
                    rows.push(HistoryRow::FilesError);
                }
                rows.extend((0..self.history.files.len()).map(HistoryRow::File));
            }
        }
        rows
    }

    fn sync_history_rows(&self, cx: &App) -> Vec<HistoryRow> {
        let rows = self.history_rows(cx);
        let mut previous = self.history.rows.borrow_mut();
        if *previous != rows {
            // An empty list may already have painted while history loaded. A
            // splice at its scroll anchor would move that anchor past the page.
            if previous.is_empty() {
                self.history.scroll.reset(rows.len());
                *previous = rows.clone();
                return rows;
            }
            let prefix = previous
                .iter()
                .zip(&rows)
                .take_while(|(a, b)| a == b)
                .count();
            let suffix = previous[prefix..]
                .iter()
                .rev()
                .zip(rows[prefix..].iter().rev())
                .take_while(|(a, b)| a == b)
                .count();
            self.history.scroll.splice(
                prefix..previous.len() - suffix,
                rows.len() - prefix - suffix,
            );
            *previous = rows.clone();
        }
        rows
    }

    fn history_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.keystroke.modifiers.modified() {
            return;
        }
        let rows: Vec<_> = self
            .history_rows(cx)
            .into_iter()
            .filter(|row| matches!(row, HistoryRow::Commit(_) | HistoryRow::File(_)))
            .collect();
        let current = self
            .history
            .cursor
            .and_then(|cursor| rows.iter().position(|row| *row == cursor));
        let cursor = current.and_then(|index| rows.get(index).copied());
        let key = event.keystroke.key.as_str();
        match key {
            "left" => {
                if let Some(index) = self.history.selected {
                    if matches!(cursor, Some(HistoryRow::File(_))) {
                        self.history.cursor = Some(HistoryRow::Commit(index));
                    } else if cursor == Some(HistoryRow::Commit(index)) {
                        self.history.expanded = false;
                    }
                }
            }
            "right" | "enter" | "space" => match cursor.or_else(|| rows.first().copied()) {
                Some(HistoryRow::Commit(index)) => {
                    if key == "right" && self.history.selected == Some(index) {
                        self.history.expanded = true;
                    } else {
                        self.toggle_history_commit(index, cx);
                    }
                }
                Some(HistoryRow::File(index)) => self.select_history_file(index, cx),
                _ => {}
            },
            "up" | "down" | "home" | "end" => {
                let next = match key {
                    "up" => current.unwrap_or(0).saturating_sub(1),
                    "down" => {
                        current.map_or(0, |index| (index + 1).min(rows.len().saturating_sub(1)))
                    }
                    "home" => 0,
                    _ => rows.len().saturating_sub(1),
                };
                if let Some(row) = rows.get(next).copied() {
                    self.history.cursor = Some(row);
                    match row {
                        HistoryRow::Commit(index) => {
                            if self.history.selected != Some(index) {
                                self.select_commit(index, cx);
                            }
                        }
                        HistoryRow::File(index) => self.select_history_file(index, cx),
                        _ => {}
                    }
                }
            }
            _ => return,
        }
        let rows = self.sync_history_rows(cx);
        if let Some(index) = self
            .history
            .cursor
            .and_then(|cursor| rows.iter().position(|row| *row == cursor))
        {
            self.history.scroll.scroll_to_reveal_item(index);
        }
        cx.notify();
        window.prevent_default();
        cx.stop_propagation();
    }

    fn render_history_row(&self, row: HistoryRow, cx: &mut Context<Self>) -> AnyElement {
        match row {
            HistoryRow::Commit(index) => {
                let commit = &self.history.commits[index];
                let expanded = self.history.selected == Some(index) && self.history.expanded;
                let tooltip = format!(
                    "{}\n{} · {}\n{}",
                    commit.subject,
                    commit.author,
                    commit.time_label(),
                    commit.decorations
                );
                Button::new(("history-commit", index))
                    .tab_stop(false)
                    .ghost()
                    .w_full()
                    .rounded_none()
                    .accessibility_label(format!(
                        "{} commit {}: {}",
                        if expanded { "Collapse" } else { "Expand" },
                        &commit.oid[..7],
                        commit.subject
                    ))
                    .h(px(64.))
                    .px_3()
                    .py_2()
                    .cursor_pointer()
                    .border_b_1()
                    .border_color(cx.theme().border.opacity(0.45))
                    .when(self.history.selected == Some(index), |view| {
                        view.bg(cx.theme().accent)
                    })
                    .when(self.history.cursor == Some(row), |view| {
                        view.border_l_2().border_color(cx.theme().ring)
                    })
                    .tooltip(tooltip)
                    .child(
                        v_flex()
                            .w_full()
                            .min_w_0()
                            .gap_1()
                            .child(
                                h_flex()
                                    .gap_1()
                                    .min_w_0()
                                    .child(
                                        Icon::new(if expanded {
                                            IconName::ChevronDown
                                        } else {
                                            IconName::ChevronRight
                                        })
                                        .size(px(12.))
                                        .flex_none(),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_sm()
                                            .font_semibold()
                                            .truncate()
                                            .child(commit.subject.clone()),
                                    ),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(
                                        div()
                                            .flex_none()
                                            .font_family(crate::fonts::TERMINAL_FONT_FAMILY)
                                            .child(format!(
                                                "{} {}",
                                                commit.graph,
                                                &commit.oid[..7]
                                            )),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .child(commit.author.clone()),
                                    )
                                    .child(
                                        div().flex_none().child(
                                            commit
                                                .time_label()
                                                .split(" · ")
                                                .next()
                                                .unwrap_or_default()
                                                .to_owned(),
                                        ),
                                    ),
                            ),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.history.focus.focus(window, cx);
                        this.toggle_history_commit(index, cx);
                    }))
                    .into_any_element()
            }
            HistoryRow::FilesSummary => div()
                .h(px(26.))
                .pl(px(30.))
                .pr_3()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if self.history.files_loading {
                    "Loading changed files…".into()
                } else {
                    format!("Changed files · {}", self.history.files.len())
                })
                .into_any_element(),
            HistoryRow::FilesError => v_flex()
                .px_3()
                .py_2()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(self.history.files_error.clone().unwrap_or_default()),
                )
                .child(
                    Button::new("history-files-retry")
                        .small()
                        .ghost()
                        .label("Retry files")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(index) = this.history.selected {
                                this.select_commit(index, cx);
                            }
                        })),
                )
                .into_any_element(),
            HistoryRow::File(index) => {
                let file = &self.history.files[index];
                let path = file.change.path.to_string_lossy();
                let label = if let Some(original) = &file.change.original_path {
                    format!(
                        "{}  {} → {path}",
                        file.change.kind.label(),
                        original.display()
                    )
                } else {
                    format!("{}  {path}", file.change.kind.label())
                };
                Button::new(("history-file", index))
                    .tab_stop(false)
                    .small()
                    .ghost()
                    .h(px(30.))
                    .w_full()
                    .pl(px(30.))
                    .pr_3()
                    .accessibility_label(label.clone())
                    .tooltip(label.clone())
                    .child(div().w_full().truncate().text_left().text_xs().child(label))
                    .when(self.history.selected_file == Some(index), |view| {
                        view.bg(cx.theme().accent)
                    })
                    .when(self.history.cursor == Some(row), |view| {
                        view.border_l_2().border_color(cx.theme().ring)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.history.focus.focus(window, cx);
                        this.history.cursor = Some(HistoryRow::File(index));
                        this.select_history_file(index, cx);
                    }))
                    .into_any_element()
            }
        }
    }

    pub(super) fn render_history(
        &self,
        snapshot: &RepositorySnapshot,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let query = self
            .history
            .query
            .read(cx)
            .value()
            .to_string()
            .to_lowercase();
        let indices: Vec<usize> = self
            .history
            .commits
            .iter()
            .enumerate()
            .filter(|(_, c)| c.matches(&query))
            .map(|(i, _)| i)
            .collect();
        let count = indices.len();
        let scope = self
            .history
            .scope
            .as_deref()
            .and_then(|reference| snapshot.branches.iter().find(|b| b.reference == reference))
            .map(|b| b.name.clone())
            .unwrap_or_else(|| "HEAD".into());
        let mut sidebar = v_flex()
            .w(px(340.))
            .max_w(relative(0.4))
            .bg(cx.theme().sidebar)
            .h_full()
            .min_h_0()
            .flex_none()
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .h(px(40.))
                    .px_4()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .font_semibold()
                            .child("Commit history"),
                    )
                    .child(
                        Button::new("history-scope")
                            .small()
                            .ghost()
                            .max_w(px(160.))
                            .icon(gpui_kit::assets::IconName::GitBranch)
                            .label(scope)
                            .tooltip("Choose history branch")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.history.choose_scope = !this.history.choose_scope;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .px_3()
                    .pb_2()
                    .child(Input::new(&self.history.query).small()),
            );
        if self.history.choose_scope {
            sidebar = sidebar
                .child(div().px_2().pb_2().child(Input::new(&self.branch_query)))
                .child(
                    Button::new("history-head")
                        .ghost()
                        .small()
                        .label("Current HEAD")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history.scope = None;
                            this.history.choose_scope = false;
                            this.reset_history(cx);
                        })),
                );
            let branch_query = self
                .branch_query
                .read(cx)
                .value()
                .to_string()
                .to_lowercase();
            let refs: Vec<_> = snapshot
                .branches
                .iter()
                .filter(|b| b.name.to_lowercase().contains(&branch_query))
                .map(|b| (b.reference.clone(), b.name.clone()))
                .collect();
            sidebar = sidebar.child(
                uniform_list(
                    "history-scopes",
                    refs.len(),
                    cx.processor(move |_, range: std::ops::Range<usize>, _, cx| {
                        range
                            .map(|index| {
                                let (reference, name) = refs[index].clone();
                                Button::new(("history-ref", index))
                                    .ghost()
                                    .small()
                                    .w_full()
                                    .h(px(30.))
                                    .label(name)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.history.scope = Some(reference.clone());
                                        this.history.choose_scope = false;
                                        this.reset_history(cx);
                                    }))
                            })
                            .collect()
                    }),
                )
                .flex_1()
                .min_h_0(),
            );
        } else {
            sidebar = sidebar
                .child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!(
                            "{count} matching · {} loaded",
                            self.history.commits.len()
                        )),
                )
                .child({
                    let rows = self.sync_history_rows(cx);
                    let view = cx.entity();
                    div()
                        .id("history-commits")
                        .track_focus(&self.history.focus)
                        .on_key_down(cx.listener(Self::history_key_down))
                        .border_1()
                        .border_color(cx.theme().border)
                        .focus(|style| style.border_color(cx.theme().ring))
                        .flex_1()
                        .min_h_0()
                        .child(
                            list(self.history.scroll.clone(), move |index, _, cx| {
                                view.update(cx, |this, cx| this.render_history_row(rows[index], cx))
                            })
                            .size_full(),
                        )
                })
                .when(count == 0, |view| {
                    view.child(div().p_3().text_sm().child(if self.history.tip.is_none() {
                        "No commits yet"
                    } else if self.history.loading {
                        "Loading history…"
                    } else {
                        "No matching loaded commits"
                    }))
                })
                .when_some(self.history.error.clone(), |view, error| {
                    view.child(
                        div()
                            .p_2()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(error),
                    )
                })
                .child(
                    Button::new("history-load-more")
                        .ghost()
                        .small()
                        .label(if self.history.loading {
                            "Loading…"
                        } else if self.history.error.is_some() {
                            "Retry"
                        } else {
                            "Load more"
                        })
                        .disabled(
                            self.history.loading
                                || !self.history.more
                                || self.history.tip.is_none(),
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.load_history_page(cx))),
                );
        }
        h_flex()
            .size_full()
            .min_h_0()
            .child(sidebar)
            .child(self.render_history_detail(cx))
            .into_any_element()
    }

    fn render_history_detail(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(commit) = self
            .history
            .selected
            .and_then(|i| self.history.commits.get(i))
        else {
            return self
                .render_empty(
                    "Select a commit",
                    "Choose a commit to inspect its metadata, files and diff.",
                    cx,
                )
                .into_any_element();
        };
        let oid = commit.oid.clone();
        let detach_oid = oid.clone();
        let busy = self.active_mutation.is_some();
        let comparison = match commit.parents.first() {
            None => "Root commit · compared with empty tree".into(),
            Some(parent) if commit.parents.len() > 1 => format!(
                "Merge commit · compared with first parent {}",
                &parent[..12]
            ),
            Some(parent) => format!("Compared with parent {}", &parent[..12]),
        };
        let git = cx.entity().downgrade();
        let subject = commit.subject.clone();
        let mut metadata = v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                h_flex().gap_2().min_w_0()
                    .child(div().id("history-commit-subject").flex_1().min_w_0()
                        .text_sm().font_semibold().truncate()
                        .tooltip(move |window, cx| Tooltip::new(subject.clone()).build(window, cx))
                        .child(commit.subject.clone()))
                    .child(Button::new("history-metadata-toggle").small().ghost().size(px(24.))
                        .icon(IconName::Info)
                        .selected(self.history.show_metadata)
                        .accessibility_label("Commit details")
                        .tooltip(if self.history.show_metadata { "Hide commit details" } else { "Show commit details" })
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.history.show_metadata = !this.history.show_metadata;
                            cx.notify();
                        })))
                    .child(Button::new("history-copy").small().ghost().size(px(24.))
                        .icon(IconName::Copy).accessibility_label("Copy commit id").tooltip("Copy commit id")
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(oid.clone()))
                        })))
                    .child(Button::new("history-commit-actions").small().ghost().size(px(24.))
                        .icon(IconName::Ellipsis).accessibility_label("Commit actions").tooltip("Commit actions")
                        .dropdown_menu_with_anchor(gpui_kit::Anchor::TopRight, move |menu, _, _| {
                            let branch_git = git.clone();
                            let detach_git = git.clone();
                            let detach_oid = detach_oid.clone();
                            menu.item(PopupMenuItem::new("Create branch here").icon(gpui_kit::assets::IconName::GitBranch).disabled(busy)
                                .on_click(move |_, _, cx| {
                                    let _ = branch_git.update(cx, |this, cx| {
                                        this.history.create_branch = !this.history.create_branch;
                                        cx.notify();
                                    });
                                }))
                                .item(PopupMenuItem::new("Check out commit…").disabled(busy)
                                    .on_click(move |_, _, cx| {
                                        let _ = detach_git.update(cx, |this, cx| this.request_detach(detach_oid.clone(), cx));
                                    }))
                        })),
            )
            .child(
                h_flex().gap_2().min_w_0().text_xs().text_color(cx.theme().muted_foreground)
                    .child(div().flex_none().font_family(crate::fonts::TERMINAL_FONT_FAMILY).child(commit.oid[..12].to_owned()))
                    .child(div().min_w_0().truncate().child(format!("{} · {}", commit.author, commit.time_label())))
                    .when(!commit.decorations.is_empty(), |view| {
                        let decorations = commit.decorations.clone();
                        view.child(div().id("history-commit-refs").min_w_0().truncate().px_2().rounded_sm().bg(cx.theme().muted)
                            .tooltip(move |window, cx| Tooltip::new(decorations.clone()).build(window, cx))
                            .child(commit.decorations.clone()))
                    }),
            )
            .when(self.history.show_metadata, |view| {
                view.child(v_flex().gap_2().pt_2()
                    .child(self.render_detail("Commit", commit.oid.clone(), cx))
                    .child(self.render_detail("Author", format!("{} <{}> · {}", commit.author, commit.email, commit.authored), cx))
                    .child(self.render_detail("Committer", format!("{} <{}> · {}", commit.committer, commit.committer_email, commit.committed), cx))
                    .child(self.render_detail("Parents", if commit.parents.is_empty() { "none".into() } else { commit.parents.join(" · ") }, cx))
                    .when(!commit.body.is_empty(), |view| view.child(div().text_sm().child(commit.body.clone())))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(comparison))
                    .when(commit.metadata_truncated, |view| view.child(div().text_xs().text_color(cx.theme().muted_foreground)
                        .child("Large commit metadata is truncated: body limit 64 KiB; other fields 4 KiB. Copy the id to inspect the complete commit in Git."))))
            });
        if self.history.create_branch {
            let oid = commit.oid.clone();
            metadata = metadata.child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.history.branch_name)))
                    .child(
                        Button::new("history-create-confirm")
                            .small()
                            .primary()
                            .label("Create branch")
                            .disabled(
                                busy || self.history.branch_name.read(cx).value().trim().is_empty(),
                            )
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let name = this.history.branch_name.read(cx).value().to_string();
                                this.request_remote(
                                    Action::CreateAt {
                                        name,
                                        oid: oid.clone(),
                                    },
                                    window,
                                    cx,
                                );
                            })),
                    ),
            );
        }
        let diff = v_flex()
            .flex_1().min_w_0().h_full().min_h_0()
            .when_some(self.history.selected_file.and_then(|index| self.history.files.get(index)), |view, file| {
                let path = file.change.path.to_string_lossy().into_owned();
                let detail = if let Some(original) = &file.change.original_path {
                    format!("{} · {} → {}", file.change.kind.label(), original.display(), path)
                } else { format!("{} · {}", file.change.kind.label(), path) };
                view.child(h_flex().h(px(34.)).flex_none().gap_2().px_3().border_b_1().border_color(cx.theme().border)
                    .child(div().id("history-diff-path").flex_1().min_w_0().truncate().text_sm().font_semibold()
                        .tooltip(move |window, cx| Tooltip::new(detail.clone()).build(window, cx)).child(path))
                    .child(self.history.viewer.update(cx, |viewer, cx| viewer.render_header_controls(cx))))
            })
            .child(div().flex_1().min_h_0().child(if self.history.selected_file.is_some() {
                self.history.viewer.clone().into_any_element()
            } else {
                self.render_empty(
                    if self.history.files_loading { "Loading changed files…" } else { "Select a changed file" },
                    "Expand the commit in the sidebar and choose a file to view its changes.", cx,
                ).into_any_element()
            }));
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .child(
                div()
                    .id("history-metadata")
                    .max_h(px(260.))
                    .flex_none()
                    .overflow_y_scroll()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(metadata),
            )
            .child(diff)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use std::{cell::RefCell, process::Command, rc::Rc};

    #[gpui_kit::test]
    fn history_selection_filter_and_reset_reject_obsolete_requests(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "History Test"]);
        git(&["config", "user.email", "history@example.com"]);
        git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(root.join("root.txt"), "root\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "root commit"]);
        std::fs::write(root.join("new.txt"), "new\n").unwrap();
        std::fs::write(root.join("root.txt"), "root updated\n").unwrap();
        git(&["add", "-A"]);
        git(&["commit", "-m", "new commit"]);
        let snapshot = load_snapshot(root).unwrap();
        let page = history::load_page(
            root,
            snapshot.oid.as_deref().unwrap(),
            0,
            vec![],
            &AtomicBool::new(false),
        )
        .unwrap();
        cx.update(gpui_kit::init);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let checkout = root.to_owned();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let dialog =
                cx.new(|cx| GitView::new(checkout, HashSet::new(), None, Vec::new(), window, cx));
            dialog.update(cx, |this, cx| {
                this.generation += 1; // reject the initial status request
                this.load_in_flight = false;
                this.state = LoadState::Loaded(Box::new(snapshot));
                this.mode = GitMode::History;
                this.history.more = false;
                cx.notify();
            });
            *slot.borrow_mut() = Some(dialog.clone());
            gpui_kit::component::Root::new(dialog, window, cx)
        });
        let dialog = holder.borrow().clone().unwrap();
        // Paint the empty list before the first page arrives, as in the app.
        test_cx.update(|window, cx| window.render_frame(cx));
        dialog.update(test_cx, |this, cx| {
            this.history.commits = page.commits.iter().cycle().take(100).cloned().collect();
            cx.notify();
        });
        test_cx.update(|window, cx| window.render_frame(cx));
        dialog.read_with(test_cx, |this, _| {
            assert_eq!(this.history.scroll.logical_scroll_top().item_ix, 0);
        });
        dialog.update(test_cx, |this, cx| {
            this.history.commits = page.commits;
            let cancel = this.history.cancel.clone();
            let generation = this.history.generation;
            this.history_filter_changed(cx);
            assert!(cancel.load(Ordering::Relaxed));
            assert!(this.history.generation > generation);
            assert_eq!(this.history.commits.len(), 2);
            this.select_commit(0, cx);
            let cancel = this.history.detail_cancel.clone();
            this.select_commit(1, cx);
            assert!(cancel.load(Ordering::Relaxed));
        });
        test_cx.run_until_parked();
        dialog.read_with(test_cx, |this, _| {
            assert_eq!(this.history.selected, Some(1));
            assert!(!this.history.files_loading);
            assert_eq!(this.history.files.len(), 1);
            assert_eq!(this.history.files[0].change.path, Path::new("root.txt"));
        });
        test_cx.update(|window, cx| window.render_frame(cx));
        test_cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.history.focus.focus(window, cx);
            });
        });
        test_cx.simulate_keystrokes("home");
        assert_eq!(
            dialog.read_with(test_cx, |this, _| this.history.selected),
            Some(0)
        );
        test_cx.run_until_parked();
        dialog.read_with(test_cx, |this, cx| {
            assert!(this.history.expanded);
            assert_eq!(this.history.selected_file, Some(0));
            assert_eq!(
                this.history_rows(cx),
                vec![
                    HistoryRow::Commit(0),
                    HistoryRow::FilesSummary,
                    HistoryRow::File(0),
                    HistoryRow::File(1),
                    HistoryRow::Commit(1),
                ]
            );
        });
        test_cx.simulate_keystrokes("down down");
        dialog.read_with(test_cx, |this, _| {
            assert_eq!(this.history.selected, Some(0));
            assert_eq!(this.history.selected_file, Some(1));
            assert_eq!(this.history.cursor, Some(HistoryRow::File(1)));
        });
        test_cx.simulate_keystrokes("left left");
        dialog.read_with(test_cx, |this, cx| {
            assert!(!this.history.expanded);
            assert_eq!(
                this.history.selected_file,
                Some(1),
                "collapse retains the diff"
            );
            assert_eq!(
                this.history_rows(cx),
                vec![HistoryRow::Commit(0), HistoryRow::Commit(1)]
            );
        });
        test_cx.simulate_keystrokes("enter");
        dialog.read_with(test_cx, |this, _| assert!(this.history.expanded));
        test_cx.simulate_keystrokes("end");
        assert_eq!(
            dialog.read_with(test_cx, |this, _| this.history.selected),
            Some(1)
        );
        test_cx.simulate_keystrokes("home");
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(("history-commit", 0_usize), cx);
        });
        dialog.read_with(test_cx, |this, _| assert!(!this.history.expanded));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click(("history-commit", 0_usize), cx);
        });
        dialog.read_with(test_cx, |this, _| assert!(this.history.expanded));
        test_cx.simulate_keystrokes("down");
        assert_eq!(
            dialog.read_with(test_cx, |this, _| this.history.selected_file),
            Some(0)
        );
        let (obsolete_generation, obsolete_file) = dialog.update(test_cx, |this, cx| {
            this.select_history_file(0, cx);
            let cancel = this.history.diff_cancel.clone();
            let generation = this.history.diff_generation;
            let file = this.history.files[0].clone();
            this.select_history_file(1, cx);
            assert!(cancel.load(Ordering::Relaxed));
            assert!(this.history.diff_generation > generation);
            (generation, file)
        });
        test_cx.run_until_parked();
        let stale_diff =
            crate::git::diff::load_history_diff(root, &obsolete_file, &AtomicBool::new(false))
                .unwrap();
        dialog.update(test_cx, |this, cx| {
            this.complete_history_diff(obsolete_generation, "new.txt".into(), Ok(stale_diff), cx);
            this.complete_history_diff(
                obsolete_generation,
                "new.txt".into(),
                Err("obsolete error".into()),
                cx,
            );
        });
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window
                    .find("diff-body")
                    .label()
                    .unwrap()
                    .contains("root.txt"),
                "obsolete results cannot replace the current file"
            );
            window.click("git-diff-split", cx);
            window.press("home", cx);
            window.press("down", cx);
            window.press("end", cx);
        });
        dialog.read_with(test_cx, |this, _| {
            assert_eq!(
                this.history.selected,
                Some(0),
                "diff keys leave the commit selection alone"
            );
            assert_eq!(this.history.selected_file, Some(1));
        });
        dialog.update(test_cx, |this, cx| {
            this.mode = GitMode::Changes;
            cx.notify();
        });
        test_cx.update(|window, cx| window.render_frame(cx));
        test_cx.update(|window, cx| {
            dialog.update(cx, |this, cx| {
                this.restore_commit_message("keep this message", window, cx);
                let focus = this.commit_message.read(cx).focus_handle(cx);
                focus.focus(window, cx);
                this.active_mutation = Some(Mutation::Commit {
                    message: "keep this message".into(),
                });
                this.set_active(false, cx);
                assert!(
                    this.active_mutation.is_some(),
                    "leaving Git keeps the operation alive"
                );
                this.set_active(true, cx);
                this.active_mutation = None;
                this.commit_completed("older message", window, cx);
                assert_eq!(
                    this.commit_message.read(cx).value().as_ref(),
                    "keep this message"
                );
                this.commit_completed("keep this message", window, cx);
                assert!(this.commit_message.read(cx).value().is_empty());
                this.restore_commit_message("keep this message", window, cx);
            });
        });
        test_cx.simulate_keystrokes("home down");
        assert_eq!(
            dialog.read_with(test_cx, |this, _| this.history.selected),
            Some(0),
            "editor arrows must not change history selection"
        );
        assert_eq!(
            dialog.read_with(test_cx, |this, cx| this
                .commit_message
                .read(cx)
                .value()
                .to_string()),
            "keep this message"
        );
        dialog.update(test_cx, |this, cx| {
            let oid = this.history.commits[1].oid.clone();
            this.request_detach(oid, cx);
            let Some(Confirmation::Remote {
                detail, mutation, ..
            }) = &this.confirmation
            else {
                panic!("detached confirmation expected")
            };
            assert!(detail.contains("detached HEAD"));
            assert!(matches!(
                mutation,
                Mutation::Remote {
                    action: Action::Detach { .. },
                    ..
                }
            ));
            this.history.scope = Some("refs/heads/no-longer-here".into());
            this.reset_history(cx);
            assert!(this.history.scope.is_none());
            assert!(this.history.selected.is_none());
            assert!(this.history.files.is_empty());
        });
    }
}
