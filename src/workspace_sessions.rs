use super::*;
use gpui_kit::base::{Accordion, AccordionHeader, AccordionItem, AccordionPanel, AccordionTrigger};
use gpui_kit::component::Disableable as _;
use gpui_kit::component::dialog::{Confirm, DialogFooter};
use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{StatefulInteractiveElement as _, TestSupportExt as _, WeakEntity};

/// One stop for navigation-mode `j`/`k` in the Agent sessions sidebar, in
/// display order (open sessions, pinned sessions, then recent catalog rows).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionNavTarget {
    Open(u64),
    Recent(SessionKey),
}

/// Shared projection keeps rendered rows and keyboard navigation identical.
struct SessionSidebarSections {
    open: Vec<u64>,
    pinned: Vec<SessionSummary>,
    recent: Vec<SessionSummary>,
}

impl SessionSidebarSections {
    fn expanded(mut self, expanded: [bool; 3]) -> Self {
        if !expanded[SessionSection::Open as usize] {
            self.open.clear();
        }
        if !expanded[SessionSection::Pinned as usize] {
            self.pinned.clear();
        }
        if !expanded[SessionSection::Recent as usize] {
            self.recent.clear();
        }
        self
    }

    fn project<'a>(
        open_sessions: impl Iterator<Item = (u64, &'a Path, Option<&'a SessionKey>)>,
        snapshot: &SessionSnapshot,
        pinned_keys: &[SessionKey],
        checkout: &Path,
        limit: usize,
        visible: impl Fn(&Path) -> bool,
    ) -> Self {
        let sessions: Vec<_> = open_sessions.collect();
        let open_keys: HashSet<_> = sessions.iter().filter_map(|(_, _, key)| *key).collect();
        let mut open: Vec<_> = sessions
            .iter()
            .filter(|(_, checkout, _)| visible(checkout))
            .map(|(id, _, _)| *id)
            .collect();
        open.sort_by_key(|id| std::cmp::Reverse(*id));
        let pins: HashSet<_> = pinned_keys.iter().collect();
        // Resolve only from the account-filtered catalog. Unavailable pins
        // stay persisted and reappear when discovery recovers.
        let mut seen = HashSet::new();
        let pinned = pinned_keys
            .iter()
            .filter(|key| seen.insert(*key) && !open_keys.contains(*key))
            .filter_map(|key| snapshot.sessions.iter().find(|s| &s.key == key))
            .filter(|session| visible(&session.checkout))
            .cloned()
            .collect();
        let recent = snapshot
            .sessions
            .iter()
            .filter(|session| {
                session.checkout == checkout
                    && !open_keys.contains(&session.key)
                    && !pins.contains(&session.key)
            })
            .take(limit)
            .cloned()
            .collect();
        Self {
            open,
            pinned,
            recent,
        }
    }

    fn nav_order(self) -> Vec<SessionNavTarget> {
        self.open
            .into_iter()
            .map(SessionNavTarget::Open)
            .chain(
                self.pinned
                    .into_iter()
                    .chain(self.recent)
                    .map(|session| SessionNavTarget::Recent(session.key)),
            )
            .collect()
    }
}

/// Shared presentation for live sessions and rows from the recent catalog.
struct SessionSidebarRow {
    target: SessionNavTarget,
    title: String,
    provider: String,
    repository: Option<String>,
    age: Option<String>,
    tooltip: String,
    icon: AnyElement,
    status: Option<ActivityState>,
    open_id: Option<u64>,
    pin_key: Option<SessionKey>,
    selected: bool,
    cursor: bool,
}

#[derive(Clone, Copy)]
enum SessionSection {
    Open,
    Pinned,
    Recent,
}

impl Workspace {
    fn render_session_section(
        &self,
        section: SessionSection,
        count: usize,
        separated: bool,
        content: impl IntoElement,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (id, title, scope, color) = match section {
            SessionSection::Open => (
                "open-sessions",
                "Open sessions",
                "Open sessions across repositories in the selected space",
                cx.theme().info,
            ),
            SessionSection::Pinned => (
                "pinned-sessions",
                "Pinned sessions",
                "Pinned sessions across repositories in the selected space",
                cx.theme().warning,
            ),
            SessionSection::Recent => (
                "recent-sessions",
                "Recent sessions",
                "Recent sessions for the current repository",
                cx.theme().muted_foreground,
            ),
        };
        let index = section as usize;
        let expanded = self.session_sections_expanded[index];
        let focus = self.session_section_focus[index].clone();
        let workspace = cx.entity().downgrade();
        let trigger = AccordionTrigger::new(SharedString::from(format!("toggle-{id}")))
            .open(expanded)
            .track_focus(&focus)
            .tab_stop(true)
            .aria_label(title)
            .aria_description(scope)
            .h_flex()
            .items_center()
            .gap_2()
            .px_4()
            .py_1()
            .bg(cx.theme().secondary.opacity(0.5))
            .border_1()
            .border_color(gpui_kit::transparent_black())
            .text_xs()
            .cursor_pointer()
            .hover(|header| header.bg(cx.theme().secondary))
            .focus(|header| header.border_color(cx.theme().ring))
            .tooltip(move |window, cx| Tooltip::new(scope).build(window, cx))
            .child(
                Icon::new(if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .flex_none()
                .text_color(cx.theme().muted_foreground),
            )
            .child(div().flex_1().font_semibold().child(title))
            .child(
                div()
                    .px_2()
                    .rounded_full()
                    .text_color(color)
                    .bg(color.opacity(0.12))
                    .child(count.to_string()),
            )
            .on_change(move |expanded, _, window, cx| {
                workspace
                    .update(cx, |this, cx| {
                        this.session_section_focus[index].focus(window, cx);
                        this.set_session_section_expanded(index, expanded, cx);
                    })
                    .ok();
            });
        Accordion::new(id)
            .v_flex()
            .flex_none()
            .w_full()
            .when(separated, |section| section.mt_3())
            .child(
                AccordionItem::new()
                    .v_flex()
                    .open(expanded)
                    .header(AccordionHeader::new(trigger))
                    .panel(AccordionPanel::new().child(content)),
            )
            .into_any_element()
    }

    pub(super) fn set_session_section_expanded(
        &mut self,
        index: usize,
        expanded: bool,
        cx: &mut Context<Self>,
    ) {
        self.session_sections_expanded[index] = expanded;
        self.close_navigation(cx);
        cx.notify();
    }
}

/// Keyboard-navigable agent chooser behind `New session`.
/// Backed by the kit's `List`, so up/down move the selection, Enter confirms,
/// Esc closes, and the global default (Settings > Agent) starts selected
/// with a `Default` badge. Only enabled harnesses are offered; the list never
/// ends empty — an empty enable set falls back to all harnesses.
/// `ListDelegate` callbacks cannot borrow the workspace, so the delegate
/// holds a weak handle instead.
struct AgentPicker {
    agents: Vec<AgentKind>,
    default: AgentKind,
    selected: usize,
    workspace: WeakEntity<Workspace>,
    icons: Rc<crate::agent_icons::AgentIconTiles>,
}

impl ListDelegate for AgentPicker {
    type Item = ListItem;

    fn items_count(&self, _section: usize, _cx: &App) -> usize {
        self.agents.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let agent = self.agents.get(ix.row).copied()?;
        let icon = crate::agent_icons::agent_icon(agent, &self.icons, crate::agent_icons::ICON_PX);
        Some(
            ListItem::new(("new-session-agent", ix.row)).child(
                v_flex()
                    .w_full()
                    .items_start()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(icon)
                            .child(div().text_sm().font_semibold().child(agent.label()))
                            .when(agent == self.default, |row| {
                                row.child(
                                    div().text_xs().text_color(rgb(0x737878)).child("Default"),
                                )
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(0x737878))
                            .child(agent.description()),
                    ),
            ),
        )
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        if let Some(ix) = ix {
            self.selected = ix.row;
        }
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) {
        let agent = self
            .agents
            .get(self.selected)
            .copied()
            .or_else(|| self.agents.first().copied())
            .unwrap_or(AgentKind::DEFAULT);
        let workspace = self.workspace.clone();
        window.close_dialog(cx);
        workspace
            .update(cx, |this, cx| this.start_agent_session(agent, window, cx))
            .ok();
    }
}

impl Workspace {
    pub(crate) fn ensure_repository_agent_idle(
        &mut self,
        key: &str,
        cx: &App,
    ) -> anyhow::Result<()> {
        self.remember_agent_pane(cx);
        if let Some(root) = &self.data_root
            && let Some(checkout) = crate::data::checkout_for(root, key)
        {
            anyhow::ensure!(
                !self
                    .open_sessions
                    .values()
                    .any(|session| session.checkout == checkout_identity(&checkout)),
                "Close this repository's open agent sessions before moving it to another space."
            );
        }
        Ok(())
    }

    pub(crate) fn ensure_agent_accounts_idle(
        &mut self,
        space: &str,
        cx: &App,
    ) -> anyhow::Result<()> {
        self.remember_agent_pane(cx);
        if let Some(root) = &self.data_root {
            let (repositories, errors) = crate::data::all_repositories(root)?;
            anyhow::ensure!(errors.is_empty(), "Could not check open agent accounts");
            let busy = repositories
                .iter()
                .filter(|repo| crate::data::space_eq(&repo.space, space))
                .filter_map(|repo| repo.checkout_path.as_ref())
                .any(|checkout| {
                    self.open_sessions
                        .values()
                        .any(|session| session.checkout == checkout_identity(checkout))
                });
            anyhow::ensure!(
                !busy,
                "Close the open agent sessions in {space} before changing its accounts."
            );
        }
        Ok(())
    }

    pub(super) fn new_review(cwd: &Path, cx: &mut Context<Self>) -> Entity<ReviewView> {
        let review = cx.new(|cx| ReviewView::new(cwd, cx));
        let checkout = cwd.to_owned();
        cx.subscribe(
            &review,
            move |this, review, event: &crate::review::OpenReviewFile, cx| {
                let file = checkout.join(&event.path);
                let error = if !file.is_file() {
                    Some(format!(
                        "{} is unavailable in the current checkout.",
                        event.path
                    ))
                } else if this.editor_instance != Some(EditorChoice::BuiltIn) {
                    Some("Choose Devcroft editor in Settings to open Review files here.".into())
                } else if let Some(editor) = this.native_editor.as_ref() {
                    editor.update(cx, |view, cx| view.request_open(file, event.line, cx));
                    this.active_tab = WorkspaceTab::Editor;
                    cx.notify();
                    None
                } else {
                    Some("The built-in editor is not ready yet.".into())
                };
                review.update(cx, |view, cx| view.set_open_error(error, cx));
            },
        )
        .detach();
        review
    }

    fn create_agent_session(
        &mut self,
        agent: AgentKind,
        cwd: &Path,
        title: &str,
        cx: &mut Context<Self>,
    ) -> Entity<TerminalPane> {
        let pane = cx
            .new(|cx| TerminalPane::new(WorkspaceTab::Agent, cwd, agent, &self.agent_activity, cx));
        if !pane.read(cx).launch_failed()
            && let Some(id) = pane.read(cx).launch_id()
        {
            self.open_sessions.insert(
                id,
                OpenAgentSession {
                    pane: pane.clone(),
                    checkout: cwd.to_owned(),
                    agent,
                    key: None,
                    title: title.into(),
                    created_at: crate::relative_time::current_unix_secs(),
                },
            );
        }
        pane
    }

    pub(super) fn remember_agent_pane(&mut self, cx: &App) {
        let Some(pane) = self.tabs[WorkspaceTab::Agent as usize].clone() else {
            return;
        };
        let Some(id) = pane.read(cx).launch_id() else {
            return;
        };
        self.open_sessions
            .entry(id)
            .or_insert_with(|| OpenAgentSession {
                pane,
                checkout: self.working_directory.clone(),
                agent: self.session_agent,
                key: None,
                title: "New session".into(),
                created_at: crate::relative_time::current_unix_secs(),
            });
    }

    pub(super) fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        if self.session_refreshing {
            return;
        }
        self.session_refreshing = true;
        let catalog = self.session_catalog.clone();
        let root = self.data_root.clone();
        cx.spawn(async move |this, cx| {
            let projects = cx
                .background_spawn(async move {
                    let mut projects = root
                        .as_ref()
                        .map(|r| recent_repositories(r, usize::MAX))
                        .unwrap_or_default();
                    for project in &mut projects {
                        project.checkout_path = checkout_identity(&project.checkout_path);
                    }
                    catalog.refresh();
                    projects
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.session_refreshing = false;
                this.session_projects = projects;
                this.publish_sessions(cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn publish_sessions(&mut self, cx: &mut Context<Self>) {
        if let Some(root) = &self.data_root
            && let Ok(state) = DeviceStore::new(root).load()
        {
            self.pinned_sessions = state.pinned_sessions;
        }
        self.acknowledge_project_completions();
        self.session_snapshot = self.session_catalog.snapshot();
        self.remember_agent_pane(cx);
        let accounts = crate::agent_accounts::Accounts::load(self.data_root.as_ref());
        let index = WorkspaceTab::Agent as usize;
        // External metadata edits or portable sync can also change ownership.
        // Retain old processes for explicit closing, but detach their panes.
        if self.tabs[index].as_ref().is_some_and(|pane| {
            !pane.read(cx).launch_failed()
                && !accounts.as_ref().is_ok_and(|accounts| {
                    pane.read(cx)
                        .account_matches(&accounts.for_checkout(&self.working_directory))
                })
        }) {
            self.tabs[index] = None;
            self.agent_autostart.remove(&self.working_directory);
        }
        for open in self.open_sessions.values_mut() {
            if !accounts.as_ref().is_ok_and(|accounts| {
                open.pane
                    .read(cx)
                    .account_matches(&accounts.for_checkout(&open.checkout))
            }) {
                continue;
            }
            if let Some(native) = open.pane.read(cx).native_session() {
                // The provider signal, not recency or title, establishes identity.
                if let Some(session) = self.session_snapshot.sessions.iter().find(|s| {
                    s.key.provider == open.agent.id()
                        && s.key.id == native
                        && s.checkout == open.checkout
                }) {
                    open.key = Some(session.key.clone());
                    open.title = session.title.clone();
                }
            }
        }
        // Fallback for harnesses without an identity signal (Codex has no
        // observer) or missed provider events: when native is still unknown,
        // adopt the single unclaimed catalog session for the same
        // provider+checkout updated after the pane existed. Requires exactly
        // one candidate for the pane and exactly one claimant globally, so
        // concurrent new sessions never cross-adopt and old history is never
        // mistaken for the new pane.
        let claimed: HashSet<SessionKey> = self
            .open_sessions
            .values()
            .filter_map(|open| open.key.clone())
            .collect();
        let mut candidates_by_open: HashMap<u64, Vec<SessionKey>> = HashMap::new();
        for (&id, open) in &self.open_sessions {
            if !accounts.as_ref().is_ok_and(|accounts| {
                open.pane
                    .read(cx)
                    .account_matches(&accounts.for_checkout(&open.checkout))
            }) {
                continue;
            }
            if open.key.is_some() || open.pane.read(cx).native_session().is_some() {
                continue;
            }
            let threshold = open.created_at.saturating_sub(60);
            let candidates: Vec<SessionKey> = self
                .session_snapshot
                .sessions
                .iter()
                .filter(|session| {
                    session.key.provider == open.agent.id()
                        && session.checkout == open.checkout
                        && session.updated > 0
                        && session.updated >= threshold
                        && !claimed.contains(&session.key)
                })
                .map(|session| session.key.clone())
                .collect();
            candidates_by_open.insert(id, candidates);
        }
        let mut usage: HashMap<SessionKey, usize> = HashMap::new();
        for candidates in candidates_by_open.values() {
            for key in candidates {
                *usage.entry(key.clone()).or_default() += 1;
            }
        }
        for (&id, candidates) in &candidates_by_open {
            if candidates.len() == 1
                && usage.get(&candidates[0]) == Some(&1)
                && let Some(session) = self
                    .session_snapshot
                    .sessions
                    .iter()
                    .find(|session| session.key == candidates[0])
                && let Some(open) = self.open_sessions.get_mut(&id)
            {
                open.key = Some(session.key.clone());
                open.title = session.title.clone();
            }
        }
        let mut cards = Vec::new();
        for session in &self.session_snapshot.sessions {
            let Some(label) = self.session_repository_label(&session.checkout) else {
                continue;
            };
            // Registered repositories filter by the active space; an
            // unregistered checkout (the current or a retained ad-hoc one)
            // has no space to filter on and stays visible — the user opened
            // it explicitly.
            if self
                .session_space(&session.checkout)
                .is_some_and(|space| !self.active_space.matches(&space))
            {
                continue;
            }
            cards.push((session.clone(), label));
            if cards.len() == HOME_LIMIT {
                break;
            }
        }
        self.home.update(cx, |view, cx| {
            view.set_sessions(
                cards,
                self.session_snapshot.errors.clone(),
                self.session_snapshot.loaded,
                cx,
            )
        });
        cx.notify();
    }

    /// Space of the registered repository whose checkout is `checkout`, when
    /// one exists. Unregistered checkouts return `None`.
    pub(super) fn session_space(&self, checkout: &Path) -> Option<String> {
        self.session_projects
            .iter()
            .chain(self.recent_repositories.iter())
            .find(|project| project.checkout_path == checkout)
            .map(|project| project.space.clone())
    }

    /// Display label for one session checkout: a registered repository's
    /// label when bound, otherwise the ad-hoc checkout's directory name while
    /// it is current or retained. The palette cache is consulted alongside
    /// the catalog's project list so session rows can label rows before the first
    /// catalog refresh republishes `session_projects`.
    pub(super) fn session_repository_label(&self, checkout: &Path) -> Option<String> {
        self.session_projects
            .iter()
            .chain(self.recent_repositories.iter())
            .find(|project| project.checkout_path == checkout)
            .map(|project| project.label().to_owned())
            .or_else(|| {
                (self.working_directory == checkout
                    || self.inactive_repositories.contains_key(checkout))
                .then(|| {
                    checkout
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                })
            })
    }

    /// Whether a session list is currently on screen: Home's Recent
    /// Activity or the Agent sidebar. Gates the
    /// periodic catalog refresh and the relative-time repaint so they run
    /// exactly while their labels are visible.
    pub(super) fn sessions_visible(&self) -> bool {
        self.git_page_origin.is_none()
            && !self.git_header_busy
            && (self.home_visible || self.active_tab == WorkspaceTab::Agent)
    }

    pub(super) fn open_agent_session(
        &mut self,
        key: SessionKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        // Location changes (including async completions racing an open HUD)
        // clear stale navigation state.
        self.close_navigation(cx);
        self.remember_agent_pane(cx);
        if let Some((&id, _)) = self
            .open_sessions
            .iter()
            .find(|(_, s)| s.key.as_ref() == Some(&key))
        {
            self.activate_open_session(id, window, cx);
            return;
        }
        self.session_navigation = self.session_navigation.wrapping_add(1);
        let generation = self.session_navigation;
        let root = self.data_root.clone();
        let catalog = self.session_catalog.clone();
        let mut open_checkouts: HashSet<PathBuf> =
            self.inactive_repositories.keys().cloned().collect();
        open_checkouts.insert(self.working_directory.clone());
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move {
                    let session = catalog.prepare_open(&key)?;
                    let project = root.as_ref().and_then(|root| {
                        recent_repositories(root, usize::MAX)
                            .into_iter()
                            .find(|p| p.checkout_path == session.checkout)
                    });
                    anyhow::ensure!(
                        project.is_some() || open_checkouts.contains(&session.checkout),
                        "This checkout is no longer registered or open"
                    );
                    if let (Some(root), Some(project)) = (&root, &project) {
                        record_repository_open(root, &project.key)?;
                    }
                    Ok::<_, anyhow::Error>((session, project.map(|p| p.key)))
                })
                .await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    if this.session_navigation != generation {
                        return;
                    }
                    match outcome {
                        Ok((session, repository)) => {
                            this.launch_recent_session(session, repository, window, cx)
                        }
                        Err(error) => {
                            window.push_notification(format!("Could not open session: {error}"), cx)
                        }
                    }
                })
            });
        })
        .detach();
    }

    fn launch_recent_session(
        &mut self,
        session: SessionSummary,
        repository: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        if let Some((&id, _)) = self
            .open_sessions
            .iter()
            .find(|(_, s)| s.key.as_ref() == Some(&session.key))
        {
            self.activate_open_session(id, window, cx);
            return;
        }
        let Some(agent) = session.agent() else {
            return;
        };
        let pane = cx.new(|cx| {
            TerminalPane::with_session(
                WorkspaceTab::Agent,
                &session.cwd,
                agent,
                &self.agent_activity,
                Some(&session),
                cx,
            )
        });
        if pane.read(cx).launch_failed() {
            window.push_notification(
                "Could not launch the session terminal. Your previous session is still open.",
                cx,
            );
            return;
        }
        let Some(id) = pane.read(cx).launch_id() else {
            return;
        };
        self.open_sessions.insert(
            id,
            OpenAgentSession {
                pane,
                checkout: session.checkout.clone(),
                agent,
                key: Some(session.key),
                title: session.title,
                created_at: crate::relative_time::current_unix_secs(),
            },
        );
        self.activate_open_session(id, window, cx);
        self.current_repository = repository;
        self.refresh_sessions(cx);
    }

    pub(super) fn focus_attention_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.agent_activity.snapshot();
        let id = self
            .open_sessions
            .iter()
            .filter(|(_, session)| session.checkout == self.working_directory)
            .filter(|(id, _)| {
                snapshot
                    .for_launch(**id)
                    .is_some_and(|a| a.state == ActivityState::NeedsAttention)
            })
            .map(|(id, _)| *id)
            .min();
        if let Some(id) = id {
            self.activate_open_session(id, window, cx);
        }
    }

    fn activate_open_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.close_navigation(cx);
        self.remember_agent_pane(cx);
        let Some(open) = self.open_sessions.get(&id) else {
            return;
        };
        let valid =
            crate::agent_accounts::Accounts::load(self.data_root.as_ref()).is_ok_and(|accounts| {
                open.pane
                    .read(cx)
                    .account_matches(&accounts.for_checkout(&open.checkout))
            });
        if !valid {
            window.push_notification("This session uses the repository's previous agent account. Close it and start a new session.", cx);
            return;
        }
        let (pane, checkout, agent) = (open.pane.clone(), open.checkout.clone(), open.agent);
        // The explicit live-session selection wins over an older catalog
        // resume still validating on a worker.
        self.session_navigation = self.session_navigation.wrapping_add(1);
        self.restore_repository_tabs(&checkout, cx);
        self.tabs[WorkspaceTab::Agent as usize] = Some(pane);
        self.session_agent = agent;
        self.active_tab = WorkspaceTab::Agent;
        self.current_repository = self
            .session_projects
            .iter()
            .find(|p| p.checkout_path == checkout)
            .map(|p| p.key.clone());
        self.project_name = checkout
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
            .into();
        self.resources.update(cx, |view, cx| {
            view.set_scope(
                crate::artifacts::Scope::Repository(self.current_repository.clone()),
                cx,
            )
        });
        self.enter_repository(window, cx);
        self.refresh_git_status(cx);
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Ask which harness a new session should use before spawning anything.
    /// Starting a session launches a process, so an explicit pick beats a
    /// mislaunch. The global default (Settings > Agent) starts selected;
    /// this choice applies to this session only, exactly like selecting a
    /// historical session. The picker is a real `List`, so arrow keys move
    /// the highlight and Enter confirms.
    pub(super) fn prompt_new_agent_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_header_busy && !self.can_leave_git(window, cx) {
            return;
        }
        // Opening a dialog clears stale navigation state; the dialog trap
        // owns the keyboard from here.
        self.close_navigation(cx);
        let workspace = cx.entity().downgrade();
        let project = self.project_name.clone();
        let icons = self.agent_icon_tiles.clone();
        let mut agents = self.enabled_agents.clone();
        if agents.is_empty() {
            agents = AgentKind::ALL.to_vec();
        }
        let default = if agents.contains(&self.default_agent) {
            self.default_agent
        } else {
            agents.first().copied().unwrap_or(AgentKind::DEFAULT)
        };
        let selected = agents
            .iter()
            .position(|agent| *agent == default)
            .unwrap_or(0);
        let list_state = cx.new(|cx| {
            ListState::new(
                AgentPicker {
                    agents,
                    default,
                    selected,
                    workspace,
                    icons,
                },
                window,
                cx,
            )
        });
        list_state.update(cx, |state, cx| {
            state.set_selected_index(Some(IndexPath::new(selected)), window, cx);
        });
        let dialog_list = list_state.clone();
        window.open_dialog(cx, move |dialog, _, cx| {
            dialog
                .title("New session")
                .w(px(460.))
                .child(
                    div()
                        .pb_3()
                        .text_sm()
                        .text_color(rgb(0x858989))
                        .child(format!("Choose the agent for a new session in {project}.")),
                )
                .child(div().w_full().h(px(288.)).child(List::new(&dialog_list)))
                .footer(
                    DialogFooter::new()
                        .child(
                            div()
                                .mr_auto()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("↑↓ Select · Enter Start"),
                        )
                        .child(
                            Button::new("cancel-new-session")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        ),
                )
        });
        // Opening the dialog focuses its own trap; hand focus to the list so
        // up/down and Enter work immediately without a click or Tab.
        list_state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// Launch a new session with the chosen harness in the current checkout.
    fn start_agent_session(
        &mut self,
        agent: AgentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.remember_agent_pane(cx);
        // An explicit New wins over an in-flight auto-resume: bump the
        // navigation so a stale open completion cannot replace this pane,
        // and mark the checkout started either way.
        self.session_navigation += 1;
        self.agent_autostart.insert(self.working_directory.clone());
        let pane =
            self.create_agent_session(agent, &self.working_directory.clone(), "New session", cx);
        if pane.read(cx).launch_failed() {
            window.push_notification("Could not create a session terminal", cx);
            return;
        }
        self.tabs[WorkspaceTab::Agent as usize] = Some(pane);
        self.session_agent = agent;
        // A new session can start from Home or any tab: reveal the Agent tab
        // and enter the checkout so the new pane is visible. The Agent tab is
        // already set, so `enter_repository` cannot auto-resume a second
        // session on the way in.
        self.active_tab = WorkspaceTab::Agent;
        self.enter_repository(window, cx);
        self.focus_active_pane(window, cx);
        self.refresh_sessions(cx);
        cx.notify();
    }

    fn close_agent_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let busy = self
            .agent_activity
            .snapshot()
            .for_launch(id)
            .is_some_and(|a| {
                matches!(
                    a.state,
                    ActivityState::Working
                        | ActivityState::NeedsAttention
                        | ActivityState::Starting
                        | ActivityState::Unavailable
                )
            });
        if busy {
            let workspace = cx.entity().downgrade();
            window.open_dialog(cx,move |dialog,_,_| {
                let workspace=workspace.clone();
                dialog.title("Close agent session?").child("This stops its terminal process. The saved conversation stays in the agent's history.")
                    .footer(DialogFooter::new()
                        .child(Button::new("cancel-close-session").label("Cancel").on_click(|_,window,cx|window.close_dialog(cx)))
                        .child(Button::new("confirm-close-session").primary().label("Close session").on_click(|_,window,cx|window.dispatch_action(Box::new(Confirm{secondary:false}),cx))))
                    .on_ok(move |_,window,cx| {let _=workspace.update(cx,|this,cx|this.remove_agent_session(id,window,cx));true})
            });
        } else {
            self.remove_agent_session(id, window, cx);
        }
    }
    fn remove_agent_session(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.open_sessions.remove(&id) {
            // Other views can retain the pane, so dropping the sidebar row is
            // insufficient to stop a terminal shared with an open agent session.
            session.pane.update(cx, |pane, cx| pane.close(cx));
        }
        let agent_index = WorkspaceTab::Agent as usize;
        let active = self.tabs[agent_index]
            .as_ref()
            .and_then(|p| p.read(cx).launch_id())
            == Some(id);
        if active {
            self.tabs[agent_index] = None;
        }
        for repository in self.inactive_repositories.values_mut() {
            if repository.tabs[agent_index]
                .as_ref()
                .and_then(|p| p.read(cx).launch_id())
                == Some(id)
            {
                repository.tabs[agent_index] = None;
            }
        }
        if active
            && let Some((&next, _)) = self
                .open_sessions
                .iter()
                .find(|(_, s)| s.checkout == self.working_directory)
        {
            self.activate_open_session(next, window, cx);
        }
        self.sync_activity_visibility(window, cx);
        cx.notify();
    }

    fn session_in_selected_space(&self, checkout: &Path) -> bool {
        self.session_space(checkout)
            .is_none_or(|space| self.active_space.matches(&space))
    }

    fn session_sidebar_sections(&self) -> SessionSidebarSections {
        SessionSidebarSections::project(
            self.open_sessions
                .iter()
                .map(|(&id, session)| (id, session.checkout.as_path(), session.key.as_ref())),
            &self.session_snapshot,
            &self.pinned_sessions,
            &self.working_directory,
            self.session_limit,
            |checkout| {
                self.session_in_selected_space(checkout)
                    && self.session_repository_label(checkout).is_some()
            },
        )
    }

    /// Display order for the sessions sidebar cursor, matching its sections.
    pub(super) fn session_nav_order(&self) -> Vec<SessionNavTarget> {
        self.session_sidebar_sections()
            .expanded(self.session_sections_expanded)
            .nav_order()
    }

    fn toggle_session_pin(&mut self, key: SessionKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = &self.data_root else {
            window.push_notification("Could not save session pin: local data is unavailable", cx);
            return;
        };
        let pinned = !self.pinned_sessions.contains(&key);
        match DeviceStore::new(root).set_session_pinned(&key, pinned) {
            Ok(pins) => {
                self.pinned_sessions = pins;
                self.close_navigation(cx);
                cx.notify();
            }
            Err(error) => {
                window.push_notification(format!("Could not save session pin: {error}"), cx);
            }
        }
    }

    /// Start the sidebar cursor at the active session (or the first row) when
    /// navigation mode opens on the Sessions pane.
    pub(super) fn init_session_cursor(
        &mut self,
        active: Option<u64>,
        active_key: Option<SessionKey>,
    ) {
        let order = self.session_nav_order();
        if order.is_empty() {
            self.session_cursor = None;
            return;
        }
        let position = active
            .and_then(|id| {
                order
                    .iter()
                    .position(|t| matches!(t, SessionNavTarget::Open(open) if *open == id))
            })
            .or_else(|| {
                active_key.as_ref().and_then(|key| {
                    order.iter().position(|t| match t {
                        SessionNavTarget::Open(id) => {
                            self.open_sessions.get(id).and_then(|s| s.key.as_ref()) == Some(key)
                        }
                        SessionNavTarget::Recent(recent) => recent == key,
                    })
                })
            })
            .unwrap_or(0);
        self.session_cursor = Some(position.min(order.len() - 1));
    }

    /// Move the sidebar cursor one step, clamping at the ends like pane
    /// movement. Keeps navigation mode open for repeats. A first press with
    /// no cursor lands at the nearest end instead of skipping it.
    pub(super) fn move_session_cursor(&mut self, down: bool) {
        let order = self.session_nav_order();
        if order.is_empty() {
            self.session_cursor = None;
            return;
        }
        let Some(current) = self.session_cursor else {
            self.session_cursor = Some(if down { 0 } else { order.len() - 1 });
            return;
        };
        let current = current.min(order.len() - 1);
        self.session_cursor = Some(crate::navigation::move_index(current, order.len(), down));
    }

    /// Open the cursor session for navigation-mode `Enter`. Returns true when
    /// something ran so the caller exits navigation mode.
    pub(super) fn activate_session_cursor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let order = self.session_nav_order();
        let Some(cursor) = self.session_cursor else {
            return false;
        };
        match order.get(cursor) {
            Some(SessionNavTarget::Open(id)) => {
                let id = *id;
                if self.open_sessions.contains_key(&id) {
                    self.activate_open_session(id, window, cx);
                    return true;
                }
                false
            }
            Some(SessionNavTarget::Recent(key)) => {
                let key = key.clone();
                self.open_agent_session(key, window, cx);
                true
            }
            None => false,
        }
    }

    fn render_session_row(
        &self,
        row: SessionSidebarRow,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let accent = cx.theme().info;
        let muted = cx.theme().muted_foreground;
        let id = match &row.target {
            SessionNavTarget::Open(id) => format!("open-agent-{id}"),
            SessionNavTarget::Recent(key) => format!(
                "recent-agent-{}-{}-{}",
                key.provider,
                key.store.display(),
                key.id
            ),
        };
        let close_id = format!("close-agent-{}", row.open_id.unwrap_or_default());
        let pin_id = format!("pin-{id}");
        let hover_group = SharedString::from(format!("session-card-{id}"));
        let card_focus = window
            .use_keyed_state(hover_group.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let show_actions = row.cursor || card_focus.contains_focused(window, cx);
        let has_actions = row.pin_key.is_some() || row.open_id.is_some();
        let mut metadata = h_flex()
            .items_center()
            .gap_2()
            .w_full()
            .min_w_0()
            .text_xs()
            .text_color(muted)
            .child(div().flex_1().min_w_0().truncate().child(row.provider));
        if let Some(status) = row.status {
            let color = match status {
                ActivityState::Starting | ActivityState::Working => accent,
                ActivityState::NeedsAttention => cx.theme().warning,
                ActivityState::Finished => cx.theme().success,
                _ => muted,
            };
            metadata = metadata.child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .items_center()
                    .text_color(color)
                    .child(div().size(px(5.)).rounded_full().bg(color))
                    .child(status.label()),
            );
        }
        let compact = row.repository.is_none();
        // The timestamp and actions share a fixed slot in the first row,
        // whether that row contains a repository or a compact session title.
        let time = div()
            .id(SharedString::from(format!("time-{id}")))
            .test_support()
            .flex_none()
            .w(px(if compact { 56. } else { 72. }))
            .text_right()
            .text_color(muted)
            .text_xs()
            .truncate()
            .when(has_actions, |time| {
                time.group_hover(hover_group.clone(), |time| time.invisible())
                    .when(show_actions, |time| time.invisible())
            })
            .when_some(row.age, |time, age| time.child(age));
        let title = row.title;
        let title_row = h_flex()
            .w_full()
            .min_w_0()
            .min_h(px(20.))
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_none()
                    .size(px(18.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(row.icon),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .truncate()
                    .when(row.selected, |title| title.font_semibold())
                    .child(title.clone()),
            );
        let mut content = v_flex().w_full().min_w_0().items_start().gap_1();
        if let Some(repository) = row.repository {
            content = content
                .child(
                    h_flex()
                        .w_full()
                        .min_w_0()
                        .h(px(20.))
                        .items_center()
                        .gap_2()
                        .text_xs()
                        .text_color(muted)
                        .child(div().flex_1().min_w_0().truncate().child(repository))
                        .child(time),
                )
                .child(title_row);
        } else {
            content = content.child(title_row.child(time));
        }
        content = content.child(metadata);
        let mut card = h_flex()
            .mx_2()
            .relative()
            .group(hover_group.clone())
            .track_focus(&card_focus)
            .tab_stop(false)
            .rounded_lg()
            .border_1()
            .border_color(if row.cursor {
                cx.theme().ring
            } else {
                gpui_kit::transparent_black()
            })
            .when(row.selected, |card| card.bg(accent.opacity(0.10)))
            .hover(|card| {
                card.bg(if row.selected {
                    accent.opacity(0.15)
                } else {
                    cx.theme().secondary.opacity(0.6)
                })
            })
            .child(
                Button::new(SharedString::from(id))
                    .ghost()
                    .small()
                    .flex_1()
                    .min_w_0()
                    .h_auto()
                    .px_2()
                    .py_2()
                    .tooltip(row.tooltip)
                    .accessibility_label(title.clone())
                    .child(content)
                    .on_click(cx.listener(move |this, _, window, cx| match &row.target {
                        SessionNavTarget::Open(id) => this.activate_open_session(*id, window, cx),
                        SessionNavTarget::Recent(key) => {
                            this.open_agent_session(key.clone(), window, cx)
                        }
                    })),
            );
        if row.selected {
            card = card.child(
                div()
                    .absolute()
                    .left_0()
                    .top(px(10.))
                    .bottom(px(10.))
                    .w(px(2.))
                    .rounded_full()
                    .bg(accent),
            );
        }
        let mut actions = h_flex()
            .absolute()
            .top(px(8.))
            .right(px(8.))
            .gap(px(2.))
            // Keep action hitboxes available while faded out. Do not occlude
            // the card's hover group: its timestamp must stay hidden when the
            // pointer moves onto these buttons. Click handlers stop propagation.
            .opacity(0.)
            .group_hover(hover_group, |actions| actions.opacity(1.))
            .when(show_actions, |actions| actions.opacity(1.));
        if let Some(key) = row.pin_key {
            let pinned = self.pinned_sessions.contains(&key);
            let action = if pinned {
                "Unpin session"
            } else {
                "Pin session"
            };
            actions = actions.child(
                Button::new(SharedString::from(pin_id))
                    .ghost()
                    .xsmall()
                    .icon(gpui_kit::assets::IconName::Pin)
                    .text_color(if pinned { accent } else { muted })
                    .tooltip(action)
                    .accessibility_label(format!("{action}: {title}"))
                    .disabled(self.data_root.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.toggle_session_pin(key.clone(), window, cx)
                    })),
            );
        }
        if let Some(id) = row.open_id {
            actions = actions.child(
                Button::new(SharedString::from(close_id))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .text_color(muted)
                    .tooltip("Close session")
                    .accessibility_label(format!("Close session: {title}"))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.close_agent_session(id, window, cx)
                    })),
            );
        }
        card.child(actions).into_any_element()
    }

    pub(super) fn render_agent_sessions(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let agent_index = WorkspaceTab::Agent as usize;
        let active = self.tabs[agent_index]
            .as_ref()
            .and_then(|p| p.read(cx).launch_id());
        let selected = active
            .and_then(|id| self.open_sessions.get(&id))
            .and_then(|s| s.key.as_ref());
        let activity = self.agent_activity.snapshot();
        let mut sidebar = v_flex()
            .track_focus(&self.agent_sidebar_focus)
            .focus(|style| style.border_1().border_color(cx.theme().ring))
            .h_full()
            .min_h_0()
            .flex_none()
            .w(px(280.))
            .border_r_1()
            .border_color(cx.theme().border)
            .py_2()
            .gap_2();
        let sections = self.session_sidebar_sections();
        let counts = [
            sections.open.len(),
            sections.pinned.len(),
            sections.recent.len(),
        ];
        let sections = sections.expanded(self.session_sections_expanded);
        let mut list = v_flex()
            .id("agent-session-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar();
        let mut open_content = v_flex().gap_1().pt_1();
        let mut pinned_content = v_flex().gap_1().pt_1();
        let mut recent_content = v_flex().gap_1().pt_1();
        for (open_pos, &id) in sections.open.iter().enumerate() {
            let session = &self.open_sessions[&id];
            let status = activity
                .for_launch(id)
                .map(|a| a.state)
                .unwrap_or(ActivityState::Unavailable);
            open_content = open_content.child(
                self.render_session_row(
                    SessionSidebarRow {
                        target: SessionNavTarget::Open(id),
                        title: session.title.clone(),
                        provider: session.agent.label().into(),
                        repository: Some(
                            self.session_repository_label(&session.checkout)
                                .unwrap_or_else(|| session.checkout.display().to_string()),
                        ),
                        age: Some(
                            session
                                .key
                                .as_ref()
                                .and_then(|key| {
                                    self.session_snapshot
                                        .sessions
                                        .iter()
                                        .find(|s| &s.key == key)
                                })
                                .map(|saved| saved.age())
                                .unwrap_or_else(|| {
                                    crate::relative_time::relative_duration_label(
                                        crate::relative_time::current_unix_secs()
                                            .saturating_sub(session.created_at),
                                    )
                                }),
                        ),
                        tooltip: format!(
                            "{}\n{}\n{} · {}",
                            session.title,
                            session.checkout.display(),
                            session.agent.label(),
                            status.label()
                        ),
                        icon: crate::agent_icons::agent_icon(
                            session.agent,
                            &self.agent_icon_tiles,
                            crate::agent_icons::ICON_PX,
                        ),
                        status: Some(status),
                        open_id: Some(id),
                        pin_key: session.key.clone(),
                        selected: active == Some(id),
                        cursor: self.navigation_open
                            && self.navigation_pane == 0
                            && self.session_cursor == Some(open_pos),
                    },
                    window,
                    cx,
                ),
            );
        }
        let open_count = sections.open.len();
        let pinned_count = sections.pinned.len();
        for (position, session) in sections.pinned.iter().chain(&sections.recent).enumerate() {
            let key = session.key.clone();
            let card = self.render_session_row(
                SessionSidebarRow {
                    target: SessionNavTarget::Recent(key.clone()),
                    title: session.title.clone(),
                    provider: session.provider_label().into(),
                    repository: (position < pinned_count).then(|| {
                        self.session_repository_label(&session.checkout)
                            .unwrap_or_else(|| session.checkout.display().to_string())
                    }),
                    age: Some(session.age()),
                    tooltip: format!("{}\n{}", session.tooltip(), session.checkout.display()),
                    icon: crate::agent_icons::session_icon(
                        session.agent(),
                        &self.agent_icon_tiles,
                        crate::agent_icons::ICON_PX,
                    ),
                    status: None,
                    open_id: None,
                    pin_key: Some(key.clone()),
                    selected: selected == Some(&key),
                    cursor: self.navigation_open
                        && self.navigation_pane == 0
                        && self.session_cursor == Some(open_count.saturating_add(position)),
                },
                window,
                cx,
            );
            if position < pinned_count {
                pinned_content = pinned_content.child(card);
            } else {
                recent_content = recent_content.child(card);
            }
        }
        if counts[SessionSection::Recent as usize] == 0 {
            recent_content = if !self.session_snapshot.loaded {
                recent_content.child(div().mx_2().p_3().text_sm().child("Loading sessions…"))
            } else if self.session_snapshot.errors.is_empty() {
                recent_content.child(
                    crate::empty_state::empty_state(
                        IconName::SquareTerminal,
                        if counts[0] + counts[1] > 0 {
                            "No other recent sessions"
                        } else {
                            "No recent sessions yet"
                        },
                        "Recent conversations in this repository appear here. Use New session to start one.",
                    )
                    .mx_2()
                    .p_3()
                    .flex_none(),
                )
            } else {
                recent_content
            };
        }
        for error in &self.session_snapshot.errors {
            recent_content = recent_content.child(
                div()
                    .mx_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(error.clone()),
            );
        }
        if counts[0] > 0 {
            list = list.child(self.render_session_section(
                SessionSection::Open,
                counts[0],
                false,
                open_content,
                cx,
            ));
        }
        if counts[1] > 0 {
            list = list.child(self.render_session_section(
                SessionSection::Pinned,
                counts[1],
                counts[0] > 0,
                pinned_content,
                cx,
            ));
        }
        list = list.child(self.render_session_section(
            SessionSection::Recent,
            counts[2],
            counts[0] + counts[1] > 0,
            recent_content,
            cx,
        ));
        sidebar = sidebar.child(list);
        sidebar = sidebar.child(
            div()
                .mx_2()
                .border_t_1()
                .border_color(cx.theme().border)
                .pt_2()
                .child(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("new-agent-session")
                                .small()
                                .flex_1()
                                .icon(IconName::Plus)
                                .label("New session")
                                .tooltip("Choose an agent and start a session")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.prompt_new_agent_session(window, cx)
                                })),
                        )
                        .child(
                            Button::new("refresh-agent-sessions")
                                .small()
                                .ghost()
                                .icon(IconName::RotateCw)
                                .tooltip("Refresh sessions")
                                .accessibility_label("Refresh sessions")
                                .loading(self.session_refreshing)
                                .on_click(cx.listener(|this, _, _, cx| this.refresh_sessions(cx))),
                        ),
                ),
        );
        let terminal = self.tabs[agent_index]
            .clone()
            .map(|p| p.into_any_element())
            .unwrap_or_else(|| {
                div()
                    .p_4()
                    .child("Select a session or start a new one.")
                    .into_any_element()
            });
        h_flex()
            .items_stretch()
            .size_full()
            .min_h_0()
            .child(sidebar)
            .child(div().flex_1().min_w_0().h_full().child(terminal))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn session_sidebar_accordions_hide_rows_and_keep_navigation_and_focus_in_sync(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use std::cell::RefCell;

        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().canonicalize().unwrap();
        let pinned = session(checkout.to_str().unwrap(), "pinned-fixture");
        let recent = session(checkout.to_str().unwrap(), "recent-fixture");
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new_for_test(window, cx, &checkout);
                workspace.data_root = None;
                workspace.home_visible = false;
                workspace.active_tab = WorkspaceTab::Agent;
                workspace.agent_activity = AgentActivityStore::new().0;
                let pane = cx.new(|cx| {
                    TerminalPane::test_agent(
                        workspace.agent_activity.start(&checkout, AgentKind::Codex),
                        cx,
                    )
                });
                let id = pane.read(cx).launch_id().unwrap();
                workspace.open_sessions.insert(
                    id,
                    OpenAgentSession {
                        pane: pane.clone(),
                        checkout: checkout.clone(),
                        agent: AgentKind::Codex,
                        key: None,
                        title: "open-fixture".into(),
                        created_at: 0,
                    },
                );
                workspace.tabs = WorkspaceTab::ALL
                    .into_iter()
                    .map(|tab| tab.has_terminal().then(|| pane.clone()))
                    .collect();
                workspace.editor_instance = Some(EditorChoice::Neovim);
                workspace
            });
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.session_snapshot = SessionSnapshot {
                    sessions: vec![pinned.clone(), recent.clone()],
                    loaded: true,
                    ..Default::default()
                };
                workspace.pinned_sessions = vec![pinned.key.clone()];
                cx.notify();
            });
            window.render_frame(cx);
            let open_id = *workspace.read(cx).open_sessions.keys().next().unwrap();
            let generation = workspace.read(cx).session_navigation;
            let card_id = |session: &SessionSummary| {
                format!(
                    "recent-agent-{}-{}-{}",
                    session.key.provider,
                    session.key.store.display(),
                    session.key.id
                )
            };
            let targets = [
                SessionNavTarget::Open(open_id),
                SessionNavTarget::Recent(pinned.key.clone()),
                SessionNavTarget::Recent(recent.key.clone()),
            ];
            assert_eq!(workspace.read(cx).session_nav_order(), targets);
            for (index, (header, card)) in [
                ("toggle-open-sessions", format!("open-agent-{open_id}")),
                ("toggle-pinned-sessions", card_id(&pinned)),
                ("toggle-recent-sessions", card_id(&recent)),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(window.find(header).expanded(), Some(true));
                window.click(header, cx);
                assert_eq!(window.find(header).expanded(), Some(false));
                assert!(window.try_find(card.clone()).is_none());
                let remaining = workspace.read(cx).session_nav_order();
                assert_eq!(remaining, targets[index + 1..]);
                assert!(workspace.read(cx).session_section_focus[index].is_focused(window));
                assert_eq!(workspace.read(cx).session_navigation, generation);
                assert_eq!(workspace.read(cx).open_sessions.len(), 1);
            }
            assert!(workspace.read(cx).session_nav_order().is_empty());
            window.press("space", cx);
            assert_eq!(window.find("toggle-recent-sessions").expanded(), Some(true));
            assert!(window.try_find(card_id(&recent)).is_some());
            assert!(window.try_find(card_id(&pinned)).is_none());
            // Navigation-mode Enter on a focused header must toggle that
            // header rather than activate a stale session cursor.
            workspace.update(cx, |workspace, cx| {
                workspace.navigation_open = true;
                workspace.navigation_pane = 0;
                workspace.session_cursor = Some(0);
                cx.notify();
            });
            window.render_frame(cx);
            window.press("enter", cx);
            assert_eq!(
                window.find("toggle-recent-sessions").expanded(),
                Some(false)
            );
            assert_eq!(workspace.read(cx).session_navigation, generation);
            assert!(!workspace.read(cx).navigation_open);
            assert!(workspace.read(cx).session_cursor.is_none());
            window.press("enter", cx);
            assert_eq!(window.find("toggle-recent-sessions").expanded(), Some(true));
            assert_eq!(workspace.read(cx).session_nav_order(), targets[2..]);
            window.click("toggle-recent-sessions", cx);
            window.click("toggle-open-sessions", cx);
            workspace.update(cx, |workspace, cx| {
                workspace.navigation_open = true;
                workspace.navigation_pane = 0;
                workspace.session_cursor = Some(0);
                cx.notify();
            });
            window.render_frame(cx);
            window.press("j", cx);
            assert!(workspace.read(cx).agent_sidebar_focus.is_focused(window));
            assert_eq!(workspace.read(cx).session_cursor, Some(0));
            window.press("enter", cx);
            assert_eq!(window.find("toggle-open-sessions").expanded(), Some(true));
            assert_ne!(workspace.read(cx).session_navigation, generation);
            assert_eq!(workspace.read(cx).open_sessions.len(), 1);
        });
    }

    #[gpui_kit::test]
    fn session_card_pin_hover_and_activation_do_not_open_the_session(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::component::Root;
        use gpui_kit::test::TestWindowExt as _;
        use std::cell::RefCell;

        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().canonicalize().unwrap();
        let data_root = DataRoot::new(checkout.join("data"));
        std::fs::create_dir_all(data_root.root()).unwrap();
        let saved = session(checkout.to_str().unwrap(), "card-actions");
        let card_id = format!(
            "recent-agent-{}-{}-{}",
            saved.key.provider,
            saved.key.store.display(),
            saved.key.id,
        );
        let pin_id = format!("pin-{card_id}");
        let time_id = format!("time-{card_id}");
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new_for_test(window, cx, &checkout);
                workspace.data_root = Some(data_root.clone());
                workspace.home_visible = false;
                workspace.active_tab = WorkspaceTab::Agent;
                workspace.pinned_sessions.clear();
                workspace.current_repository = Some("fixture".into());
                workspace.session_snapshot = SessionSnapshot {
                    sessions: vec![saved.clone()],
                    loaded: true,
                    ..Default::default()
                };
                workspace
            });
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        let generation = workspace.read_with(test_cx, |workspace, _| workspace.session_navigation);
        // Activation by target bounds also models an accessibility press
        // without a preceding pointer hover over the card.
        for expected in [vec![saved.key.clone()], vec![]] {
            test_cx.update(|window, cx| {
                // Startup catalog tasks may publish before this update;
                // install the fixture immediately before the tested frame.
                workspace.update(cx, |workspace, cx| {
                    workspace.session_snapshot = SessionSnapshot {
                        sessions: vec![saved.clone()],
                        loaded: true,
                        ..Default::default()
                    };
                    cx.notify();
                });
                window.render_frame(cx);
                window.hover("new-agent-session", cx);
                assert!(window.find(time_id.clone()).visible());
                window.hover(card_id.clone(), cx);
                assert!(!window.find(time_id.clone()).visible());
                // The action is over the card. Hovering it must keep the
                // timestamp hidden for both Recent and Pinned sessions.
                window.hover(pin_id.clone(), cx);
                assert!(!window.find(time_id.clone()).visible());
                window.hover("new-agent-session", cx);
                assert!(window.find(time_id.clone()).visible());
                // Activate by target bounds without a preceding card hover.
                window.click(pin_id.clone(), cx);
            });
            workspace.read_with(test_cx, |workspace, _| {
                assert_eq!(workspace.pinned_sessions, expected);
                assert_eq!(workspace.session_navigation, generation);
                assert_eq!(workspace.current_repository.as_deref(), Some("fixture"));
                assert!(workspace.open_sessions.is_empty());
            });
            assert_eq!(
                DeviceStore::new(&data_root).load().unwrap().pinned_sessions,
                expected
            );
        }
    }

    #[gpui_kit::test]
    fn session_sidebar_live_activation_restores_exact_pane_and_cancels_pending_resume(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        // Retain unstarted panes in every terminal slot so workspace entry
        // never launches shells or real harnesses during the regression.
        let first = directory.path().join("first");
        let other = directory.path().join("other");
        let (_, test_cx) =
            cx.add_window_view(|window, cx| Workspace::new_for_test(window, cx, &first));
        test_cx.update(|window, cx| {
            let workspace = window.root::<Workspace>().unwrap().unwrap();
            workspace.update(cx, |workspace, cx| {
                workspace.data_root = None;
                workspace.agent_activity = AgentActivityStore::new().0;
                workspace.session_sections_expanded = [false, true, false];
                let pane = |checkout: &Path, cx: &mut Context<Workspace>| {
                    cx.new(|cx| {
                        TerminalPane::test_agent(
                            workspace.agent_activity.start(checkout, AgentKind::Codex),
                            cx,
                        )
                    })
                };
                let first_pane = pane(&first, cx);
                let other_pane = pane(&other, cx);
                let first_id = first_pane.read(cx).launch_id().unwrap();
                let other_id = other_pane.read(cx).launch_id().unwrap();
                for (id, pane, checkout) in [
                    (first_id, first_pane.clone(), first.clone()),
                    (other_id, other_pane.clone(), other.clone()),
                ] {
                    workspace.open_sessions.insert(
                        id,
                        OpenAgentSession {
                            pane,
                            checkout,
                            agent: AgentKind::Codex,
                            key: None,
                            title: "fixture".into(),
                            created_at: 0,
                        },
                    );
                }
                let retained_tabs = |pane: &Entity<TerminalPane>| {
                    WorkspaceTab::ALL
                        .into_iter()
                        .map(|tab| tab.has_terminal().then(|| pane.clone()))
                        .collect()
                };
                workspace.tabs = retained_tabs(&first_pane);
                workspace.editor_instance = Some(EditorChoice::Neovim);
                workspace.inactive_repositories.insert(
                    other.clone(),
                    RepositoryTabs {
                        git_view: None,
                        active_tab: WorkspaceTab::Agent,
                        tabs: retained_tabs(&other_pane),
                        editor_instance: Some(EditorChoice::Neovim),
                        native_editor: None,
                        review: Workspace::new_review(&other, cx),
                        agent: AgentKind::Codex,
                    },
                );
                let pending_resume = workspace.session_navigation;
                workspace.activate_open_session(other_id, window, cx);
                assert_eq!(workspace.session_sections_expanded, [false, true, false]);
                assert_ne!(workspace.session_navigation, pending_resume);
                assert_eq!(workspace.working_directory, other);
                assert_eq!(
                    workspace.tabs[WorkspaceTab::Agent as usize],
                    Some(other_pane.clone())
                );
                assert_eq!(
                    workspace.inactive_repositories[&first].tabs[WorkspaceTab::Agent as usize],
                    Some(first_pane.clone())
                );
                assert_eq!(workspace.open_sessions.len(), 2);
                workspace.activate_open_session(first_id, window, cx);
                assert_eq!(workspace.session_sections_expanded, [false, true, false]);
                assert_eq!(workspace.working_directory, first);
                assert_eq!(
                    workspace.tabs[WorkspaceTab::Agent as usize],
                    Some(first_pane)
                );
                assert_eq!(
                    workspace.inactive_repositories[&other].tabs[WorkspaceTab::Agent as usize],
                    Some(other_pane)
                );
                assert_eq!(workspace.open_sessions.len(), 2);
            });
        });
    }

    fn session(checkout: &str, id: &str) -> SessionSummary {
        SessionSummary {
            key: SessionKey {
                provider: "codex".into(),
                store: "/store".into(),
                id: id.into(),
            },
            cwd: checkout.into(),
            checkout: checkout.into(),
            title: id.into(),
            updated: 1,
            timestamp_source: "test".into(),
        }
    }

    #[test]
    fn session_sidebar_filters_global_sections_by_space_and_keeps_recent_local() {
        let current = session("/personal/current", "current");
        let other = session("/personal/other", "other");
        let work = session("/work/project", "work");
        let personal_pin = session("/personal/other", "personal-pin");
        let work_pin = session("/work/project", "work-pin");
        let recent = session("/personal/current", "recent");
        let snapshot = SessionSnapshot {
            sessions: vec![
                current.clone(),
                other.clone(),
                work.clone(),
                personal_pin.clone(),
                work_pin.clone(),
                recent.clone(),
            ],
            loaded: true,
            ..Default::default()
        };
        let open = [
            (1, current.checkout.as_path(), Some(&current.key)),
            (2, work.checkout.as_path(), Some(&work.key)),
            (3, other.checkout.as_path(), Some(&other.key)),
            // New sessions without a catalog identity remain reachable.
            (4, Path::new("/personal/other"), None),
        ];
        let pins = vec![work_pin.key.clone(), personal_pin.key.clone()];
        let project = |selection: SpaceSelection| {
            SessionSidebarSections::project(
                open.into_iter(),
                &snapshot,
                &pins,
                &current.checkout,
                25,
                |path| {
                    selection.matches(if path.starts_with("/work") {
                        "Work"
                    } else {
                        "Personal"
                    })
                },
            )
        };
        let personal = project(SpaceSelection::Named("Personal".into()));
        assert_eq!(personal.open, vec![4, 3, 1]);
        assert_eq!(personal.pinned, vec![personal_pin.clone()]);
        assert_eq!(personal.recent, vec![recent.clone()]);
        assert_eq!(
            personal.nav_order(),
            vec![
                SessionNavTarget::Open(4),
                SessionNavTarget::Open(3),
                SessionNavTarget::Open(1),
                SessionNavTarget::Recent(personal_pin.key.clone()),
                SessionNavTarget::Recent(recent.key.clone())
            ]
        );
        let work_only = project(SpaceSelection::Named("Work".into()));
        assert_eq!(work_only.open, vec![2]);
        assert_eq!(work_only.pinned, vec![work_pin.clone()]);
        assert_eq!(work_only.recent, vec![recent]);
        let all = project(SpaceSelection::All);
        assert_eq!(all.open, vec![4, 3, 2, 1]);
        assert_eq!(all.pinned, vec![work_pin, personal_pin]);
    }

    #[test]
    fn session_sidebar_pins_outlive_open_sessions_and_do_not_consume_recent_limit() {
        let pinned = session("/repo", "pinned");
        let recent = session("/repo", "recent");
        let older = session("/repo", "older");
        let mut different_store = session("/repo", "pinned");
        different_store.key.store = "/other-store".into();
        let snapshot = SessionSnapshot {
            sessions: vec![
                pinned.clone(),
                recent.clone(),
                older.clone(),
                different_store.clone(),
            ],
            loaded: true,
            ..Default::default()
        };
        let pins = vec![pinned.key.clone(), pinned.key.clone()];
        let open = [(5, pinned.checkout.as_path(), Some(&pinned.key))];
        let active = SessionSidebarSections::project(
            open.into_iter(),
            &snapshot,
            &pins,
            &pinned.checkout,
            2,
            |_| true,
        );
        assert_eq!(active.open, vec![5]);
        assert!(active.pinned.is_empty());
        assert_eq!(active.recent, vec![recent.clone(), older.clone()]);
        let closed = SessionSidebarSections::project(
            std::iter::empty(),
            &snapshot,
            &pins,
            &pinned.checkout,
            3,
            |_| true,
        );
        assert!(closed.open.is_empty());
        assert_eq!(closed.pinned, vec![pinned.clone()]);
        assert_eq!(closed.recent, vec![recent.clone(), older, different_store]);
        let unpinned = SessionSidebarSections::project(
            std::iter::empty(),
            &snapshot,
            &[],
            &pinned.checkout,
            2,
            |_| true,
        );
        assert!(unpinned.pinned.is_empty());
        assert_eq!(unpinned.recent, vec![pinned, recent]);
    }

    #[test]
    fn session_sidebar_does_not_restore_pins_missing_from_account_filtered_catalog() {
        let available = session("/repo", "available");
        let unavailable = session("/repo", "unavailable");
        let mut snapshot = SessionSnapshot {
            sessions: vec![available.clone()],
            ..Default::default()
        };
        let pins = vec![unavailable.key.clone()];
        let sections = SessionSidebarSections::project(
            std::iter::empty(),
            &snapshot,
            &pins,
            &available.checkout,
            1,
            |_| true,
        );
        assert!(sections.pinned.is_empty());
        assert_eq!(sections.recent, vec![available.clone()]);
        snapshot.sessions.push(unavailable.clone());
        let recovered = SessionSidebarSections::project(
            std::iter::empty(),
            &snapshot,
            &pins,
            &available.checkout,
            1,
            |_| true,
        );
        assert_eq!(recovered.pinned, vec![unavailable]);
        assert_eq!(recovered.recent, vec![available]);
    }
}
