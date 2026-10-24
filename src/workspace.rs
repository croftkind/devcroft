//! The workspace shell: tab definitions plus the surrounding chrome
//! (project header and tab bar) hosting the active terminal pane.

#[path = "workspace_sessions.rs"]
mod sessions;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use gpui_kit::component::{
    ActiveTheme as _, ColorName, Icon, IconName, IndexPath, Sizable, Size, StyledExt as _,
    WindowExt as _,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex,
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
    spinner::Spinner,
    tab::{Tab, TabBar},
    tag::Tag,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, DismissEvent, Entity, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, KeyUpEvent, KeystrokeEvent, ModifiersChangedEvent,
    MouseButton, ParentElement, Render, SharedString, Styled, Subscription, Window, deferred, div,
    px, rgb,
};

use crate::add_repository::AddRepositoryView;
use crate::agent::AgentKind;
use crate::agent_activity::{ActivityState, AgentActivityStore};
use crate::agent_icons::{self, AgentIconTiles};
use crate::agent_sessions::{
    Catalog, DEFAULT_SIDEBAR_LIMIT, HOME_LIMIT, SessionKey, SessionSummary,
    Snapshot as SessionSnapshot,
};
use crate::command_palette::{
    GoToAgent, GoToEditor, GoToResources, GoToReview, GoToTerminal, NewAgentSession,
    PaletteCommand, PaletteItem, PaletteMode, PaletteSection, ToggleActionsPalette,
    ToggleProjectsPalette, is_primary_modifier, is_quit_shortcut, item_at,
    palette_mode_for_shortcut, palette_sections_for_mode, query_has_terms,
};
use crate::data::{
    DataRoot, DeviceStore, RecentRepository, SpaceSelection, Spaces, SyncStatus, SyncTracker,
    checkout_for, ensure_spaces, recent_repositories, record_repository_open, resolve_current_key,
    sync_portable_with_tracker,
};
use crate::editor::{EditorChoice, ExternalEditorKind, native::NativeEditor};
use crate::git::{
    BranchPickerAction, BranchPickerRequested, CommitDraftChanged, GitMode, GitView, OpenGitFile,
    SnapshotChanged,
};
use crate::git_status::{GitStatus, load_git_status};
use crate::home::{HomeEvent, HomeView, project_state_tag};
use crate::metrics::{DEFAULT_APP_FONT_SIZE, WORKSPACE_HEADER_HEIGHT};
use crate::navigation::{
    self, Command as NavigationCommand, Context as NavigationContext,
    Decision as NavigationDecision, Input as NavigationInput,
};
use crate::pane::TerminalPane;
use crate::review::ReviewView;
use crate::settings::SettingsView;
use crate::tools::{ToolKind, ToolView};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::dialog::{Confirm, DialogFooter};
use gpui_kit::component::scroll::ScrollableElement as _;

/// How often the header re-reads branch/dirty/ahead-behind state.
///
/// The poll runs off the main thread and only notifies on change, so the
/// cadence sets staleness, not frame cost. Two seconds keeps the dirty dot
/// feeling live after saves without churning full worktree walks on large
/// checkouts.
const GIT_POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How often the automatic-sync scheduler re-reads the persisted interval.
///
/// The schedule itself (`device.json` `sync_interval_minutes`) is checked on
/// this cadence, so Settings edits re-arm within one tick and turning the
/// schedule off takes effect promptly. Ten seconds keeps a 2-minute minimum
/// interval reasonably tight while each tick costs one small JSON read.
const AUTO_SYNC_POLL: Duration = Duration::from_secs(10);

/// Key context of the workspace subtree. `bind_app_keys` (see `main.rs`)
/// unbinds Tab/Shift+Tab here, so normal-mode focus traversal stays out of
/// the focused component's way — a terminal sends both keys to the pty
/// (agent harnesses switch agents/models/modes with them) instead of moving
/// focus. Component traversal lives in navigation mode (see
/// `move_navigation_component_focus`).
pub(crate) const WORKSPACE_KEY_CONTEXT: &str = "Workspace";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkspaceTab {
    Agent,
    Editor,
    Terminal,
    Review,
    Resources,
}

impl WorkspaceTab {
    pub(crate) const ALL: [Self; 5] = [
        Self::Agent,
        Self::Editor,
        Self::Terminal,
        Self::Review,
        Self::Resources,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Agent => "Agent",
            Self::Editor => "Editor",
            Self::Terminal => "Terminal",
            Self::Review => "Review",
            Self::Resources => "Resources",
        }
    }

    pub(crate) fn command(self) -> Option<&'static str> {
        match self {
            // Single source of truth lives on the default harness, so the
            // label in Settings and the spawned command cannot drift.
            Self::Agent => Some(AgentKind::DEFAULT.command()),
            Self::Editor => Some("nvim ."),
            Self::Terminal | Self::Review | Self::Resources => None,
        }
    }

    /// Native pages render content instead of hosting a shell.
    pub(crate) fn has_terminal(self) -> bool {
        !matches!(self, Self::Review | Self::Resources)
    }
}

/// Where the global artifact browser should return to. Captured on
/// entry so the titlebar back button behaves like a browser back button
/// instead of always landing on Home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PageOrigin {
    Home,
    Repository,
}

/// The Git page temporarily replaces the current view without selecting a tab.
struct GitPageOrigin {
    home_visible: bool,
    relationships_visible: bool,
    focus: Option<FocusHandle>,
}

/// Generation-guarded holder for the header's git status. Background loads
/// snapshot a load id up front and only commit while still current, so a
/// slow load for a previous checkout can never overwrite newer state —
/// e.g. the tick in flight when the user switches repositories. Same
/// pattern as `ReviewView`'s `generation`, factored out so the guard
/// itself is unit-testable without a window.
#[derive(Debug, Default)]
struct GitPoll {
    generation: u64,
    status: GitStatus,
}

impl GitPoll {
    fn status(&self) -> &GitStatus {
        &self.status
    }

    /// Claim the next load id and snapshot the directory it must read.
    /// Pure; the caller runs the load and offers the result back to
    /// [`commit`](Self::commit).
    fn begin_check(&mut self, workdir: PathBuf) -> (u64, PathBuf) {
        self.generation += 1;
        (self.generation, workdir)
    }

    /// Offer a loaded status: applies only when `generation` is still the
    /// latest check. Returns whether anything changed, so callers notify
    /// only then.
    fn commit(&mut self, generation: u64, status: GitStatus) -> bool {
        if self.generation != generation || self.status == status {
            return false;
        }
        self.status = status;
        true
    }

    /// Forget the current status (e.g. right after a switch, before the
    /// fresh load lands) without touching the generation: the pending
    /// fresh load still counts as current.
    fn reset(&mut self) {
        self.status = GitStatus::default();
    }
}

fn visible_git_status<'a>(
    page_status: Option<&'a GitStatus>,
    polled_status: &'a GitStatus,
) -> &'a GitStatus {
    page_status.unwrap_or(polled_status)
}

pub(crate) struct Workspace {
    resources: Entity<crate::artifacts::ArtifactBrowser>,
    home: Entity<HomeView>,
    relationships: Entity<crate::repository_graph::GraphPage>,
    relationships_visible: bool,
    relationships_origin: Option<PageOrigin>,
    home_visible: bool,
    /// Isolation profiles and the active one. `active_space` is machine-local
    /// (`device.json`) and drives every space-filtered surface; the catalog
    /// is portable and syncs with the records it scopes.
    spaces: Spaces,
    active_space: SpaceSelection,
    /// The titlebar space switcher's controlled popover plus the menu it
    /// shows, built per opening and owned here (never window-keyed state).
    /// The focus handle is captured at build time: closing must not read the
    /// menu entity, which is leased while its own click/dismiss handler runs.
    space_menu_open: bool,
    space_menu: Option<Entity<PopupMenu>>,
    space_menu_focus: Option<FocusHandle>,
    portable_git_poll: GitPoll,
    active_tab: WorkspaceTab,
    tabs: Vec<Option<Entity<TerminalPane>>>,
    editor_instance: Option<EditorChoice>,
    editor_preference: EditorChoice,
    external_editors: HashMap<String, bool>,
    native_editor: Option<Entity<NativeEditor>>,
    review: Entity<ReviewView>,
    /// Harness the visible Agent pane was spawned with. Preserved across
    /// repository switches with its tabs; selecting a historical session
    /// or starting a new one updates it.
    session_agent: AgentKind,
    /// Global default harness (Settings > Agent): what fresh Agent panes
    /// launch and what the New-session picker pre-selects. Loaded from
    /// `device.json` at startup and pushed live by Settings; open sessions
    /// keep running with the harness they started with. Always one of
    /// [`Self::enabled_agents`].
    default_agent: AgentKind,
    /// Enabled harnesses (Settings > Agent). Fresh Agent panes and the
    /// New-session picker only offer these; open sessions keep running even
    /// if their harness is later disabled. Never empty.
    enabled_agents: Vec<AgentKind>,
    /// Strong entity handles keep hidden PTYs and their output tasks alive.
    inactive_repositories: HashMap<PathBuf, RepositoryTabs>,
    settings: Entity<SettingsView>,
    onboarding: Option<Entity<crate::onboarding::OnboardingView>>,
    onboarding_reference: Option<crate::markdown_references::Reference>,
    /// Tool dialogs opened from the command bar's Tools section: one
    /// long-lived view per tool, created on first open. Reopening shows
    /// exactly what was left there, and a close-time save flush can still
    /// reach the view after the dialog layer drops it.
    tool_views: HashMap<ToolKind, Entity<ToolView>>,
    project_name: SharedString,
    git_poll: GitPoll,
    /// The shared Git view owns the header projection while its snapshot loads.
    /// Views are retained with checkout tabs so drafts and operations survive navigation.
    git_page_status: Option<GitStatus>,
    git_view: Option<Entity<GitView>>,
    git_page_origin: Option<GitPageOrigin>,
    git_header_busy: bool,
    git_header_busy_focus: FocusHandle,
    git_tab_focus: [FocusHandle; 2],
    git_header_return_focus: Option<FocusHandle>,
    git_picker_origin_focus: Option<FocusHandle>,
    git_header_notice: Option<String>,
    git_commit_drafts: HashMap<PathBuf, String>,
    command_open: bool,
    /// Pane focus before the palette takes it, for page routes with Back.
    command_origin_focus: Option<FocusHandle>,
    /// Which filtered view the open bar shows. Set on every opening (and on
    /// mode switches while open); the render model follows it, so confirmations
    /// always resolve against the visible rows.
    palette_mode: PaletteMode,
    /// Whether the open palette holds a non-blank query. The empty actions
    /// view omits the workspace tab jumps (they duplicate the tab bar and
    /// navigation-mode keys); typing reveals them while keeping every other
    /// row. Reset on every opening and mode switch alongside the query itself.
    palette_has_query: bool,
    /// The attention indicator reuses Projects mode interaction while
    /// projecting only blocked agent rows and omitting Add repository.
    attention_only: bool,
    command_state: Entity<CommandState>,
    /// One provider-neutral projection for every managed Agent pane retained
    /// by this window, including panes in background repositories.
    agent_activity: AgentActivityStore,
    session_catalog: Catalog,
    session_snapshot: SessionSnapshot,
    session_projects: Vec<RecentRepository>,
    session_refreshing: bool,
    session_navigation: u64,
    /// Recent-session rows per repository in the Agent sidebar. Loaded from
    /// `device.json` at startup and pushed live by Settings > Agent; the
    /// sidebar re-projects the in-memory snapshot on change, no rescan.
    session_limit: usize,
    open_sessions: HashMap<u64, OpenAgentSession>,
    /// Checkouts where the Agent pane has been started (fresh or resumed).
    /// Entering a checkout auto-resumes its most recent catalog session
    /// once; afterwards the user owns the pane (New session, history
    /// clicks, close), so later entries never retro-start.
    agent_autostart: HashSet<PathBuf>,
    data_root: Option<DataRoot>,
    sync_tracker: SyncTracker,
    /// When the last sync run started (manual or automatic). The automatic
    /// scheduler measures the interval from here, so manual runs reset the
    /// clock and a just-opened window waits a full interval first.
    last_sync_started: Instant,
    /// Checkout the panes and Review are rooted at. The git poll loop
    /// re-reads this every tick, so switching repositories re-roots status
    /// without restarting the loop.
    working_directory: PathBuf,
    /// Repository key matching [`working_directory`](Self::working_directory),
    /// when the checkout was opened through a linked repository.
    current_repository: Option<String>,
    /// Palette switcher cache, refreshed on every palette opening (and
    /// after each switch) so render never touches the filesystem.
    recent_repositories: Vec<RecentRepository>,
    /// Sections installed by the latest palette render; confirmations
    /// resolve against exactly this model.
    palette_model: Vec<PaletteSection>,
    /// Origin captured when entering the artifact browser, so the titlebar
    /// back button returns where the user came from. `Some` only while the
    /// artifact page is visible.
    artifacts_origin: Option<PageOrigin>,
    /// The which-key style navigation overlay. It deliberately owns no focus:
    /// focus remains on real panes so `h`/`l` can move it and Enter can leave
    /// the selected pane active after the overlay disappears.
    navigation_open: bool,
    navigation_pane: usize,
    /// Cursor for navigation-mode `j`/`k` through the Agent sessions sidebar
    /// in display order (open sessions, then recent). `None` outside the
    /// sidebar or outside navigation mode; `Enter` opens the cursor session
    /// once and exits the mode, `Escape` exits without opening.
    session_cursor: Option<usize>,
    /// Keys consumed by the navigation interceptor and held until release so
    /// physical repeats cannot toggle twice or leak into a newly focused
    /// terminal. The flag per key records whether any modifier moved since
    /// the claim: a later modifier press proves the previous hold ended, so
    /// a re-pressed *trigger* is honored as fresh even if its key-up was
    /// lost. Re-arm is trigger-only — a marked non-trigger key stays
    /// swallowed, since a missed press beats an accidental edit — and a
    /// trigger repeat with no modifier movement in between stays swallowed,
    /// which is what keeps a held toggle from flickering.
    navigation_claimed_keys: HashMap<String, bool>,
    agent_sidebar_focus: FocusHandle,
    /// Full-color harness logos for the Agent sidebar, the new-session
    /// picker, and session rows. The harness set is fixed, so tiles are
    /// rasterized once at startup and shared by every render.
    agent_icon_tiles: Rc<AgentIconTiles>,
    /// Kept for the workspace lifetime so the app-level pre-keymap hook is
    /// released with this window rather than leaking into later workspaces.
    _navigation_interceptor: Subscription,
}

struct OpenAgentSession {
    pane: Entity<TerminalPane>,
    checkout: PathBuf,
    agent: AgentKind,
    key: Option<SessionKey>,
    title: String,
    /// Unix seconds when this pane was tracked. Bounds the recency fallback
    /// for harnesses without an authoritative identity signal (Codex): only
    /// catalog sessions updated after the pane existed can be its own.
    created_at: i64,
}

struct RepositoryTabs {
    git_view: Option<Entity<GitView>>,
    active_tab: WorkspaceTab,
    tabs: Vec<Option<Entity<TerminalPane>>>,
    editor_instance: Option<EditorChoice>,
    native_editor: Option<Entity<NativeEditor>>,
    review: Entity<ReviewView>,
    /// Harness its Agent pane was spawned with, kept so a switch back
    /// restores the running session instead of respawning.
    agent: AgentKind,
}

impl RepositoryTabs {
    fn new(
        working_directory: &Path,
        agent: AgentKind,
        activity: &AgentActivityStore,
        editor: EditorChoice,
        cx: &mut Context<Workspace>,
    ) -> Self {
        let tabs = WorkspaceTab::ALL
            .into_iter()
            .map(|tab| {
                (tab.has_terminal()
                    && tab != WorkspaceTab::Agent
                    && (tab != WorkspaceTab::Editor || editor == EditorChoice::Neovim))
                    .then(|| {
                        cx.new(|cx| TerminalPane::new(tab, working_directory, agent, activity, cx))
                    })
            })
            .collect();
        let review = Workspace::new_review(working_directory, cx);
        Self {
            active_tab: WorkspaceTab::Agent,
            git_view: None,
            tabs,
            editor_instance: Some(editor),
            native_editor: None,
            review,
            agent,
        }
    }
}

/// Resolve aliases to the same checkout without making startup depend on I/O success.
fn checkout_identity(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

impl Workspace {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        working_directory: &Path,
    ) -> Self {
        let project_name = working_directory
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned();

        let working_directory = &checkout_identity(working_directory);
        let data_root = crate::data::ensure_ready(None).ok();
        // Global default harness (Settings > Agent). Fresh Agent panes
        // start with it; returning to a checkout resumes its most recent
        // session instead. Later edits come through `set_default_agent`.
        // The default always stays within the enabled set; later edits come
        // through `set_enabled_agents` / `set_default_agent`.
        let stored = data_root
            .as_ref()
            .and_then(|root| DeviceStore::new(root).load().ok());
        let editor_preference = stored
            .as_ref()
            .map(|state| state.editor_choice_or_default())
            .unwrap_or_default();
        let external_editors = stored
            .as_ref()
            .and_then(|state| state.external_editors.clone())
            .unwrap_or_default();
        // Isolation profiles: seed/reconcile the portable catalog (Personal
        // and Work plus any legacy group names the records reference) and
        // resume this device's last active space. A catalog that cannot be
        // read falls back to the seeded pair so Home still filters sanely.
        let spaces = data_root
            .as_ref()
            .and_then(|root| ensure_spaces(root).ok())
            .unwrap_or_else(|| {
                let mut catalog = Spaces::default();
                for name in crate::data::spaces::DEFAULT_SEED {
                    catalog.ensure_name(name);
                }
                catalog
            });
        let active_space = spaces.selection(
            stored
                .as_ref()
                .and_then(|state| state.active_space.as_deref()),
            stored
                .as_ref()
                .and_then(|state| state.all_spaces)
                .unwrap_or(false),
        );
        let mut enabled_agents = stored
            .as_ref()
            .map(|state| state.enabled_agents_or_default())
            .unwrap_or_else(|| AgentKind::ALL.to_vec());
        if enabled_agents.is_empty() {
            enabled_agents = AgentKind::ALL.to_vec();
        }
        let mut default_agent = stored
            .as_ref()
            .map(|state| state.default_agent_or_default())
            .unwrap_or(AgentKind::DEFAULT);
        if !enabled_agents.contains(&default_agent) {
            default_agent = enabled_agents
                .first()
                .copied()
                .unwrap_or(AgentKind::DEFAULT);
        }
        // Home must not launch an agent or editor in the startup directory.
        // Panes are created only when the user enters a repository workspace.
        let active_tab = WorkspaceTab::Agent;
        let tabs = vec![None; WorkspaceTab::ALL.len()];
        let review = Workspace::new_review(working_directory, cx);
        let session_agent = default_agent;
        let command_state = cx.new(|cx| CommandState::new(window, cx));
        // Interceptors run before GPUI resolves key bindings (unlike element
        // capture handlers). This is essential while the HUD is open: Escape,
        // Enter, arrows, and native-input bindings must not act before the
        // navigation mode consumes them.
        let interceptor_workspace = cx.entity().downgrade();
        let interceptor_window = window.window_handle();
        let navigation_interceptor = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() != interceptor_window {
                return;
            }
            let _ = interceptor_workspace.update(cx, |workspace, cx| {
                workspace.on_navigation_keystroke(event, window, cx);
            });
        });
        let (agent_activity, activity_updates) =
            AgentActivityStore::with_data_root(data_root.clone());
        cx.spawn(async move |this, cx| {
            while activity_updates.recv().await.is_ok() {
                while activity_updates.try_recv().is_ok() {}
                if this
                    .update(cx, |this, cx| {
                        this.publish_sessions(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.close_navigation(cx);
                this.navigation_claimed_keys.clear();
            }
            this.sync_activity_visibility(window, cx);
            cx.notify();
        })
        .detach();
        // Poll branch/dirty/ahead-behind off the main thread; the header
        // repaints only when the snapshot actually changes. The first
        // iteration loads immediately (so no worktree I/O blocks window
        // open), then repeats on the interval. The working directory is
        // re-read every tick so repository switches re-root the poll
        // without restarting the loop; each tick claims a load id so a
        // still-running load for a previous checkout can never overwrite
        // newer state (see `GitPoll`). The loop ends with the entity:
        // `update` fails once the workspace is dropped.
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let (generation, path) =
                        this.git_poll.begin_check(this.working_directory.clone());
                    Some((generation, path))
                });
                let Ok(Some((generation, path))) = next else {
                    break;
                };
                let status = cx
                    .background_spawn(async move { load_git_status(&path) })
                    .await;
                let dropped = this
                    .update(cx, |this, cx| {
                        if this.git_poll.commit(generation, status) {
                            cx.notify();
                        }
                        if this.git_page_origin.is_none()
                            && !this.home_visible
                            && !this.relationships_visible
                            && this.active_tab == WorkspaceTab::Review
                        {
                            this.review
                                .update(cx, |view, cx| view.refresh_if_changed(cx));
                        }
                    })
                    .is_err();
                if dropped {
                    break;
                }
                cx.background_executor().timer(GIT_POLL_INTERVAL).await;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                let next = this.update(cx, |this, _| {
                    let root = this.data_root.as_ref()?;
                    Some(this.portable_git_poll.begin_check(root.portable_dir()))
                });
                let Ok(Some((generation, path))) = next else {
                    break;
                };
                let status = cx
                    .background_spawn(async move { load_git_status(&path) })
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.portable_git_poll.commit(generation, status) {
                            if this.git_page_origin.is_none()
                                && !this.home_visible
                                && this.active_tab == WorkspaceTab::Resources
                            {
                                this.resources.update(cx, |view, cx| view.refresh(cx));
                            }
                            this.home.update(cx, |view, cx| view.refresh_artifacts(cx));
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                cx.background_executor().timer(GIT_POLL_INTERVAL).await;
            }
        })
        .detach();
        // Automatic portable sync: re-read the persisted interval on a short
        // tick and run a silent sync once it elapses since the last run
        // started. Skips while a sync is already in flight (join, don't
        // queue) and re-arms within one tick when the interval changes or is
        // turned off. Silent on purpose: status is visible in Settings >
        // Sync, and only manual runs push notifications. The loop ends with
        // the entity: `update` fails once the workspace is dropped.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AUTO_SYNC_POLL).await;
                let due = this.update(cx, |this, _| {
                    let minutes = this.auto_sync_interval_minutes()?;
                    (minutes > 0
                        && this.last_sync_started.elapsed()
                            >= Duration::from_secs(minutes.saturating_mul(60)))
                    .then_some(())
                });
                match due {
                    Ok(Some(())) => {
                        let alive = this
                            .update(cx, |this, cx| {
                                if this.sync_tracker.status() == SyncStatus::Syncing {
                                    return;
                                }
                                if this.data_root.is_none() {
                                    return;
                                }
                                this.trigger_sync(cx);
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
        })
        .detach();
        // Initial font size for Settings. The live global was already set
        // from the same store at startup (`main`), so this is just the
        // view's starting copy; later edits write back through the store.
        let initial_font_size = data_root
            .as_ref()
            .and_then(|root| DeviceStore::new(root).load().ok())
            .map(|state| state.app_font_size_or_default())
            .unwrap_or(DEFAULT_APP_FONT_SIZE);
        // Initial Agent sidebar limit, mirroring the font size above. Later
        // edits come through `set_session_limit` (Settings > Agent).
        let session_limit = data_root
            .as_ref()
            .and_then(|root| DeviceStore::new(root).load().ok())
            .map(|state| state.recent_sessions_limit_or_default())
            .unwrap_or(DEFAULT_SIDEBAR_LIMIT);
        // One shared tracker: the palette guard, the scheduler skip, and the
        // Settings status line all read the same run state.
        let sync_tracker = SyncTracker::default();
        let home = cx.new(|cx| HomeView::new(data_root.clone(), sync_tracker.clone(), cx));
        home.update(cx, |view, cx| {
            view.set_space(spaces.names(), active_space.clone(), cx)
        });
        home.read(cx).focus_handle.clone().focus(window, cx);
        cx.subscribe_in(&home, window, |this, _, event, window, cx| match event {
            HomeEvent::OpenReference(reference) => {
                this.open_reference(reference.clone(), window, cx)
            }
            HomeEvent::OpenRepository { key, label } => {
                this.switch_repository(key, label, window, cx)
            }
            HomeEvent::Relationships => this.open_relationships(window, cx),
            HomeEvent::AddRepository => this.open_add_repository(window, cx),
            HomeEvent::LinkRepository { key } => this.open_link_repository(key, window, cx),
            HomeEvent::EditRepository { key } => this.open_edit_repository(key, window, cx),
            HomeEvent::UnlinkRepository { key } => this.unlink_repository(key, window, cx),
            HomeEvent::RemoveRepository { key } => this.confirm_remove_repository(key, window, cx),
            HomeEvent::OpenAgentSession(key) => this.open_agent_session(key.clone(), window, cx),
            HomeEvent::RefreshSessions => this.refresh_sessions(cx),
        })
        .detach();
        let settings = cx.new(|cx| {
            SettingsView::new(
                window,
                data_root.clone(),
                initial_font_size,
                sync_tracker.clone(),
                cx,
            )
        });
        // Best-effort current-repository match so the palette can mark it
        // without waiting for the first switch; a `--checkout` outside any
        // linked binding simply starts unmarked.
        let current_repository = data_root
            .as_ref()
            .and_then(|root| resolve_current_key(root, working_directory));

        let session_catalog = Catalog::new(data_root.as_ref());
        let initial_catalog = session_catalog.clone();
        cx.spawn(async move |this, cx| {
            cx.background_spawn(async move {
                initial_catalog.load_cache();
            })
            .await;
            if this
                .update(cx, |this, cx| {
                    this.publish_sessions(cx);
                    this.refresh_sessions(cx);
                })
                .is_err()
            {
                return;
            }
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(15))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.sessions_visible() {
                            this.refresh_sessions(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // Lightweight minute timer for relative-time labels. This only
        // re-projects the in-memory snapshot (no provider scans), so stale
        // `12m ago` rows refresh while visible without churning subprocesses.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                if this
                    .update(cx, |this, cx| {
                        if this.sessions_visible() {
                            this.publish_sessions(cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let resources = cx.new(|cx| {
            crate::artifacts::ArtifactBrowser::scoped(
                data_root.clone(),
                crate::artifacts::Scope::Repository(current_repository.clone()),
                cx,
            )
        });
        cx.subscribe_in(
            &resources,
            window,
            |this, _, event: &crate::preview::OpenReference, window, cx| {
                this.open_reference(event.0.clone(), window, cx);
            },
        )
        .detach();
        cx.subscribe_in(
            &resources,
            window,
            |this, _, event: &crate::artifacts::OpenSession, window, cx| {
                this.open_agent_session(event.0.key.clone(), window, cx);
            },
        )
        .detach();
        let relationships =
            cx.new(|cx| crate::repository_graph::GraphPage::new(data_root.clone(), window, cx));
        let workspace = cx.entity().downgrade();
        relationships.update(cx, |view, cx| {
            view.set_workspace(workspace);
            view.set_space(active_space.clone(), spaces.names(), cx)
        });
        cx.subscribe_in(
            &relationships,
            window,
            |this, _, event, window, cx| match event {
                crate::repository_graph::GraphEvent::AddRepository => {
                    this.open_add_repository(window, cx)
                }
                crate::repository_graph::GraphEvent::OpenRepository { key, label } => {
                    this.switch_repository(key, label, window, cx)
                }
                crate::repository_graph::GraphEvent::MetadataChanged => {
                    this.home.update(cx, |view, cx| view.reload(cx));
                    this.reload_recent_repositories();
                    this.publish_sessions(cx);
                    this.refresh_sessions(cx);
                }
            },
        )
        .detach();
        let mut agent_icon_tiles = AgentIconTiles::new();
        agent_icons::ensure_tiles(AgentKind::ALL, &mut agent_icon_tiles, cx);
        let onboarding = data_root
            .as_ref()
            .filter(|_| {
                stored
                    .as_ref()
                    .is_some_and(|state| state.onboarding_complete == Some(false))
            })
            .map(|root| {
                cx.new(|cx| crate::onboarding::OnboardingView::new(root.clone(), window, cx))
            });
        if let Some(view) = &onboarding {
            cx.subscribe_in(
                view,
                window,
                |this, _, _: &crate::onboarding::SetupComplete, window, cx| {
                    this.complete_onboarding(window, cx);
                },
            )
            .detach();
        }
        Self {
            relationships,
            relationships_visible: false,
            relationships_origin: None,
            home,
            resources,
            home_visible: true,
            spaces,
            active_space,
            space_menu_open: false,
            space_menu: None,
            space_menu_focus: None,
            portable_git_poll: GitPoll::default(),
            active_tab,
            tabs,
            editor_instance: None,
            editor_preference,
            external_editors,
            native_editor: None,
            review,
            session_agent,
            default_agent,
            enabled_agents,
            inactive_repositories: HashMap::new(),
            settings,
            onboarding,
            onboarding_reference: None,
            tool_views: HashMap::new(),
            project_name: project_name.into(),
            git_poll: GitPoll::default(),
            git_page_status: None,
            git_page_origin: None,
            git_header_busy: false,
            git_header_busy_focus: cx.focus_handle(),
            git_tab_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
            git_header_return_focus: None,
            git_picker_origin_focus: None,
            git_header_notice: None,
            git_view: None,
            git_commit_drafts: HashMap::new(),
            command_open: false,
            command_origin_focus: None,
            palette_mode: PaletteMode::Actions,
            palette_has_query: false,
            attention_only: false,
            command_state,
            agent_activity,
            session_catalog,
            session_snapshot: SessionSnapshot::default(),
            session_projects: Vec::new(),
            session_refreshing: false,
            session_navigation: 0,
            session_limit,
            open_sessions: HashMap::new(),
            agent_autostart: HashSet::new(),
            data_root,
            sync_tracker,
            last_sync_started: Instant::now(),
            working_directory: working_directory.to_owned(),
            current_repository,
            recent_repositories: Vec::new(),
            palette_model: Vec::new(),
            artifacts_origin: None,
            navigation_open: false,
            navigation_pane: 0,
            session_cursor: None,
            navigation_claimed_keys: HashMap::new(),
            agent_sidebar_focus: cx.focus_handle().tab_stop(true),
            agent_icon_tiles: Rc::new(agent_icon_tiles),
            _navigation_interceptor: navigation_interceptor,
        }
    }

    fn can_leave_git(&self, window: &mut Window, cx: &mut App) -> bool {
        if self
            .git_view
            .as_ref()
            .is_some_and(|git| git.read(cx).busy())
        {
            window.push_notification("Wait for the active Git operation before leaving Git", cx);
            false
        } else {
            true
        }
    }

    fn select_tab(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = WorkspaceTab::ALL.get(index).copied() else {
            return;
        };
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.close_navigation(cx);
        self.active_tab = tab;
        self.enter_repository(window, cx);
        if tab == WorkspaceTab::Review {
            self.review.update(cx, |view, cx| view.activate(cx));
        }
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    fn focus_active_pane(&mut self, window: &mut Window, cx: &mut App) {
        self.remember_agent_pane(cx);
        self.sync_activity_visibility(window, cx);
        if self.git_page_origin.is_some() {
            if let Some(git) = &self.git_view {
                git.focus_handle(cx).focus(window, cx);
            }
            return;
        }
        if self.git_header_busy {
            self.git_header_busy_focus.focus(window, cx);
            return;
        }
        if self.relationships_visible {
            self.relationships
                .read(cx)
                .focus_handle
                .clone()
                .focus(window, cx);
            return;
        }
        if self.home_visible {
            self.home.read(cx).focus_handle.clone().focus(window, cx);
            return;
        }
        if self.active_tab == WorkspaceTab::Review {
            self.review.read(cx).focus_handle.clone().focus(window, cx);
            return;
        }
        if self.active_tab == WorkspaceTab::Resources {
            self.resources
                .read(cx)
                .focus_handle
                .clone()
                .focus(window, cx);
            return;
        }
        if self.active_tab == WorkspaceTab::Editor
            && self.editor_instance == Some(EditorChoice::BuiltIn)
        {
            if let Some(editor) = self.native_editor.as_ref() {
                editor.read(cx).editor_focus(cx).focus(window, cx);
            }
            return;
        }
        if let Some(Some(pane)) = self.tabs.get(self.active_tab as usize) {
            let focus_handle = pane.read(cx).focus_handle.clone();
            focus_handle.focus(window, cx);
        }
    }

    fn sync_activity_visibility(&self, window: &Window, cx: &App) {
        let visible = if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && window.is_window_active()
        {
            match self.active_tab {
                WorkspaceTab::Agent => self.tabs[WorkspaceTab::Agent as usize]
                    .as_ref()
                    .and_then(|pane| pane.read(cx).launch_id()),
                _ => None,
            }
        } else {
            None
        };
        self.agent_activity.set_visible_launch(visible);
    }

    /// Jump straight to the Agent tab. An open command bar
    /// closes first, so the shortcut never leaves the palette stranded over
    /// the new tab.
    fn go_to_agent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_tab(WorkspaceTab::Agent as usize, window, cx);
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    /// Jump straight to the Editor tab. An open command bar
    /// closes first, so the shortcut never leaves the palette stranded over
    /// the new tab.
    fn go_to_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_tab(WorkspaceTab::Editor as usize, window, cx);
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    /// Jump back to the origin of the last follow-definition hop in the
    /// built-in editor. A no-op without a built-in session or history, so
    /// the command stays harmless wherever it is offered.
    fn editor_go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_history = self
            .native_editor
            .as_ref()
            .is_some_and(|editor| editor.read(cx).has_jump_history());
        if self.home_visible || !has_history || !self.can_leave_git(window, cx) {
            return;
        }
        self.select_tab(WorkspaceTab::Editor as usize, window, cx);
        if let Some(editor) = self.native_editor.as_ref() {
            editor.update(cx, |editor, cx| editor.go_back(window, cx));
        }
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    pub(crate) fn set_editor_choice(
        &mut self,
        choice: EditorChoice,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor_preference = choice;
        if !self.home_visible {
            self.editor_instance = Some(choice);
            if choice == EditorChoice::BuiltIn {
                if let Some(editor) = self.native_editor.as_ref() {
                    if self.active_tab == WorkspaceTab::Editor {
                        editor.update(cx, |editor, cx| editor.activate(window, cx));
                    }
                } else {
                    let external_editors = self.external_editors.clone();
                    let draft_root = self.data_root.as_ref().map(|root| root.root().to_owned());
                    self.native_editor = Some(cx.new(|cx| {
                        NativeEditor::new(
                            &self.working_directory,
                            external_editors,
                            draft_root.as_deref(),
                            window,
                            cx,
                        )
                    }));
                }
            }
            if choice == EditorChoice::Neovim && self.tabs[WorkspaceTab::Editor as usize].is_none()
            {
                self.tabs[WorkspaceTab::Editor as usize] = Some(cx.new(|cx| {
                    TerminalPane::new(
                        WorkspaceTab::Editor,
                        &self.working_directory,
                        self.default_agent,
                        &self.agent_activity,
                        cx,
                    )
                }));
            }
        }
        self.refresh_review_editor_action(cx);
        cx.notify();
    }

    pub(crate) fn set_external_editor_enabled(
        &mut self,
        kind: ExternalEditorKind,
        enabled: bool,
        cx: &mut Context<Self>,
    ) {
        self.external_editors.insert(kind.id().to_owned(), enabled);
        let external_editors = self.external_editors.clone();
        let available = ExternalEditorKind::ALL
            .into_iter()
            .filter(|kind| external_editors.get(kind.id()).copied().unwrap_or(false))
            .collect::<Vec<_>>();
        if let Some(git) = &self.git_view {
            git.update(cx, |git, cx| {
                git.set_external_editors(available.clone(), cx)
            });
        }
        for tabs in self.inactive_repositories.values() {
            if let Some(git) = &tabs.git_view {
                git.update(cx, |git, cx| {
                    git.set_external_editors(available.clone(), cx)
                });
            }
        }
        if let Some(editor) = self.native_editor.as_ref() {
            editor.update(cx, |view, cx| {
                view.set_external_editors(external_editors.clone(), cx)
            });
        }
        for tabs in self.inactive_repositories.values() {
            if let Some(editor) = tabs.native_editor.as_ref() {
                editor.update(cx, |view, cx| {
                    view.set_external_editors(external_editors.clone(), cx)
                });
            }
        }
        cx.notify();
    }

    fn refresh_review_editor_action(&mut self, cx: &mut Context<Self>) {
        let available = self.editor_instance == Some(EditorChoice::BuiltIn);
        self.review
            .update(cx, |view, cx| view.set_editor_open_available(available, cx));
    }

    /// Jump straight to the Terminal tab. An open command bar
    /// closes first, so the shortcut never leaves the palette stranded over
    /// the new tab.
    fn go_to_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_tab(WorkspaceTab::Terminal as usize, window, cx);
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    /// Jump straight to the Review tab. An open command bar
    /// closes first, so the shortcut never leaves the palette stranded over
    /// the new tab.
    fn go_to_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_tab(WorkspaceTab::Review as usize, window, cx);
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    /// Jump straight to the Resources tab. An open command bar
    /// closes first, so the shortcut never leaves the palette stranded over
    /// the new tab.
    fn go_to_resources(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.select_tab(WorkspaceTab::Resources as usize, window, cx);
        if self.command_open {
            self.close_command_palette(window, cx);
        }
    }

    fn toggle_command_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.attention_only = false;
        if self.command_open {
            // Same shortcut again closes; the other shortcut switches the
            // open bar to its view instead of closing it.
            if self.palette_mode == mode {
                self.close_command_palette(window, cx);
            } else {
                self.palette_mode = mode;
                self.palette_has_query = false;
                self.reload_recent_repositories();
                self.command_state.update(cx, |state, cx| {
                    state.set_query("", window, cx);
                });
                let query_focus = self.command_state.read(cx).focus_handle(cx).clone();
                query_focus.focus(window, cx);
                cx.notify();
            }
        } else {
            self.open_command_palette(mode, window, cx);
        }
    }

    fn open_command_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.command_open {
            return;
        }
        self.command_origin_focus = self.focused_content_handle(window, cx);
        // Opening another overlay clears stale navigation state: the palette
        // trap owns the keyboard from here, so the HUD must not stay behind it.
        self.close_navigation(cx);
        self.command_open = true;
        self.attention_only = false;
        self.palette_mode = mode;
        self.palette_has_query = false;
        self.sync_tutorial_palette(cx);
        // A few small JSON reads per opening — not per render — so the
        // switcher always reflects recent adds and switches.
        self.reload_recent_repositories();
        self.command_state.update(cx, |state, cx| {
            state.set_query("", window, cx);
        });
        // `read` borrows `cx`, so clone the handle first: focusing takes the
        // context mutably.
        let query_focus = self.command_state.read(cx).focus_handle(cx).clone();
        query_focus.focus(window, cx);
        cx.notify();
    }

    /// Refresh the palette switcher cache from disk. Opening, mode-switching,
    /// and repository switches all funnel here so render never touches the
    /// filesystem. Uncapped on purpose: the palette searches every switchable
    /// repository (recency-sorted), since Home doesn't exist yet to cover
    /// overflow.
    fn reload_recent_repositories(&mut self) {
        let mut repositories = self
            .data_root
            .as_ref()
            .map(|root| recent_repositories(root, usize::MAX))
            .unwrap_or_default();
        for repository in &mut repositories {
            repository.checkout_path = checkout_identity(&repository.checkout_path);
        }
        self.recent_repositories = repositories;
    }

    fn close_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.command_open {
            return;
        }
        self.command_open = false;
        self.command_origin_focus = None;
        self.sync_tutorial_palette(cx);
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Distinguish overlays from page replacements: composed tutorials can
    /// remain behind the palette, but must hide when Git replaces the page.
    fn sync_tutorial_palette(&mut self, cx: &mut Context<Self>) {
        let palette_open = self.command_open;
        let hidden = self.git_page_origin.is_some() || self.git_header_busy;
        self.home.update(cx, |view, cx| {
            view.set_workspace_visibility(palette_open, hidden, cx)
        });
        self.resources.update(cx, |view, cx| {
            view.set_workspace_visibility(palette_open, hidden, cx)
        });
    }

    /// Same cover rule for the navigation HUD.
    fn sync_tutorial_navigation(&mut self, cx: &mut Context<Self>) {
        let open = self.navigation_open;
        self.home
            .update(cx, |view, cx| view.set_navigation_open(open, cx));
        self.resources
            .update(cx, |view, cx| view.set_navigation_open(open, cx));
    }

    /// Apply completed first-run choices to the existing Home window.
    fn complete_onboarding(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self
            .data_root
            .as_ref()
            .and_then(|root| DeviceStore::new(root).load().ok())
        else {
            return;
        };
        self.onboarding = None;
        self.active_space = state
            .active_space
            .as_deref()
            .unwrap_or(crate::data::DEFAULT_SPACE)
            .into();
        self.set_enabled_agents(
            state.enabled_agents_or_default(),
            state.default_agent_or_default(),
            cx,
        );
        self.set_editor_choice(state.editor_choice_or_default(), window, cx);
        gpui_kit::component::Theme::change(
            crate::theme_mode_from_name(state.theme.as_deref().unwrap_or("dark")),
            Some(window),
            cx,
        );
        crate::apply_focus_theme(cx);
        self.reload_spaces(cx);
        self.home.update(cx, |view, cx| view.reload(cx));
        self.settings
            .update(cx, |view, cx| view.refresh_from_disk(window, cx));
        self.last_sync_started = Instant::now();
        self.home.read(cx).focus_handle.clone().focus(window, cx);
        if let Some(reference) = self.onboarding_reference.take() {
            self.open_reference(reference, window, cx);
        }
        cx.notify();
    }

    /// Open Settings as a modal dialog. The long-lived view preserves its
    /// selected section and pending edits across reopenings.
    fn open_settings(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_header_busy && !self.can_leave_git(window, cx) {
            return;
        }
        let settings = self.settings.clone();
        let workspace = cx.entity().downgrade();
        // Link the workspace for Sync-now delegation and reload the persisted
        // interval plus saved origin, so the Sync section never shows state
        // gone stale behind the dialog (e.g. after external git edits).
        settings.update(cx, |view, cx| {
            view.set_workspace(workspace);
            view.refresh_from_disk(window, cx);
        });
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Settings")
                .w(px(920.))
                .h(px(600.))
                .child(settings.clone().into_any_element())
        });
    }

    /// Open a developer tool as a modal dialog. The view is created once per
    /// tool and kept here, so its inputs and derived output survive
    /// close/reopen; the tool view owns the dialog chrome, including the
    /// close-time save flush.
    fn open_tool(&mut self, tool: ToolKind, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_header_busy && !self.can_leave_git(window, cx) {
            return;
        }
        let view = match self.tool_views.get(&tool) {
            Some(view) => view.clone(),
            None => {
                let data_root = self.data_root.clone();
                let view = cx.new(|cx| ToolView::new(window, cx, tool, data_root));
                self.tool_views.insert(tool, view.clone());
                view
            }
        };
        ToolView::open_dialog(view, window, cx);
    }

    /// Retain one native Git projection per checkout, refreshing on entry.
    fn ensure_git_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_view.is_some() {
            return;
        }
        self.git_page_status = None;
        let dirty_editor_paths = self
            .native_editor
            .as_ref()
            .map(|editor| editor.read(cx).dirty_paths())
            .unwrap_or_default();
        let git = cx.new(|cx| {
            GitView::new(
                self.working_directory.clone(),
                dirty_editor_paths,
                self.data_root.as_ref().map(|root| root.root().to_owned()),
                ExternalEditorKind::ALL
                    .into_iter()
                    .filter(|kind| {
                        self.external_editors
                            .get(kind.id())
                            .copied()
                            .unwrap_or(false)
                    })
                    .collect(),
                window,
                cx,
            )
        });
        cx.subscribe(&git, |this, _, event: &SnapshotChanged, cx| {
            if this.working_directory == event.checkout
                && !this.home_visible
                && !this.relationships_visible
                && this.git_page_status.as_ref() != Some(&event.status)
            {
                this.git_page_status = Some(event.status.clone());
                cx.notify();
            }
        })
        .detach();
        cx.subscribe_in(&git, window, |this, _, event: &OpenGitFile, window, cx| {
            this.open_git_file(event, window, cx)
        })
        .detach();
        if let Some(message) = self.git_commit_drafts.get(&self.working_directory) {
            git.update(cx, |git, cx| {
                git.restore_commit_message(message, window, cx)
            });
        }
        cx.subscribe(&git, |this, _, event: &CommitDraftChanged, cx| {
            if event.message.is_empty() {
                this.git_commit_drafts.remove(&event.checkout);
            } else {
                this.git_commit_drafts
                    .insert(event.checkout.clone(), event.message.clone());
            }
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &git,
            window,
            |this, _, event: &BranchPickerRequested, window, cx| {
                if event.checkout != this.working_directory {
                    return;
                }
                this.sync_git_view(cx);
                let Some(git) = this.git_view.clone() else {
                    return;
                };
                match &event.action {
                    BranchPickerAction::OpenGit => {
                        this.command_origin_focus = this.git_picker_origin_focus.take();
                        this.open_git_page(window, cx);
                    }
                    BranchPickerAction::Switch(reference) => {
                        this.git_header_return_focus = this.git_picker_origin_focus.take();
                        git.update(cx, |git, cx| {
                            git.switch_to_local_branch(reference.clone(), window, cx)
                        });
                    }
                    BranchPickerAction::NewBranch => {
                        this.git_header_return_focus = this.git_picker_origin_focus.take();
                        git.update(cx, |git, cx| git.open_new_branch_dialog(window, cx));
                    }
                }
            },
        )
        .detach();
        cx.observe_in(&git, window, |this, git, window, cx| {
            if this.git_view.as_ref() != Some(&git) {
                return;
            }
            let busy = git.read(cx).busy();
            if busy && this.git_page_origin.is_none() && !this.git_header_busy {
                if this.git_header_return_focus.is_none() {
                    this.git_header_return_focus = this.focused_content_handle(window, cx);
                }
                this.git_header_busy = true;
                this.git_header_busy_focus.focus(window, cx);
            } else if !busy && this.git_header_busy {
                this.git_header_busy = false;
                let focus = this.git_header_return_focus.take();
                if this.git_page_origin.is_none()
                    && !window.has_active_dialog(cx)
                    && !this.command_open
                {
                    this.focus_active_pane(window, cx);
                    if let Some(focus) = focus {
                        focus.focus(window, cx);
                    }
                }
            }
            let notice = git
                .read(cx)
                .operation_notice()
                .map(|(message, error)| (message.to_owned(), error));
            if this.git_header_notice.as_deref()
                != notice.as_ref().map(|(message, _)| message.as_str())
            {
                this.git_header_notice = notice.as_ref().map(|(message, _)| message.clone());
                if let Some((message, true)) = notice {
                    window.push_notification(message, cx);
                }
            }
            this.sync_tutorial_palette(cx);
            cx.notify();
        })
        .detach();
        self.git_view = Some(git);
    }

    fn select_git_mode(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_page_origin.is_some()
            && let Some(mode) = GitMode::ALL.get(index).copied()
            && let Some(git) = &self.git_view
        {
            self.git_tab_focus[index].focus(window, cx);
            git.update(cx, |git, cx| git.select_mode(mode, cx));
        }
    }

    fn open_git_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_page_origin.is_none() {
            let focus = self
                .command_origin_focus
                .take()
                .or_else(|| self.focused_content_handle(window, cx));
            self.git_page_origin = Some(GitPageOrigin {
                home_visible: self.home_visible,
                relationships_visible: self.relationships_visible,
                focus,
            });
        }
        self.close_navigation(cx);
        self.command_open = false;
        self.home_visible = false;
        self.relationships_visible = false;
        self.home.update(cx, |view, cx| view.deactivate(cx));
        self.relationships
            .update(cx, |view, cx| view.set_active(false, cx));
        self.resources
            .update(cx, |view, cx| view.set_active(false, cx));
        self.ensure_git_view(window, cx);
        self.sync_git_view(cx);
        self.sync_tutorial_palette(cx);
        self.sync_tutorial_navigation(cx);
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Overlay focus must never become a page's return target.
    fn focused_content_handle(&self, window: &Window, cx: &App) -> Option<FocusHandle> {
        if self.git_header_busy {
            return None;
        }
        self.navigation_panes(cx)
            .into_iter()
            .any(|(_, pane)| pane.contains_focused(window, cx))
            .then(|| window.focused(cx))
            .flatten()
    }

    fn back_from_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        let Some(origin) = self.git_page_origin.take() else {
            return;
        };
        self.close_navigation(cx);
        self.home_visible = origin.home_visible;
        self.relationships_visible = origin.relationships_visible;
        self.sync_git_view(cx);
        if self.relationships_visible {
            self.relationships
                .update(cx, |view, cx| view.set_active(true, cx));
        } else if self.home_visible {
            self.home.update(cx, |view, cx| view.resume_page(cx));
        } else {
            self.resources.update(cx, |view, cx| {
                view.set_active(self.active_tab == WorkspaceTab::Resources, cx)
            });
        }
        self.sync_tutorial_palette(cx);
        self.sync_tutorial_navigation(cx);
        self.focus_active_pane(window, cx);
        if let Some(focus) = origin.focus {
            focus.focus(window, cx);
        }
        cx.notify();
    }

    fn sync_git_view(&mut self, cx: &mut Context<Self>) {
        let active = !self.home_visible && !self.relationships_visible;
        if !active {
            self.git_page_status = None;
        }
        if let Some(git) = &self.git_view {
            let dirty = self
                .native_editor
                .as_ref()
                .map(|editor| editor.read(cx).dirty_paths())
                .unwrap_or_default();
            git.update(cx, |git, cx| {
                git.set_dirty_editor_paths(dirty);
                git.set_page_visible(self.git_page_origin.is_some());
                git.set_active(active, cx);
            });
        }
    }

    fn open_git_file(&mut self, event: &OpenGitFile, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        if event.checkout != self.working_directory
            || event.path.is_absolute()
            || event
                .path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            window.push_notification("The selected Git path is no longer valid.", cx);
            return;
        }
        let file = self.working_directory.join(&event.path);
        if !file.is_file() {
            window.push_notification(
                format!(
                    "{} is unavailable in the working tree.",
                    event.path.display()
                ),
                cx,
            );
            return;
        }
        self.set_editor_choice(EditorChoice::BuiltIn, window, cx);
        if let Some(editor) = self.native_editor.as_ref() {
            editor.update(cx, |editor, cx| editor.request_open(file, None, cx));
            self.active_tab = WorkspaceTab::Editor;
            self.git_page_origin = None;
            self.sync_git_view(cx);
            self.sync_tutorial_palette(cx);
            self.sync_tutorial_navigation(cx);
            self.home_visible = false;
            self.focus_active_pane(window, cx);
            cx.notify();
        } else {
            window.push_notification("The built-in editor is not ready yet.", cx);
        }
    }

    /// Run the confirmed palette entry. `index` addresses the model installed
    /// by the latest `Command` render (before filtering), so it resolves
    /// against [`palette_model`](Self::palette_model) however the query
    /// narrowed the list.
    fn on_palette_confirm(
        &mut self,
        index: IndexPath,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = item_at(&self.palette_model, index.section, index.row) else {
            self.close_command_palette(window, cx);
            return;
        };
        // Every path below leaves the bar closed; `select_tab` focuses and
        // notifies itself, the other arms do it explicitly.
        self.command_open = false;
        self.sync_tutorial_palette(cx);
        let focus_attention_target = self.attention_only;
        match item {
            PaletteItem::SwitchRepository { key, label } => {
                self.switch_repository(&key, &label, window, cx);
                if focus_attention_target && !self.home_visible {
                    self.focus_attention_session(window, cx);
                    self.select_tab(WorkspaceTab::Agent as usize, window, cx);
                }
            }
            PaletteItem::OpenCheckout { checkout_path, .. } => {
                self.open_managed_checkout(&checkout_path, window, cx);
                if focus_attention_target && !self.home_visible {
                    self.focus_attention_session(window, cx);
                    self.select_tab(WorkspaceTab::Agent as usize, window, cx);
                }
            }
            PaletteItem::Tool(tool) => self.open_tool(tool, window, cx),
            PaletteItem::Command(command) => match command {
                PaletteCommand::GoAgent => self.select_tab(0, window, cx),
                PaletteCommand::GoEditor => self.select_tab(1, window, cx),
                PaletteCommand::GoTerminal => self.select_tab(2, window, cx),
                PaletteCommand::GoReview => self.select_tab(3, window, cx),
                PaletteCommand::GoResources => self.select_tab(4, window, cx),
                PaletteCommand::OpenGit => self.open_git_page(window, cx),
                PaletteCommand::OpenSettings => self.open_settings(window, cx),
                PaletteCommand::EditorGoBack => self.editor_go_back(window, cx),
                PaletteCommand::AddRepository => self.open_add_repository(window, cx),
                PaletteCommand::GoHome => self.go_home(window, cx),
                PaletteCommand::RepositoryRelationships => self.open_relationships(window, cx),
                PaletteCommand::BrowseArtifacts => self.browse_artifacts(window, cx),
                PaletteCommand::SyncPortable => {
                    self.request_sync(window, cx);
                    self.focus_active_pane(window, cx);
                    cx.notify();
                }
            },
        }
        self.command_origin_focus = None;
    }

    /// Open the Add Repository dialog with a fresh form: re-harvesting on
    /// every opening beats reasoning about stale prefill. The dialog owns
    /// open/close; the view is dropped with it. A successful add switches
    /// straight into the new checkout so the dialog does not finish
    /// silently.
    fn open_add_repository(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_header_busy && !self.can_leave_git(window, cx) {
            return;
        }
        let data_root = self.data_root.clone();
        let spaces = self.spaces.names();
        let active_space = self.active_space.creation_space(&spaces);
        let view = cx.new(|cx| AddRepositoryView::new(window, cx, data_root, spaces, active_space));
        cx.subscribe_in(
            &view,
            window,
            |this, _, event: &crate::add_repository::RepositoryAdded, window, cx| {
                this.home.update(cx, |view, cx| view.reload(cx));
                this.relationships.update(cx, |view, cx| view.refresh(cx));
                this.reload_recent_repositories();
                this.switch_repository(&event.0, &event.0, window, cx);
            },
        )
        .detach();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title("Add repository")
                .w(px(680.))
                .h(px(640.))
                .child(view.clone().into_any_element())
        });
    }

    /// Open the Link dialog for an existing portable record: checkout picker
    /// only, key fixed. Used from the Projects page for records synced from
    /// another device (or left unbound by an interrupted create). A
    /// successful link switches into the checkout, matching Add.
    fn open_link_repository(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let data_root = self.data_root.clone();
        let key = key.to_owned();
        let spaces = self.spaces.names();
        let active_space = self.active_space.creation_space(&spaces);
        let view = cx.new(|cx| {
            AddRepositoryView::for_link(window, cx, data_root, key.clone(), spaces, active_space)
        });
        cx.subscribe_in(
            &view,
            window,
            |this, _, event: &crate::add_repository::RepositoryAdded, window, cx| {
                this.home.update(cx, |view, cx| view.reload(cx));
                this.relationships.update(cx, |view, cx| view.refresh(cx));
                this.reload_recent_repositories();
                this.switch_repository(&event.0, &event.0, window, cx);
            },
        )
        .detach();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .title(format!("Link checkout for \"{key}\""))
                .w(px(680.))
                .h(px(320.))
                .child(view.clone().into_any_element())
        });
    }

    /// Open the Edit dialog for an existing portable record: metadata fields
    /// prefilled, key fixed (renames deferred). Only the portable JSON is
    /// rewritten; the device binding is untouched.
    fn open_edit_repository(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.data_root.clone() else {
            window.push_notification("Portable data is unavailable", cx);
            return;
        };
        let key = key.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let read_key = key.clone();
            let result = cx
                .background_spawn(
                    async move { crate::data::get_repository_metadata(&root, &read_key) },
                )
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                let metadata = match result {
                    Ok(metadata) => metadata,
                    Err(error) => {
                        window
                            .push_notification(format!("Could not load \"{key}\": {error:#}"), cx);
                        return;
                    }
                };
                let data_root = this.data_root.clone();
                let spaces = this.spaces.names();
                let active_space = this.active_space.creation_space(&spaces);
                let view = cx.new(|cx| {
                    AddRepositoryView::for_edit(
                        window,
                        cx,
                        data_root,
                        key.clone(),
                        &metadata,
                        spaces,
                        active_space,
                    )
                });
                let workspace = cx.entity().downgrade();
                view.update(cx, |view, _| view.set_workspace(workspace));
                cx.subscribe(
                    &view,
                    |this, _, _: &crate::add_repository::RepositoryAdded, cx| {
                        this.home.update(cx, |view, cx| view.reload(cx));
                        this.relationships.update(cx, |view, cx| view.refresh(cx));
                        this.reload_recent_repositories();
                    },
                )
                .detach();
                window.open_dialog(cx, move |dialog, _, _| {
                    dialog
                        .title(format!("Edit \"{key}\""))
                        .w(px(680.))
                        .h(px(640.))
                        .child(view.clone().into_any_element())
                });
            });
        })
        .detach();
    }

    /// Drop this device's checkout binding, keeping the portable record.
    /// Non-destructive by construction: the Projects page offers Re-link
    /// right where the row stays.
    fn unlink_repository(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.data_root.clone() else {
            window.push_notification("Portable data is unavailable", cx);
            return;
        };
        match crate::data::unlink_repository(&root, key) {
            Ok(true) => {
                window.push_notification(format!("Unlinked \"{key}\""), cx);
            }
            Ok(false) => {
                window.push_notification(format!("\"{key}\" had no checkout binding"), cx);
            }
            Err(error) => {
                window.push_notification(format!("Could not unlink \"{key}\": {error:#}"), cx);
                return;
            }
        }
        self.home.update(cx, |view, cx| view.reload(cx));
        self.reload_recent_repositories();
        cx.notify();
    }

    /// Confirm, then delete a portable record and its device binding. The
    /// local checkout stays on disk; only Devcroft's records go away.
    fn confirm_remove_repository(&self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let workspace = cx.entity().downgrade();
        let key = key.to_owned();
        window.open_dialog(cx, move |dialog, _, _| {
            let workspace = workspace.clone();
            let key = key.clone();
            dialog
                .title(format!("Delete \"{key}\"?"))
                .child(format!(
                    "This removes the portable record for \"{key}\" and this device's checkout binding. The local checkout itself stays on disk. This cannot be undone."
                ))
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("cancel-remove-repository")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("confirm-remove-repository")
                                .primary()
                                .label("Delete")
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(Box::new(Confirm { secondary: false }), cx)
                                }),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    workspace
                        .update(cx, |this, cx| {
                            this.remove_repository_confirmed(&key, window, cx)
                        })
                        .unwrap_or(false)
                })
        });
    }

    /// Run the confirmed removal: delete records, reload lists, notify.
    /// Returns true so the confirm dialog closes either way; failures are
    /// surfaced as notifications.
    fn remove_repository_confirmed(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(root) = self.data_root.clone() else {
            window.push_notification("Portable data is unavailable", cx);
            return true;
        };
        let key = key.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let delete_key = key.clone();
            let result = cx
                .background_spawn(async move { crate::data::remove_repository(&root, &delete_key) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => {
                        window.push_notification(format!("Deleted \"{key}\""), cx);
                        this.home.update(cx, |view, cx| view.reload(cx));
                        this.relationships.update(cx, |view, cx| view.refresh(cx));
                        this.reload_recent_repositories();
                    }
                    Err(error) => window
                        .push_notification(format!("Could not delete \"{key}\": {error:#}"), cx),
                }
                cx.notify();
            });
        })
        .detach();
        true
    }

    /// Switch visible checkout after recording recency. Hidden checkout entities
    /// stay owned by the workspace, so their processes and view state survive
    /// until the window closes. Only a first visit creates new panes.
    fn switch_repository(
        &mut self,
        key: &str,
        label: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.close_navigation(cx);
        // Resolve and record even on re-entry from Home. Restoring the same
        // checkout is a no-op, so its running PTYs remain untouched.
        let Some(root) = self.data_root.clone() else {
            window.push_notification("Portable data is unavailable", cx);
            self.focus_active_pane(window, cx);
            cx.notify();
            return;
        };
        // Resolve fresh, not from the cached palette list, so a record
        // added or linked since the palette opened still switches.
        let Some(checkout) = checkout_for(&root, key) else {
            window.push_notification(format!("No linked checkout for \"{label}\""), cx);
            self.focus_active_pane(window, cx);
            cx.notify();
            return;
        };
        if !checkout.is_dir() {
            window.push_notification(
                format!(
                    "The linked checkout for \"{label}\" is unavailable at {}",
                    checkout.display()
                ),
                cx,
            );
            self.focus_active_pane(window, cx);
            cx.notify();
            return;
        }
        if let Err(error) = record_repository_open(&root, key) {
            window.push_notification(
                format!("Could not record recency for \"{label}\": {error:#}"),
                cx,
            );
            self.focus_active_pane(window, cx);
            cx.notify();
            return;
        }
        let checkout = checkout_identity(&checkout);
        self.restore_repository_tabs(&checkout, cx);
        self.enter_repository(window, cx);
        self.project_name = checkout
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned()
            .into();
        self.current_repository = Some(key.to_owned());
        self.resources.update(cx, |view, cx| {
            view.set_scope(
                crate::artifacts::Scope::Repository(self.current_repository.clone()),
                cx,
            )
        });
        // Reset plus an immediate fresh load: the header shows the new
        // checkout's state within one scan instead of flashing the old
        // checkout's status until the next poll tick — and the load id
        // invalidates the tick that was in flight for the old checkout.
        self.refresh_git_status(cx);
        self.reload_recent_repositories();
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Activate a retained checkout that has no portable repository entry.
    /// These rows are synthesized only for live managed agent panes, so this
    /// path never creates a new arbitrary checkout from palette input.
    fn open_managed_checkout(
        &mut self,
        checkout: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        // A location change clears stale navigation state.
        self.close_navigation(cx);
        let checkout = checkout_identity(checkout);
        if checkout != self.working_directory && !self.inactive_repositories.contains_key(&checkout)
        {
            window.push_notification("That agent checkout is no longer open", cx);
            self.focus_active_pane(window, cx);
            cx.notify();
            return;
        }
        self.restore_repository_tabs(&checkout, cx);
        self.enter_repository(window, cx);
        self.project_name = checkout
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workspace")
            .to_owned()
            .into();
        self.current_repository = self
            .data_root
            .as_ref()
            .and_then(|root| resolve_current_key(root, &checkout));
        self.resources.update(cx, |view, cx| {
            view.set_scope(
                crate::artifacts::Scope::Repository(self.current_repository.clone()),
                cx,
            )
        });
        self.refresh_git_status(cx);
        self.reload_recent_repositories();
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    fn restore_repository_tabs(&mut self, checkout: &Path, cx: &mut Context<Self>) {
        if self.working_directory == checkout {
            return;
        }
        self.git_page_status = None;
        self.git_page_origin = None;
        self.git_header_notice = None;
        self.git_picker_origin_focus = None;
        self.git_header_return_focus = None;
        // Restored tabs keep their running session; unseen checkouts start
        // fresh and resolve to the global default on entry.
        let next = self
            .inactive_repositories
            .remove(checkout)
            .unwrap_or_else(|| {
                RepositoryTabs::new(
                    checkout,
                    self.default_agent,
                    &self.agent_activity,
                    self.editor_preference,
                    cx,
                )
            });
        if let Some(git) = &self.git_view {
            git.update(cx, |git, cx| git.set_active(false, cx));
        }
        let previous = RepositoryTabs {
            git_view: std::mem::replace(&mut self.git_view, next.git_view),
            active_tab: std::mem::replace(&mut self.active_tab, next.active_tab),
            tabs: std::mem::replace(&mut self.tabs, next.tabs),
            editor_instance: std::mem::replace(&mut self.editor_instance, next.editor_instance),
            native_editor: std::mem::replace(&mut self.native_editor, next.native_editor),
            review: std::mem::replace(&mut self.review, next.review),
            // Swap the session agents alongside their tab sets: the active
            // panes keep the harness they were spawned with, and the hidden
            // set keeps its own.
            agent: std::mem::replace(&mut self.session_agent, next.agent),
        };
        let previous_directory =
            std::mem::replace(&mut self.working_directory, checkout.to_owned());
        self.inactive_repositories
            .insert(previous_directory, previous);
    }

    fn enter_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.git_page_origin = None;
        self.sync_tutorial_palette(cx);
        self.sync_tutorial_navigation(cx);
        // Repository ownership can change while a pane is retained. Never
        // silently restore an agent running under the previous account.
        let account = crate::agent_accounts::Accounts::load(self.data_root.as_ref())
            .map(|accounts| accounts.for_checkout(&self.working_directory));
        let agent_index = WorkspaceTab::Agent as usize;
        if self.tabs[agent_index].as_ref().is_some_and(|pane| {
            !account
                .as_ref()
                .is_ok_and(|account| pane.read(cx).account_matches(account))
        }) {
            self.remember_agent_pane(cx);
            self.tabs[agent_index] = None;
            self.agent_autostart.remove(&self.working_directory);
        }
        self.relationships_visible = false;
        self.relationships
            .update(cx, |view, cx| view.set_active(false, cx));
        // Catch-all for tab/location changes (select, switch, session open):
        // a stale HUD must not survive the move.
        self.close_navigation(cx);
        self.artifacts_origin = None;
        self.resources.update(cx, |view, cx| {
            view.set_active(self.active_tab == WorkspaceTab::Resources, cx)
        });
        self.home_visible = false;
        self.home.update(cx, |view, cx| view.deactivate(cx));
        if self.editor_instance.is_none() {
            self.editor_instance = Some(self.editor_preference);
        }
        if self.editor_instance == Some(EditorChoice::BuiltIn) {
            if let Some(editor) = self.native_editor.as_ref() {
                if self.active_tab == WorkspaceTab::Editor {
                    editor.update(cx, |editor, cx| editor.activate(window, cx));
                }
            } else {
                let external_editors = self.external_editors.clone();
                let draft_root = self.data_root.as_ref().map(|root| root.root().to_owned());
                self.native_editor = Some(cx.new(|cx| {
                    NativeEditor::new(
                        &self.working_directory,
                        external_editors,
                        draft_root.as_deref(),
                        window,
                        cx,
                    )
                }));
            }
        }
        self.refresh_review_editor_action(cx);
        self.sync_git_view(cx);
        for tab in WorkspaceTab::ALL {
            if tab.has_terminal()
                && self.tabs[tab as usize].is_none()
                && (tab != WorkspaceTab::Editor
                    || self.editor_instance == Some(EditorChoice::Neovim))
            {
                if tab == WorkspaceTab::Agent
                    && self.agent_autostart.insert(self.working_directory.clone())
                    && let Some(last) = self
                        .session_snapshot
                        .recent(&self.working_directory, 1)
                        .into_iter()
                        .next()
                {
                    // First entry starts the last session, not a fresh
                    // shell. The shared open path owns async validation
                    // and pane creation; a failed resume keeps the
                    // placeholder with Retry guidance instead of silently
                    // opening a new conversation.
                    self.open_agent_session(last.key, window, cx);
                    continue;
                }
                if tab == WorkspaceTab::Agent {
                    self.session_agent = self.default_agent;
                }
                self.tabs[tab as usize] = Some(cx.new(|cx| {
                    TerminalPane::new(
                        tab,
                        &self.working_directory,
                        self.default_agent,
                        &self.agent_activity,
                        cx,
                    )
                }));
            }
        }
    }

    fn open_relationships(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.git_page_origin = None;
        if !self.relationships_visible {
            self.relationships_origin = Some(if self.home_visible {
                PageOrigin::Home
            } else {
                PageOrigin::Repository
            });
        }
        self.close_navigation(cx);
        self.home.update(cx, |view, cx| view.deactivate(cx));
        self.resources
            .update(cx, |view, cx| view.set_active(false, cx));
        self.relationships_visible = true;
        self.home_visible = true;
        self.sync_git_view(cx);
        self.command_open = false;
        self.sync_tutorial_palette(cx);
        self.relationships
            .update(cx, |view, cx| view.set_active(true, cx));
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    fn back_from_relationships(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.relationships_visible = false;
        self.relationships
            .update(cx, |view, cx| view.set_active(false, cx));
        match self.relationships_origin.take() {
            Some(PageOrigin::Repository) => self.enter_repository(window, cx),
            _ => {
                self.home.update(cx, |view, cx| view.resume_page(cx));
            }
        }
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Switch isolation profiles: persist the choice machine-locally and
    /// re-project every space-filtered surface. The open repository workspace
    /// stays open (an explicit context); its sidebar keeps its own sessions.
    pub(crate) fn switch_space(
        &mut self,
        selection: SpaceSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selection = match selection {
            SpaceSelection::All => SpaceSelection::All,
            SpaceSelection::Named(name) => {
                let Some(canonical) = self.spaces.canonical(&name) else {
                    return;
                };
                SpaceSelection::Named(canonical)
            }
        };
        if self.active_space.equivalent(&selection) {
            self.close_space_menu(window, cx);
            return;
        }
        self.active_space = selection.clone();
        self.close_space_menu(window, cx);
        self.close_navigation(cx);
        if let Some(root) = &self.data_root {
            let _ = DeviceStore::new(root).update(|state| {
                state.all_spaces = Some(selection == SpaceSelection::All);
                if let Some(name) = selection.name() {
                    state.active_space = Some(name.to_owned());
                }
            });
        }
        self.refresh_spaces();
        let names = self.spaces.names();
        self.home.update(cx, |view, cx| {
            view.set_space(names.clone(), self.active_space.clone(), cx);
        });
        self.relationships.update(cx, |view, cx| {
            view.set_space(self.active_space.clone(), names, cx);
        });
        self.reload_recent_repositories();
        self.publish_sessions(cx);
        self.refresh_sessions(cx);
        cx.notify();
    }

    /// Toggle the titlebar space switcher (the navigation-mode `s` entry
    /// opens it). Reconciles the catalog first so a space added on another
    /// device shows up without a restart.
    fn toggle_space_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.space_menu_open {
            self.close_space_menu(window, cx);
        } else {
            self.open_space_menu(window, cx);
        }
    }

    /// Whether either artifact browser (the global Home one or the
    /// repository Resources tab) holds an unsaved draft. Settings blocks
    /// catalog renames/deletes then: the rewrite replaces the records a
    /// draft is based on, and a stashed draft's key can name the space being
    /// edited. Catalog edits are rare enough that a blanket guard is
    /// cheaper than tracking every draft-to-space relationship.
    pub(crate) fn has_artifact_draft(&self, cx: &App) -> bool {
        self.home.read(cx).has_artifact_draft(cx) || self.resources.read(cx).has_draft()
    }

    /// Attention count for the active space: agent panes in registered
    /// repositories count under their repository's space; unregistered
    /// checkouts (explicitly opened ad-hoc) always count, matching the Home
    /// session cards' rule.
    fn attention_count(&self) -> usize {
        self.agent_activity
            .snapshot()
            .launches_by_checkout()
            .filter(|(checkout, agent)| {
                agent.state == ActivityState::NeedsAttention
                    && self
                        .session_space(checkout)
                        .is_none_or(|space| self.active_space.matches(&space))
            })
            .count()
    }

    /// Re-read the portable catalog. Best effort: a read failure keeps the
    /// in-memory catalog so the switcher still works.
    fn refresh_spaces(&mut self) {
        if let Some(root) = &self.data_root
            && let Ok(catalog) = ensure_spaces(root)
        {
            self.spaces = catalog;
        }
    }

    /// Re-read the catalog after Settings edits and re-project every
    /// space-filtered surface. When the active name no longer exists (it was
    /// renamed or deleted), follow the retargeted `device.json` value; a
    /// value the catalog does not know falls back to the first space.
    pub(crate) fn reload_spaces(&mut self, cx: &mut Context<Self>) {
        self.refresh_spaces();
        if self
            .active_space
            .name()
            .is_some_and(|name| !self.spaces.contains(name))
        {
            let stored = self
                .data_root
                .as_ref()
                .and_then(|root| DeviceStore::new(root).load().ok());
            self.active_space = self.spaces.selection(
                stored
                    .as_ref()
                    .and_then(|state| state.active_space.as_deref()),
                stored
                    .as_ref()
                    .and_then(|state| state.all_spaces)
                    .unwrap_or(false),
            );
            if let Some(root) = &self.data_root {
                let active = self.active_space.clone();
                let _ = DeviceStore::new(root).update(|state| {
                    state.active_space = active.name().map(str::to_owned);
                    state.all_spaces = Some(active == SpaceSelection::All);
                });
            }
        }
        let names = self.spaces.names();
        self.home.update(cx, |view, cx| {
            view.set_space(names.clone(), self.active_space.clone(), cx);
        });
        self.relationships.update(cx, |view, cx| {
            view.set_space(self.active_space.clone(), names, cx);
        });
        self.reload_recent_repositories();
        self.publish_sessions(cx);
        self.refresh_sessions(cx);
        cx.notify();
    }

    /// Titlebar space switcher: one controlled popover so the mouse and the
    /// navigation-mode `s` entry open the same menu. The menu lists the
    /// catalog in order with the active space checked; picking one switches
    /// every space-filtered surface.
    fn space_switcher(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let active = self.active_space.clone();
        let menu = self.space_menu.clone();
        // The popover focuses its tracked handle when it opens; pointing that
        // at the menu is what makes the `s` entry keyboard-navigable (the
        // menu is built before the popover's own open transition runs).
        let menu_focus = menu.as_ref().map(|menu| menu.focus_handle(cx));
        Popover::new("space-switcher")
            .open(self.space_menu_open)
            .appearance(false)
            .when_some(menu_focus, |popover, focus| popover.track_focus(&focus))
            .on_open_change(cx.listener(|this, open, window, cx| {
                // The popover owns its trigger's toggling; this callback is
                // the single place that opens and closes the menu.
                if *open {
                    this.open_space_menu(window, cx);
                } else {
                    this.close_space_menu(window, cx);
                }
            }))
            .trigger(
                Button::new("space-switcher-trigger")
                    .ghost()
                    .small()
                    .dropdown_caret(true)
                    .max_w(px(200.))
                    .label(active.selector_label())
                    .tooltip("Switch space · s in navigation mode"),
            )
            .content(move |_, _, _| match &menu {
                Some(menu) => menu.clone().into_any_element(),
                None => div().into_any_element(),
            })
    }

    /// Build the switcher's menu for the current catalog and show it. The
    /// menu is created per opening (catalog edits and space switches are
    /// reflected immediately) and owned by the workspace, so no window-keyed
    /// element state is involved.
    fn open_space_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_spaces();
        let selections = std::iter::once(SpaceSelection::All)
            .chain(self.spaces.names().into_iter().map(SpaceSelection::Named))
            .collect::<Vec<_>>();
        let active = self.active_space.clone();
        let workspace = cx.entity().downgrade();
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            let mut menu = menu.min_w(px(200.));
            for selection in selections {
                let selected = selection.equivalent(&active);
                let workspace = workspace.clone();
                menu = menu.item(
                    PopupMenuItem::new(selection.selector_label())
                        .checked(selected)
                        .on_click(move |_, window, cx| {
                            let _ = workspace.update(cx, |this, cx| {
                                this.switch_space(selection.clone(), window, cx)
                            });
                        }),
                );
            }
            menu
        });
        // Dismissing the menu (Escape, an item pick) closes the popover too.
        let this = cx.entity().downgrade();
        window
            .subscribe(&menu, cx, move |_, _: &DismissEvent, window, cx| {
                let _ = this.update(cx, |this, cx| this.close_space_menu(window, cx));
            })
            .detach();
        // The popover focuses its own handle; the menu needs focus for its
        // arrow keys and Enter to work, like the built-in dropdown.
        self.space_menu_focus = Some(menu.focus_handle(cx));
        menu.focus_handle(cx).focus(window, cx);
        self.space_menu = Some(menu);
        self.space_menu_open = true;
        cx.notify();
    }

    /// Hide the switcher and drop its menu. When the menu itself holds
    /// focus, focus moves back to Home first: leaving it on a dropped
    /// handle would swallow every following keystroke.
    fn close_space_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.space_menu_open && self.space_menu.is_none() {
            return;
        }
        let menu_focused = self
            .space_menu_focus
            .as_ref()
            .is_some_and(|handle| handle.contains_focused(window, cx));
        self.space_menu = None;
        self.space_menu_focus = None;
        self.space_menu_open = false;
        if menu_focused {
            self.home.read(cx).focus_handle.clone().focus(window, cx);
        }
        cx.notify();
    }

    fn go_home(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.git_page_origin = None;
        self.relationships_visible = false;
        self.relationships
            .update(cx, |view, cx| view.set_active(false, cx));
        self.close_navigation(cx);
        self.artifacts_origin = None;
        self.resources
            .update(cx, |view, cx| view.set_active(false, cx));
        self.home_visible = true;
        self.sync_git_view(cx);
        self.command_open = false;
        self.sync_tutorial_palette(cx);
        self.home.update(cx, |view, cx| view.activate(cx));
        self.refresh_sessions(cx);
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    /// Jump to Home's artifact browser (the command palette's
    /// **Browse artifacts** entry). The global view includes unassociated legacy artifacts.
    /// Deactivates the repository resources view so its poll stops
    /// while Home is visible. Captures the origin so the titlebar back
    /// button returns where the user came from.
    fn browse_artifacts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_leave_git(window, cx) {
            return;
        }
        self.git_page_origin = None;
        self.relationships_visible = false;
        self.relationships
            .update(cx, |view, cx| view.set_active(false, cx));
        self.close_navigation(cx);
        let already_there = self.home_visible && self.home.read(cx).is_artifacts_page();
        if !already_there {
            let origin = if !self.home_visible {
                PageOrigin::Repository
            } else {
                PageOrigin::Home
            };
            self.artifacts_origin = Some(origin);
        }
        self.resources
            .update(cx, |view, cx| view.set_active(false, cx));
        self.home_visible = true;
        self.sync_git_view(cx);
        self.command_open = false;
        self.sync_tutorial_palette(cx);
        self.home
            .update(cx, |view, cx| view.show_artifacts_page(cx));
        self.focus_active_pane(window, cx);
        cx.notify();
    }

    pub(crate) fn open_reference(
        &mut self,
        reference: crate::markdown_references::Reference,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.onboarding.is_some() {
            self.onboarding_reference = Some(reference);
            return;
        }
        if !self.can_leave_git(window, cx) {
            return;
        }
        use crate::markdown_references::Reference;
        match reference {
            Reference::Repository(key) => self.switch_repository(&key, &key, window, cx),
            Reference::Artifact(id) => {
                match self.home.update(cx, |home, cx| home.open_artifact(id, cx)) {
                    Ok(()) => self.browse_artifacts(window, cx),
                    Err(error) => window.push_notification(error.to_string(), cx),
                }
            }
            reference @ Reference::Session { .. } => {
                let Some(root) = self.data_root.clone() else {
                    window.push_notification("Portable data is unavailable", cx);
                    return;
                };
                self.session_navigation = self.session_navigation.wrapping_add(1);
                let generation = self.session_navigation;
                let catalog = self.session_catalog.clone();
                cx.spawn_in(window, async move |this, cx| {
                    let result = cx
                        .background_spawn(async move {
                            catalog.refresh();
                            reference.resolve_session(&root, &catalog.snapshot().sessions)
                        })
                        .await;
                    let _ = this.update_in(cx, |this, window, cx| {
                        if this.session_navigation != generation {
                            return;
                        }
                        match result {
                            Ok(key) => this.open_agent_session(key, window, cx),
                            Err(error) => window.push_notification(
                                format!("Could not open session reference: {error}"),
                                cx,
                            ),
                        }
                    });
                })
                .detach();
            }
        }
    }

    /// Return from the global artifact browser to its captured origin.
    fn go_back_from_artifacts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_navigation(cx);
        match self.artifacts_origin.take() {
            Some(PageOrigin::Repository) => {
                self.enter_repository(window, cx);
                self.focus_active_pane(window, cx);
                cx.notify();
            }
            Some(PageOrigin::Home) | None => {
                self.go_home(window, cx);
            }
        }
    }

    fn artifacts_back_label(&self) -> SharedString {
        match self.artifacts_origin {
            Some(PageOrigin::Repository) => format!("‹ {}", self.project_name).into(),
            Some(PageOrigin::Home) | None => "‹ Home".into(),
        }
    }

    /// Apply a Settings > Agent sidebar limit live: re-project the
    /// in-memory snapshot so the sidebar updates immediately. The caller
    /// owns persistence; values snap to the slider step for defense in depth.
    pub(crate) fn set_session_limit(&mut self, limit: usize, cx: &mut Context<Self>) {
        use crate::agent_sessions::snap_sidebar_limit;
        let limit = snap_sidebar_limit(limit);
        if self.session_limit == limit {
            return;
        }
        self.session_limit = limit;
        self.publish_sessions(cx);
    }

    /// Apply a Settings > Agent enable set live: fresh Agent panes and the
    /// New-session picker only offer these afterwards. The caller owns
    /// persistence; open sessions keep running. Never ends empty — an empty
    /// update falls back to all harnesses — and the default always stays
    /// within the enabled set.
    pub(crate) fn set_enabled_agents(
        &mut self,
        enabled: Vec<AgentKind>,
        default: AgentKind,
        cx: &mut Context<Self>,
    ) {
        let mut enabled = enabled;
        if enabled.is_empty() {
            enabled = AgentKind::ALL.to_vec();
        }
        let mut ordered: Vec<AgentKind> = AgentKind::ALL
            .into_iter()
            .filter(|agent| enabled.contains(agent))
            .collect();
        if ordered.is_empty() {
            ordered = AgentKind::ALL.to_vec();
        }
        let default = if ordered.contains(&default) {
            default
        } else {
            ordered.first().copied().unwrap_or(AgentKind::DEFAULT)
        };
        let changed = self.enabled_agents != ordered || self.default_agent != default;
        self.enabled_agents = ordered;
        self.default_agent = default;
        if !self.enabled_agents.contains(&self.session_agent) {
            // Open panes keep running; only future fresh panes use the
            // default. No session_agent rewrite needed.
        }
        if changed {
            cx.notify();
        }
    }

    /// Apply a Settings > Agent default harness live: fresh Agent panes and
    /// the New-session picker pick it up immediately. The caller owns
    /// persistence; open sessions keep running with the harness they
    /// started with. Disabled harnesses are ignored so the default never
    /// leaves the enabled set.
    pub(crate) fn set_default_agent(&mut self, agent: AgentKind, cx: &mut Context<Self>) {
        if !self.enabled_agents.contains(&agent) {
            return;
        }
        if self.default_agent == agent {
            return;
        }
        self.default_agent = agent;
        cx.notify();
    }

    /// Re-check git status right now instead of waiting for the next poll
    /// tick. Claims a fresh load id (invalidating any older in-flight
    /// load), clears the header to loading state, and applies the result
    /// only while still current.
    fn refresh_git_status(&mut self, cx: &mut Context<Self>) {
        let (generation, path) = self.git_poll.begin_check(self.working_directory.clone());
        self.git_poll.reset();
        cx.spawn(async move |this, cx| {
            let status = cx
                .background_spawn(async move { load_git_status(&path) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.git_poll.commit(generation, status) {
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Manual sync entry point shared by the command palette and the
    /// Settings Sync section. Guards an in-flight run and surfaces
    /// notifications; the silent scheduler goes through [`trigger_sync`](Self::trigger_sync)
    /// directly.
    pub(crate) fn request_sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sync_tracker.status() == SyncStatus::Syncing {
            window.push_notification("Portable sync is already running", cx);
        } else if self.data_root.is_none() {
            window.push_notification("Portable data is unavailable", cx);
        } else {
            window.push_notification("Syncing portable data…", cx);
            self.trigger_sync(cx);
        }
        cx.notify();
    }

    /// Persisted automatic-sync interval, re-read every scheduler tick so
    /// Settings edits re-arm the schedule within one poll. `None` (or zero)
    /// means Off.
    fn auto_sync_interval_minutes(&self) -> Option<u64> {
        if self.onboarding.is_some() {
            return None;
        }
        self.data_root
            .as_ref()
            .and_then(|root| DeviceStore::new(root).load().ok())
            .and_then(|state| state.sync_interval_minutes)
            .filter(|minutes| *minutes > 0)
    }

    /// Reload portable-backed projections (Review surfaces) after the files
    /// underneath them may have moved — a sync rebase or a Settings branch
    /// switch. Shared so both paths reload exactly the same set.
    pub(crate) fn reload_portable_projections(&mut self, cx: &mut Context<Self>) {
        self.relationships.update(cx, |view, cx| view.refresh(cx));
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Resources
        {
            self.resources.update(cx, |view, cx| view.refresh(cx));
        }
        self.home.update(cx, |view, cx| view.reload(cx));
        self.reload_recent_repositories();
        self.review.update(cx, |view, cx| view.activate(cx));
        for repository in self.inactive_repositories.values() {
            repository.review.update(cx, |view, cx| view.activate(cx));
        }
        cx.notify();
    }

    /// Run one portable sync off the main thread. Completion reloads the
    /// Review projection when the rebase may have moved files (including
    /// after an error, which can still leave working-tree changes behind)
    /// and repaints the status bar via the shared [`SyncTracker`].
    fn trigger_sync(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.data_root.clone() else {
            return;
        };
        // Manual runs reset the automatic schedule clock.
        self.last_sync_started = Instant::now();
        let tracker = self.sync_tracker.clone();
        let settings = self.settings.clone();
        cx.spawn(async move |this, cx| {
            let outcome = cx
                .background_executor()
                .spawn(async move { sync_portable_with_tracker(&root, &tracker) })
                .await;
            let reload = match &outcome {
                Ok(outcome) => outcome.reload_required,
                Err(_) => true,
            };
            let _ = this.update(cx, |this, cx| {
                if reload {
                    this.reload_portable_projections(cx);
                }
                // Repaint the Settings dialog if open so its status line
                // follows the run without polling.
                settings.update(cx, |_, cx| cx.notify());
                cx.notify();
            });
        })
        .detach();
    }

    fn render_command_bar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = cx.entity().downgrade();
        let confirm_workspace = workspace.clone();
        let query_workspace = workspace.clone();
        let attention_only = self.attention_only;
        let mut command = Command::new(&self.command_state)
            .placeholder(self.palette_mode.placeholder())
            .on_query(move |query, _window, cx| {
                let has_query = query_has_terms(query);
                let _ = query_workspace.update(cx, |this, cx| {
                    if this.palette_has_query != has_query {
                        this.palette_has_query = has_query;
                        cx.notify();
                    }
                });
            })
            .on_confirm(move |index, window, cx| {
                confirm_workspace
                    .update(cx, |this, cx| this.on_palette_confirm(index, window, cx))
                    .ok();
            })
            .on_cancel(move |window, cx| {
                workspace
                    .update(cx, |this, cx| this.close_command_palette(window, cx))
                    .ok();
            })
            .footer(|_, _, cx| {
                h_flex()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("↑↓ Navigate")
                    .child("Enter Select")
                    .child("Esc Close")
            })
            .empty(move |_, _, cx| {
                div()
                    .px_3()
                    .py_6()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if attention_only {
                        "No agents need attention"
                    } else {
                        "No matching commands"
                    })
            })
            .w_full();
        // Install exactly the model confirmations resolve against, so render
        // and confirm can never disagree about what a row means. Labels,
        // keywords, and icons all come from the model item. The model follows
        // the open mode, so each shortcut sees only its own rows.
        let activity = self.agent_activity.snapshot();
        let visible_repositories = if attention_only {
            self.recent_repositories
                .iter()
                .filter(|repository| {
                    self.active_space.matches(&repository.space)
                        && activity
                            .for_checkout(&repository.checkout_path)
                            .is_some_and(|agent| agent.state == ActivityState::NeedsAttention)
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            self.recent_repositories
                .iter()
                .filter(|repository| self.active_space.matches(&repository.space))
                .cloned()
                .collect::<Vec<_>>()
        };
        let mut open_checkouts = activity
            .iter()
            .filter(|(checkout, agent)| {
                agent.state != ActivityState::NoAgent
                    && (!attention_only || agent.state == ActivityState::NeedsAttention)
                    && !self
                        .recent_repositories
                        .iter()
                        .any(|repository| repository.checkout_path.as_path() == *checkout)
            })
            .map(|(checkout, _)| PaletteItem::open_checkout(checkout))
            .collect::<Vec<_>>();
        open_checkouts.sort_by(|left, right| left.label().cmp(right.label()));
        self.palette_model = if attention_only {
            vec![PaletteSection {
                heading: "Needs attention",
                items: visible_repositories
                    .iter()
                    .map(PaletteItem::switch_target)
                    .chain(open_checkouts)
                    .collect(),
            }]
        } else {
            let mut sections = palette_sections_for_mode(
                &visible_repositories,
                self.palette_mode,
                self.home_visible,
                self.palette_has_query,
            );
            if self.palette_mode == PaletteMode::Projects && !open_checkouts.is_empty() {
                sections.push(PaletteSection {
                    heading: "Open checkouts",
                    items: open_checkouts,
                });
            }
            sections
        };
        let current = self.current_repository.clone();
        let current_checkout = self.working_directory.clone();
        for section in &self.palette_model {
            command = command.group(
                CommandGroup::new().label(section.heading).items(
                    section
                        .items
                        .iter()
                        .map(|item| {
                            let mut keywords = item
                                .keywords()
                                .into_iter()
                                .map(str::to_owned)
                                .collect::<Vec<_>>();
                            let rendered = match item {
                                // Tabs with direct keybindings carry the real
                                // action: the row renders the binding hint for
                                // free, and confirming still runs
                                // `on_palette_confirm` afterwards (idempotent
                                // re-select of the same tab).
                                PaletteItem::Command(PaletteCommand::GoAgent) => CommandItem::new()
                                    .label(item.label())
                                    .icon(palette_icon(PaletteCommand::GoAgent))
                                    .action(Box::new(GoToAgent)),
                                PaletteItem::Command(PaletteCommand::GoEditor) => {
                                    CommandItem::new()
                                        .label(item.label())
                                        .icon(palette_icon(PaletteCommand::GoEditor))
                                        .action(Box::new(GoToEditor))
                                }
                                PaletteItem::Command(PaletteCommand::GoTerminal) => {
                                    CommandItem::new()
                                        .label(item.label())
                                        .icon(palette_icon(PaletteCommand::GoTerminal))
                                        .action(Box::new(GoToTerminal))
                                }
                                PaletteItem::Command(PaletteCommand::GoReview) => {
                                    CommandItem::new()
                                        .label(item.label())
                                        .icon(palette_icon(PaletteCommand::GoReview))
                                        .action(Box::new(GoToReview))
                                }
                                PaletteItem::Command(PaletteCommand::GoResources) => {
                                    CommandItem::new()
                                        .label(item.label())
                                        .icon(palette_icon(PaletteCommand::GoResources))
                                        .action(Box::new(GoToResources))
                                }
                                PaletteItem::Command(command) => CommandItem::new()
                                    .label(item.label())
                                    .icon(palette_icon(*command)),
                                PaletteItem::Tool(tool) => {
                                    CommandItem::new().label(item.label()).icon(tool.icon())
                                }
                                PaletteItem::SwitchRepository { key, .. } => {
                                    let label = item.label().to_owned();
                                    let repository = self
                                        .recent_repositories
                                        .iter()
                                        .find(|repository| repository.key == *key);
                                    let agent = repository
                                        .and_then(|repository| {
                                            activity.for_checkout(&repository.checkout_path)
                                        })
                                        .cloned();
                                    let working = agent
                                        .as_ref()
                                        .is_some_and(|agent| agent.state == ActivityState::Working);
                                    let (provider, state, color) = match agent {
                                        Some(agent) => {
                                            keywords.push(agent.agent.label().to_owned());
                                            keywords.push(agent.state.label().to_owned());
                                            (
                                                agent.agent.label().to_owned(),
                                                agent.state.label().to_owned(),
                                                activity_state_color(agent.state),
                                            )
                                        }
                                        None => (
                                            "Agent".to_owned(),
                                            ActivityState::NoAgent.label().to_owned(),
                                            activity_state_color(ActivityState::NoAgent),
                                        ),
                                    };
                                    let status = format!("{provider} · {state}");
                                    let is_current = current.as_deref() == Some(key.as_str());
                                    // Custom content on purpose: the colored provider/state
                                    // text, the working spinner and the current dot are this
                                    // palette's live status presentation. gpui-kit's command
                                    // row cache rejects any model containing custom content
                                    // (`same_layout`), so the projects palette keeps
                                    // remeasuring; that is the accepted trade for the status
                                    // display.
                                    CommandItem::new().label(label.clone()).child(move |_, _| {
                                        h_flex()
                                            .flex_1()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Folder)
                                                    .size(px(16.))
                                                    .text_color(rgb(0x858989)),
                                            )
                                            .child(label.clone())
                                            .when(is_current, |row| {
                                                row.child(
                                                    div()
                                                        .size(px(6.))
                                                        .flex_shrink_0()
                                                        .rounded_full()
                                                        .bg(rgb(0x4ade80)),
                                                )
                                            })
                                            .child(
                                                h_flex()
                                                    .ml_auto()
                                                    .gap_1()
                                                    .text_xs()
                                                    .text_color(rgb(color))
                                                    .when(working, |this| {
                                                        this.child(
                                                            Spinner::new()
                                                                .with_size(px(12.))
                                                                .color(rgb(color).into()),
                                                        )
                                                    })
                                                    .child(status.clone()),
                                            )
                                    })
                                }
                                PaletteItem::OpenCheckout { checkout_path, .. } => {
                                    let label = item.label().to_owned();
                                    let agent = activity.for_checkout(checkout_path).cloned();
                                    let working = agent
                                        .as_ref()
                                        .is_some_and(|agent| agent.state == ActivityState::Working);
                                    let (provider, state, color) = match agent {
                                        Some(agent) => {
                                            keywords.push(agent.agent.label().to_owned());
                                            keywords.push(agent.state.label().to_owned());
                                            (
                                                agent.agent.label().to_owned(),
                                                agent.state.label().to_owned(),
                                                activity_state_color(agent.state),
                                            )
                                        }
                                        None => (
                                            "Agent".to_owned(),
                                            ActivityState::NoAgent.label().to_owned(),
                                            activity_state_color(ActivityState::NoAgent),
                                        ),
                                    };
                                    let status = format!("{provider} · {state}");
                                    let is_current =
                                        current_checkout.as_path() == checkout_path.as_path();
                                    // Custom child for the same live status as the switch
                                    // targets above.
                                    CommandItem::new().label(label.clone()).child(move |_, _| {
                                        h_flex()
                                            .flex_1()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Folder)
                                                    .size(px(16.))
                                                    .text_color(rgb(0x858989)),
                                            )
                                            .child(label.clone())
                                            .when(is_current, |row| {
                                                row.child(
                                                    div()
                                                        .size(px(6.))
                                                        .flex_shrink_0()
                                                        .rounded_full()
                                                        .bg(rgb(0x4ade80)),
                                                )
                                            })
                                            .child(
                                                h_flex()
                                                    .ml_auto()
                                                    .gap_1()
                                                    .text_xs()
                                                    .text_color(rgb(color))
                                                    .when(working, |this| {
                                                        this.child(
                                                            Spinner::new()
                                                                .with_size(px(12.))
                                                                .color(rgb(color).into()),
                                                        )
                                                    })
                                                    .child(status.clone()),
                                            )
                                    })
                                }
                            };
                            rendered.keywords(keywords)
                        })
                        .collect::<Vec<_>>(),
                ),
            );
        }
        command
    }

    fn render_active_content(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.git_page_origin.is_none()
            && self
                .git_view
                .as_ref()
                .is_some_and(|git| git.read(cx).busy())
        {
            let message = self
                .git_view
                .as_ref()
                .and_then(|git| git.read(cx).operation_notice())
                .map(|(message, _)| message.to_owned())
                .unwrap_or_else(|| "Working…".into());
            return v_flex()
                .id("git-header-operation")
                .size_full()
                .items_center()
                .justify_center()
                .track_focus(&self.git_header_busy_focus)
                .child(div().text_sm().child(message))
                .into_any_element();
        }
        if self.git_page_origin.is_some() {
            return self
                .git_view
                .as_ref()
                .map(|git| git.clone().into_any_element())
                .unwrap_or_else(|| div().size_full().into_any_element());
        }
        if self.relationships_visible {
            return self.relationships.clone().into_any_element();
        }
        if self.home_visible {
            return self.home.clone().into_any_element();
        }
        if self.active_tab == WorkspaceTab::Resources {
            return self.resources.clone().into_any_element();
        }
        if self.active_tab == WorkspaceTab::Review {
            return self.review.clone().into_any_element();
        }
        if self.active_tab == WorkspaceTab::Agent {
            return self.render_agent_sessions(cx);
        }
        if self.active_tab == WorkspaceTab::Editor
            && self.editor_instance == Some(EditorChoice::BuiltIn)
        {
            return self
                .native_editor
                .as_ref()
                .map(|editor| editor.clone().into_any_element())
                .unwrap_or_else(|| div().size_full().into_any_element());
        }
        match self
            .tabs
            .get(self.active_tab as usize)
            .and_then(|tab| tab.clone())
        {
            Some(pane) => pane.into_any_element(),
            None => div().size_full().into_any_element(),
        }
    }

    fn navigation_context(&self, cx: &App) -> NavigationContext {
        if self.git_page_origin.is_some() || self.git_header_busy {
            return NavigationContext::Git;
        }
        if self.relationships_visible {
            return NavigationContext::Relationships;
        }
        if self.home_visible {
            if self.home.read(cx).is_artifacts_page() {
                NavigationContext::Artifacts
            } else {
                NavigationContext::Home
            }
        } else {
            NavigationContext::Workspace
        }
    }

    fn navigation_resource_state(&self, cx: &App) -> navigation::ResourceState {
        if self.git_page_origin.is_some() || self.git_header_busy || self.relationships_visible {
            return navigation::ResourceState::default();
        }
        if self.home_visible {
            return if self.home.read(cx).is_artifacts_page() {
                self.home.read(cx).artifacts_navigation_state(cx)
            } else {
                navigation::ResourceState::default()
            };
        }
        let resource = if self.active_tab == WorkspaceTab::Resources {
            self.resources.read(cx).navigation_state()
        } else {
            navigation::ResourceState::default()
        };
        navigation::ResourceState {
            active_tab: match self.active_tab {
                WorkspaceTab::Agent => Some(navigation::Tab::Agent),
                WorkspaceTab::Editor => Some(navigation::Tab::Editor),
                WorkspaceTab::Terminal => Some(navigation::Tab::Terminal),
                WorkspaceTab::Review => Some(navigation::Tab::Review),
                WorkspaceTab::Resources => Some(navigation::Tab::Resources),
            },
            built_in_editor: self.editor_instance == Some(EditorChoice::BuiltIn),
            markdown_file: self
                .native_editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).supports_markdown_preview()),
            ..resource
        }
    }

    fn navigation_panes(&self, cx: &App) -> Vec<(&'static str, FocusHandle)> {
        if self.git_header_busy && self.git_page_origin.is_none() {
            return vec![("Git operation", self.git_header_busy_focus.clone())];
        }
        if self.git_page_origin.is_some() {
            return self
                .git_view
                .as_ref()
                .map(|git| vec![("Git", git.focus_handle(cx))])
                .unwrap_or_default();
        }
        if self.relationships_visible {
            return vec![(
                "Relationships",
                self.relationships.read(cx).focus_handle.clone(),
            )];
        }
        if self.home_visible {
            return if self.home.read(cx).is_artifacts_page() {
                self.home.read(cx).artifacts_navigation_panes(cx)
            } else {
                vec![("Home", self.home.read(cx).focus_handle.clone())]
            };
        }
        match self.active_tab {
            WorkspaceTab::Agent => {
                let mut panes = vec![("Sessions", self.agent_sidebar_focus.clone())];
                if let Some(Some(pane)) = self.tabs.get(WorkspaceTab::Agent as usize) {
                    panes.push(("Agent", pane.read(cx).focus_handle.clone()));
                }
                panes
            }
            WorkspaceTab::Resources => self.resources.read(cx).navigation_panes(cx),
            WorkspaceTab::Review => self.review.read(cx).navigation_panes(cx),
            WorkspaceTab::Editor if self.editor_instance == Some(EditorChoice::BuiltIn) => self
                .native_editor
                .as_ref()
                .map(|editor| editor.read(cx).navigation_panes(cx))
                .unwrap_or_default(),
            WorkspaceTab::Editor | WorkspaceTab::Terminal => self
                .tabs
                .get(self.active_tab as usize)
                .and_then(|pane| pane.as_ref())
                .map(|pane| vec![(self.active_tab.label(), pane.read(cx).focus_handle.clone())])
                .unwrap_or_default(),
        }
    }

    fn open_navigation(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Dialog and sheet layers own their own keyboard traps. Do not open a
        // workspace HUD behind either, and close any stale HUD before they can
        // receive a keystroke.
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) || self.command_open {
            self.close_navigation(cx);
            return;
        }
        let panes = self.navigation_panes(cx);
        self.navigation_pane = panes
            .iter()
            // Detail containers contain their comments/outline descendants;
            // prefer the deepest rendered target so reopening the HUD names
            // the region the pointer or keyboard actually left focused.
            .rposition(|(_, focus)| focus.contains_focused(window, cx))
            .unwrap_or(0);
        self.navigation_open = true;
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Editor
            && let Some(editor) = &self.native_editor
        {
            editor.update(cx, |editor, cx| editor.set_navigation_active(true, cx));
        }
        self.sync_tutorial_navigation(cx);
        // Start item cursors where `j`/`k` should repeat from: the active
        // session in the sidebar, the first Home card on the dashboard.
        // Artifact and review lists reuse their existing selection.
        if self.home_visible
            && !self.relationships_visible
            && !self.home.read(cx).is_artifacts_page()
        {
            self.home
                .update(cx, |view, cx| view.set_navigation_active(true, cx));
            self.session_cursor = None;
        } else if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Agent
            && self.navigation_pane == 0
        {
            let agent_index = WorkspaceTab::Agent as usize;
            let active = self.tabs[agent_index]
                .as_ref()
                .and_then(|pane| pane.read(cx).launch_id());
            let active_key = active
                .and_then(|id| self.open_sessions.get(&id))
                .and_then(|session| session.key.clone());
            self.init_session_cursor(active, active_key);
            self.home
                .update(cx, |view, cx| view.set_navigation_active(false, cx));
        } else {
            self.session_cursor = None;
            self.home
                .update(cx, |view, cx| view.set_navigation_active(false, cx));
        }
        cx.notify();
    }

    fn close_navigation(&mut self, cx: &mut Context<Self>) {
        let was_open = self.navigation_open;
        if was_open {
            self.navigation_open = false;
            if let Some(editor) = &self.native_editor {
                editor.update(cx, |editor, cx| editor.set_navigation_active(false, cx));
            }
            self.sync_tutorial_navigation(cx);
        }
        if self.session_cursor.is_some() {
            self.session_cursor = None;
        }
        // Clearing the Home highlight is cheap and keeps a stale cursor from
        // reappearing on the next visit; `notify` only when something changed.
        let home_cleared = self.home.update(cx, |view, cx| {
            if view.take_navigation_cursor().is_some() {
                cx.notify();
                true
            } else {
                false
            }
        });
        if was_open || home_cleared {
            cx.notify();
        }
    }

    fn move_navigation_focus(&mut self, right: bool, window: &mut Window, cx: &mut Context<Self>) {
        let panes = self.navigation_panes(cx);
        self.navigation_pane = navigation::move_index(self.navigation_pane, panes.len(), right);
        if let Some((_, focus)) = panes.get(self.navigation_pane) {
            focus.focus(window, cx);
        }
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Review
        {
            self.review.update(cx, |review, cx| {
                review.focus_navigation_pane(self.navigation_pane, window, cx)
            });
        }
        cx.notify();
    }

    /// Navigation-mode `Tab`/`Shift+Tab`: move keyboard focus to the next or
    /// previous focusable component and keep the mode open so traversal can
    /// repeat, like `h`/`l`. This is the workspace's only component
    /// traversal: outside navigation mode the key belongs to the focused
    /// component (see `WORKSPACE_KEY_CONTEXT`), so terminals forward it to
    /// the pty and inputs keep it.
    fn move_navigation_component_focus(
        &mut self,
        next: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if next {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        // Traversal can cross a pane boundary (or leave every pane for a
        // titlebar control). Re-anchor the HUD's pane label — and `h`/`l`'s
        // starting point — whenever focus landed inside a pane.
        let panes = self.navigation_panes(cx);
        if let Some(index) = panes
            .iter()
            .rposition(|(_, focus)| focus.contains_focused(window, cx))
        {
            self.navigation_pane = index;
        }
        cx.notify();
    }

    /// Move the item cursor within the focused pane for navigation-mode
    /// `j`/`k`. Keeps the mode open for repeats; locations without a list
    /// consume the key and stay open. Clamps at the ends like pane movement.
    fn move_navigation_item(&mut self, down: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_page_origin.is_some() || self.git_header_busy || self.relationships_visible {
            return;
        }
        if self.home_visible {
            if self.home.read(cx).is_artifacts_page() {
                self.home
                    .update(cx, |view, cx| view.artifacts_move_selection(down, cx));
            } else {
                self.home
                    .update(cx, |view, cx| view.move_navigation_cursor(down, cx));
            }
        } else {
            match self.active_tab {
                WorkspaceTab::Agent => {
                    if self.navigation_pane == 0 {
                        self.move_session_cursor(down);
                    }
                }
                WorkspaceTab::Resources => {
                    if self.navigation_pane == 0 {
                        self.resources
                            .update(cx, |view, cx| view.move_selection(down, cx));
                    }
                }
                WorkspaceTab::Review => match self.navigation_pane {
                    0 => self
                        .review
                        .update(cx, |view, cx| view.move_file_selection(down, cx)),
                    1 => self
                        .review
                        .update(cx, |view, cx| view.move_diff_scroll(down, cx)),
                    _ => {}
                },
                WorkspaceTab::Editor if self.editor_instance == Some(EditorChoice::BuiltIn) => {
                    if let Some(editor) = &self.native_editor {
                        editor.update(cx, |editor, cx| {
                            editor.move_navigation_item(self.navigation_pane, down, window, cx)
                        });
                    }
                }
                WorkspaceTab::Editor | WorkspaceTab::Terminal => {}
            }
        }
        cx.notify();
    }

    /// Navigation-mode `Enter`: open the `j`/`k` cursor item when one is
    /// active (sessions sidebar, Home cards), otherwise keep the focused pane
    /// like before. Folder toggles keep navigation open; other activations
    /// return to normal mode.
    fn accept_navigation_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.git_page_origin.is_some() {
            self.close_navigation(cx);
            return;
        }
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Agent
            && self.navigation_pane == 0
            && self.session_cursor.is_some()
            && self.activate_session_cursor(window, cx)
        {
            return;
        }
        if self.home_visible
            && !self.relationships_visible
            && !self.home.read(cx).is_artifacts_page()
        {
            let activated = self
                .home
                .update(cx, |view, cx| view.activate_navigation_cursor(window, cx));
            if activated {
                self.close_navigation(cx);
                return;
            }
        }
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && !self.relationships_visible
            && self.active_tab == WorkspaceTab::Editor
            && self.editor_instance == Some(EditorChoice::BuiltIn)
            && let Some(editor) = &self.native_editor
            && editor.read(cx).navigation_in_tree(self.navigation_pane)
            && editor.update(cx, |editor, cx| {
                editor.activate_navigation_cursor(window, cx)
            })
        {
            return;
        }
        self.close_navigation(cx);
    }

    fn run_navigation_command(
        &mut self,
        command: NavigationCommand,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Availability was checked to render the HUD, but actions validate it
        // again here so a refresh/save cannot make a stale row unsafe.
        // Close through the shared path so item cursors clear with the mode.
        self.close_navigation(cx);
        match command {
            NavigationCommand::AddRepository => self.open_add_repository(window, cx),
            NavigationCommand::BrowseArtifacts => self.browse_artifacts(window, cx),
            NavigationCommand::SwitchSpace => self.toggle_space_menu(window, cx),
            NavigationCommand::Agent => self.select_tab(WorkspaceTab::Agent as usize, window, cx),
            NavigationCommand::Editor => self.select_tab(WorkspaceTab::Editor as usize, window, cx),
            NavigationCommand::Terminal => {
                self.select_tab(WorkspaceTab::Terminal as usize, window, cx)
            }
            NavigationCommand::Review => self.select_tab(WorkspaceTab::Review as usize, window, cx),
            NavigationCommand::Resources => {
                self.select_tab(WorkspaceTab::Resources as usize, window, cx)
            }
            NavigationCommand::GitChanges => self.open_git_page(window, cx),
            NavigationCommand::Home => self.go_home(window, cx),
            NavigationCommand::Back => {
                if self.git_page_origin.is_some() {
                    self.back_from_git(window, cx);
                    return;
                }
                if self.relationships_visible {
                    self.back_from_relationships(window, cx);
                    return;
                }
                if self.home_visible
                    && (self.home.read(cx).is_projects_page()
                        || self.home.read(cx).is_pull_requests_page()
                        || self.home.read(cx).is_todos_page()
                        || self.home.read(cx).is_reading_page())
                {
                    self.go_home(window, cx);
                } else {
                    self.go_back_from_artifacts(window, cx);
                }
            }
            NavigationCommand::ToggleMarkdownPreview => {
                if self.editor_instance == Some(EditorChoice::BuiltIn)
                    && let Some(editor) = &self.native_editor
                {
                    editor.update(cx, |editor, cx| editor.toggle_markdown_preview(window, cx));
                }
            }
            NavigationCommand::NewSession => self.prompt_new_agent_session(window, cx),
            command @ (NavigationCommand::OpenFile
            | NavigationCommand::SearchProject
            | NavigationCommand::SwitchBuffer) => {
                if self.editor_instance == Some(EditorChoice::BuiltIn) {
                    self.select_tab(WorkspaceTab::Editor as usize, window, cx);
                    if let Some(editor) = self.native_editor.as_ref() {
                        editor.update(cx, |editor, cx| match command {
                            NavigationCommand::SearchProject => editor.open_live_grep(window, cx),
                            NavigationCommand::SwitchBuffer => {
                                editor.open_buffer_finder(window, cx)
                            }
                            _ => editor.open_file_finder(window, cx),
                        });
                    }
                }
            }
            command @ (NavigationCommand::EditMarkdown
            | NavigationCommand::AddComment
            | NavigationCommand::SaveDraft
            | NavigationCommand::CancelDraft) => {
                if self.home_visible && self.home.read(cx).is_artifacts_page() {
                    self.home.update(cx, |view, cx| {
                        view.run_artifact_command(command, window, cx)
                    });
                } else if !self.home_visible && self.active_tab == WorkspaceTab::Resources {
                    self.resources.update(cx, |view, cx| match command {
                        NavigationCommand::EditMarkdown => view.begin_markdown_edit(window, cx),
                        NavigationCommand::AddComment => view.begin_comment(window, cx),
                        NavigationCommand::SaveDraft => view.save_draft(window, cx),
                        NavigationCommand::CancelDraft => view.cancel_draft(window, cx),
                        _ => unreachable!(),
                    });
                }
                cx.notify();
            }
        }
    }

    /// Whether navigation-mode `j`/`k` moves something at the current
    /// location: Home cards, the Artifacts list, the sessions sidebar, the
    /// Resources sidebar, Review files/diff, or the built-in editor. Other panes consume the keys
    /// and stay open.
    fn navigation_item_available(&self, cx: &App) -> bool {
        if self.git_page_origin.is_some() || self.git_header_busy || self.relationships_visible {
            return false;
        }
        if self.home_visible {
            if self.home.read(cx).is_artifacts_page() {
                return true;
            }
            return self.home.read(cx).has_navigation_targets();
        }
        match self.active_tab {
            WorkspaceTab::Agent => {
                self.navigation_pane == 0 && !self.session_nav_order().is_empty()
            }
            WorkspaceTab::Resources => self.navigation_pane == 0,
            WorkspaceTab::Review => matches!(self.navigation_pane, 0 | 1),
            WorkspaceTab::Editor if self.editor_instance == Some(EditorChoice::BuiltIn) => {
                self.native_editor.as_ref().is_some_and(|editor| {
                    let editor = editor.read(cx);
                    editor.navigation_in_tree(self.navigation_pane)
                        || editor
                            .navigation_panes(cx)
                            .get(self.navigation_pane)
                            .is_some_and(|(label, _)| *label == "Editor")
                })
            }
            WorkspaceTab::Editor | WorkspaceTab::Terminal => false,
        }
    }

    /// Whether navigation-mode `Enter` opens the `j`/`k` cursor item instead
    /// of only keeping the focused pane: the editor tree, sessions sidebar and Home cards.
    /// Artifact and review lists apply their selection live, so `Enter` there
    /// only keeps focus.
    fn navigation_enter_opens(&self, cx: &App) -> bool {
        if self.git_page_origin.is_some() || self.git_header_busy || self.relationships_visible {
            return false;
        }
        if self.git_page_origin.is_none()
            && !self.git_header_busy
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Agent
            && self.navigation_pane == 0
            && self.session_cursor.is_some()
        {
            return true;
        }
        if self.home_visible
            && !self.relationships_visible
            && !self.home.read(cx).is_artifacts_page()
        {
            return self.home.read(cx).navigation_cursor_active();
        }
        self.git_page_origin.is_none()
            && !self.home_visible
            && self.active_tab == WorkspaceTab::Editor
            && self.editor_instance == Some(EditorChoice::BuiltIn)
            && self
                .native_editor
                .as_ref()
                .is_some_and(|editor| editor.read(cx).navigation_in_tree(self.navigation_pane))
    }

    /// The mode toggle: `cmd-j` on macOS, `ctrl-j` on Linux/Windows.
    /// (`cmd-m`/`super-m` previously collided with system Minimize —
    /// `Cmd+M` on macOS, `Win+M` minimize-all on Windows — so the toggle
    /// moved to `J`, which has no OS reservation. `J` also echoes the
    /// in-mode movement keys `j`/`k`, while the required modifier keeps it
    /// distinct from plain-`j` move-down.) Plain `m` stays free as the Edit
    /// Markdown action key, `cmd-m` returns to the system (Minimize on
    /// macOS), and `ctrl-m` returns to terminals as an Enter alias.
    pub(crate) fn is_trigger(keystroke: &gpui_kit::Keystroke) -> bool {
        let modifiers = &keystroke.modifiers;
        keystroke.key.eq_ignore_ascii_case("j")
            && is_primary_modifier(modifiers.platform, modifiers.control)
            && !modifiers.alt
            && !modifiers.shift
            && !modifiers.function
    }

    fn navigation_input(keystroke: &gpui_kit::Keystroke) -> NavigationInput {
        let modifiers = &keystroke.modifiers;
        let key = keystroke.key.to_ascii_lowercase();
        let trigger = Self::is_trigger(keystroke);
        if trigger {
            return NavigationInput::Trigger;
        }
        // Tab/Shift+Tab are the mode's component traversal (see
        // `move_navigation_component_focus`); they are the only modified
        // keystrokes the mode acts on, every other one stays consumed below.
        if key == "tab" {
            let shift_only = modifiers.shift
                && !modifiers.control
                && !modifiers.alt
                && !modifiers.platform
                && !modifiers.function;
            return if shift_only {
                NavigationInput::BackTab
            } else if modifiers.modified() {
                NavigationInput::Modified
            } else {
                NavigationInput::Tab
            };
        }
        if modifiers.modified() {
            return NavigationInput::Modified;
        }
        match key.as_str() {
            "escape" => NavigationInput::Escape,
            "enter" => NavigationInput::Enter,
            // GPUI reports Space by name rather than as a one-character key.
            "space" => NavigationInput::Key(' '),
            "h" | "left" | "arrowleft" => NavigationInput::Left,
            "l" | "right" | "arrowright" => NavigationInput::Right,
            "j" | "down" | "arrowdown" => NavigationInput::Down,
            "k" | "up" | "arrowup" => NavigationInput::Up,
            _ => {
                let mut chars = keystroke.key.chars();
                match (chars.next(), chars.next()) {
                    (Some(key), None) => NavigationInput::Key(key),
                    _ => NavigationInput::Modified,
                }
            }
        }
    }

    fn on_navigation_keystroke(
        &mut self,
        event: &KeystrokeEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.onboarding.is_some() {
            return;
        }
        // Quit stays live everywhere, including navigation mode and held-key
        // repeats: close the HUD (if open) and let the keystroke continue to
        // normal dispatch, which quits via the global OS-primary binding.
        // Must run before the claimed-key rearm below, or repeats would stay
        // swallowed.
        if is_quit_shortcut(
            &event.keystroke.key,
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.alt,
        ) {
            if self.navigation_open {
                self.close_navigation(cx);
            }
            return;
        }
        let key = event.keystroke.key.to_ascii_lowercase();
        // `KeystrokeEvent` intentionally omits raw `is_held`; retain each
        // claimed key until its real KeyUp event so physical repeats cannot
        // toggle a newly changed navigation state or leak into an overlay.
        // Escape hatch for a lost key-up: if any modifier moved since the
        // claim, the previous hold provably ended, so a re-pressed trigger
        // is honored as fresh instead of staying wedged. Trigger-only, and
        // only with modifier movement — a genuine held repeat (nothing
        // moved) stays swallowed, as does any marked non-trigger key.
        if self.navigation_claimed_keys.contains_key(&key) {
            let rearm = self.navigation_claimed_keys[&key] && Self::is_trigger(&event.keystroke);
            if !rearm {
                window.prevent_default();
                cx.stop_propagation();
                return;
            }
            self.navigation_claimed_keys.remove(&key);
        }
        // A dialog/sheet is rendered above the workspace and must retain its
        // own keyboard trap. It can have appeared through another workflow
        // while the HUD was open, so clear the stale mode before considering
        // the current key and let that key reach the overlay.
        if window.has_active_dialog(cx) || window.has_active_sheet(cx) {
            self.close_navigation(cx);
            // Claim the chord so held repeats cannot open the HUD after
            // the overlay closes; leave ordinary dialog typing alone.
            if Self::is_trigger(&event.keystroke) {
                self.navigation_claimed_keys.insert(key, false);
            }
            return;
        }
        // The two palette shortcuts stay live inside navigation mode (quit is
        // handled above and also stays live):
        // opening another overlay clears the mode (same rule as dialogs
        // above) and the keystroke continues to normal dispatch, which
        // opens the palette. Every other modified keystroke is still
        // consumed below, so nothing can edit while navigating.
        if self.navigation_open
            && palette_mode_for_shortcut(
                &event.keystroke.key,
                event.keystroke.modifiers.platform,
                event.keystroke.modifiers.control,
                event.keystroke.modifiers.alt,
            )
            .is_some()
        {
            self.close_navigation(cx);
            return;
        }
        let input = Self::navigation_input(&event.keystroke);
        let decision = navigation::decide(
            self.navigation_open,
            input,
            self.navigation_context(cx),
            self.navigation_resource_state(cx),
        );
        if decision == NavigationDecision::Ignore {
            return;
        }
        self.navigation_claimed_keys.insert(key, false);
        window.prevent_default();
        cx.stop_propagation();
        match decision {
            NavigationDecision::Open => self.open_navigation(window, cx),
            NavigationDecision::Close => self.close_navigation(cx),
            NavigationDecision::AcceptFocus => self.accept_navigation_focus(window, cx),
            NavigationDecision::FocusLeft => self.move_navigation_focus(false, window, cx),
            NavigationDecision::FocusRight => self.move_navigation_focus(true, window, cx),
            NavigationDecision::FocusNext => self.move_navigation_component_focus(true, window, cx),
            NavigationDecision::FocusPrevious => {
                self.move_navigation_component_focus(false, window, cx)
            }
            NavigationDecision::PrevItem => self.move_navigation_item(false, window, cx),
            NavigationDecision::NextItem => self.move_navigation_item(true, window, cx),
            NavigationDecision::Execute(command) => {
                self.run_navigation_command(command, window, cx)
            }
            NavigationDecision::Consume | NavigationDecision::Ignore => {}
        }
    }

    fn render_navigation_hud(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let rows = navigation::rows(
            self.navigation_context(cx),
            self.navigation_resource_state(cx),
        );
        let panes = self.navigation_panes(cx);
        let viewport = window.viewport_size();
        let hud_width = (f32::from(viewport.width) - 32.).clamp(200., 300.);
        let hud_height = (f32::from(viewport.height) - WORKSPACE_HEADER_HEIGHT - 36.).max(120.);
        let content_width = hud_width - 26.;
        let binding = |keys: String, label: String| {
            h_flex()
                .w_full()
                .min_h(px(28.))
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(label),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .min_w(px(24.))
                        .px_1p5()
                        .py(px(2.))
                        .rounded_sm()
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().accent)
                        .text_center()
                        .text_xs()
                        .text_color(cx.theme().accent_foreground)
                        .child(keys),
                )
        };
        let action = |row: &navigation::Row| {
            binding(
                if row.key == ' ' {
                    "Space".to_owned()
                } else {
                    row.key_label()
                },
                if row.command == NavigationCommand::GitChanges {
                    "Git".to_owned()
                } else {
                    row.label.to_owned()
                },
            )
        };
        let section = |title: &str, content: gpui_kit::Div| {
            v_flex()
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(title.to_owned()),
                )
                .child(content)
        };
        let mut sections = v_flex().gap_3().w(px(content_width));
        // Keep presentation independent of key resolution: every policy row
        // appears once, while contextual actions lead and shared controls trail.
        let contextual_groups = if self.navigation_context(cx) == NavigationContext::Workspace
            && self.active_tab == WorkspaceTab::Agent
        {
            ["Agent", "Resource", "Editor", "Home"]
        } else {
            ["Resource", "Editor", "Agent", "Home"]
        };
        for group in contextual_groups {
            let contextual: Vec<_> = rows.iter().filter(|row| row.group == group).collect();
            if !contextual.is_empty() {
                sections = sections.child(section(
                    &format!("{group} actions"),
                    v_flex()
                        .gap_1()
                        .children(contextual.into_iter().map(action)),
                ));
            }
        }
        let is_tab = |command| {
            matches!(
                command,
                NavigationCommand::Agent
                    | NavigationCommand::Editor
                    | NavigationCommand::Terminal
                    | NavigationCommand::Review
                    | NavigationCommand::Resources
            )
        };
        let tabs: Vec<_> = rows.iter().filter(|row| is_tab(row.command)).collect();
        if !tabs.is_empty() {
            sections = sections.child(section(
                "Switch tab",
                v_flex().gap_1().children(tabs.into_iter().map(action)),
            ));
        }
        let mut common = v_flex().gap_1().children(
            rows.iter()
                .filter(|row| row.group == "Navigate" && !is_tab(row.command))
                .map(action),
        );
        common = common.child(binding("Tab / ⇧Tab".to_owned(), "Cycle focus".to_owned()));
        if panes.len() > 1 {
            common = common.child(binding("h / l".to_owned(), "Change pane".to_owned()));
        }
        if self.navigation_item_available(cx) {
            common = common.child(binding("j / k".to_owned(), "Move down / up".to_owned()));
        }
        common = common
            .child(binding(
                "Enter".to_owned(),
                if self.navigation_enter_opens(cx) {
                    "Open / toggle"
                } else {
                    "Keep focus"
                }
                .to_owned(),
            ))
            .child(binding(
                if cfg!(target_os = "macos") {
                    "Esc / ⌘J"
                } else {
                    "Esc / Ctrl+J"
                }
                .to_owned(),
                "Close".to_owned(),
            ));
        sections = sections.child(section("Common", common));
        div()
            .absolute()
            .bottom(px(12.))
            .right(px(12.))
            .child(
                v_flex()
                    .w(px(hud_width))
                    .max_h(px(hud_height))
                    .overflow_y_scrollbar()
                    .p_3()
                    .gap_3()
                    .rounded_md()
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().background)
                    .shadow_lg()
                    .child(sections),
            )
            .into_any_element()
    }

    fn render_navigation_indicator(&self) -> AnyElement {
        h_flex()
            .flex_none()
            .w(px(104.))
            .items_center()
            // The wider palette covers part of this slot. Keep its layout
            // space while hiding the whole badge, including its border.
            .when(self.command_open, |indicator| indicator.invisible())
            .child(
                h_flex()
                    .flex_none()
                    .items_center()
                    .gap_1p5()
                    .h(px(28.))
                    .px_2()
                    .rounded_full()
                    .border_1()
                    .border_color(rgb(0x252828))
                    .text_xs()
                    .text_color(rgb(0x858989))
                    .when(self.navigation_open, |this| {
                        this.bg(rgb(0x211609))
                            .border_color(rgb(0x713f12))
                            .text_color(rgb(0xf59e0b))
                    })
                    .child(Icon::new(gpui_kit::assets::IconName::Keyboard).size(px(14.)))
                    .child(if self.navigation_open {
                        "Navigation"
                    } else if cfg!(target_os = "macos") {
                        "⌘J"
                    } else {
                        "Ctrl+J"
                    }),
            )
            .into_any_element()
    }

    fn render_git_status_trigger(&self, status: &GitStatus, cx: &mut Context<Self>) -> AnyElement {
        div()
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.git_picker_origin_focus = this.focused_content_handle(window, cx);
            }))
            .when_some(self.git_view.as_ref(), |this, git| {
                this.child(git.update(cx, |git, cx| git.render_branch_picker(status, cx)))
            })
            .into_any_element()
    }
}

/// Leading glyph per palette command. A command with a direct keybinding
/// additionally carries its real GPUI `Action` on the row via
/// `CommandItem::action`, which renders the binding hint for free; `on_confirm` still resolves it afterwards, idempotently.
fn palette_icon(command: PaletteCommand) -> IconName {
    match command {
        PaletteCommand::GoAgent => IconName::Bot,
        PaletteCommand::GoEditor => IconName::FileText,
        PaletteCommand::GoTerminal => IconName::SquareTerminal,
        PaletteCommand::GoReview => IconName::Eye,
        PaletteCommand::GoResources => IconName::CircleCheck,
        PaletteCommand::OpenGit => IconName::Github,
        PaletteCommand::GoHome => IconName::LayoutDashboard,
        PaletteCommand::BrowseArtifacts => IconName::BookOpen,
        PaletteCommand::RepositoryRelationships => IconName::LayoutDashboard,
        PaletteCommand::AddRepository => IconName::Plus,
        PaletteCommand::OpenSettings => IconName::Settings,
        PaletteCommand::EditorGoBack => IconName::ArrowLeft,
        PaletteCommand::SyncPortable => IconName::RotateCw,
    }
}

fn activity_state_color(state: ActivityState) -> u32 {
    match state {
        ActivityState::Starting | ActivityState::Idle | ActivityState::NoAgent => 0x858989,
        ActivityState::Working => 0x60a5fa,
        ActivityState::NeedsAttention => 0xf59e0b,
        ActivityState::Finished => 0x4ade80,
        ActivityState::Unavailable => 0x9ca3af,
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(onboarding) = &self.onboarding {
            return div()
                .size_full()
                .child(onboarding.clone())
                .into_any_element();
        }
        if !self.home_visible && !self.relationships_visible {
            self.ensure_git_view(window, cx);
        }
        self.sync_activity_visibility(window, cx);
        let active_index = self.active_tab as usize;
        self.sync_git_view(cx);
        let git_mode = self
            .git_page_origin
            .as_ref()
            .and_then(|_| self.git_view.as_ref().map(|git| git.read(cx).mode()));
        let header_tab_index = git_mode.map_or(active_index, |mode| mode as usize);
        let header_tabs: Vec<_> = if git_mode.is_some() {
            GitMode::ALL.into_iter().map(GitMode::label).collect()
        } else {
            WorkspaceTab::ALL
                .into_iter()
                .map(WorkspaceTab::label)
                .collect()
        };
        let active_content = self.render_active_content(cx);
        // Resources and Artifacts full-pages share the repository workspace
        // chrome: title and back navigation live here, not in a second row.
        // The portable git status is hidden on these pages and the lists
        // stay live through their active polls.
        let is_artifacts = self.home_visible
            && !self.relationships_visible
            && self.home.read(cx).is_artifacts_page();
        let is_projects = self.home_visible
            && !self.relationships_visible
            && self.home.read(cx).is_projects_page();
        let artifacts_back = if is_artifacts {
            self.artifacts_back_label()
        } else {
            SharedString::from("")
        };
        let is_pull_requests = self.home_visible
            && !self.relationships_visible
            && self.home.read(cx).is_pull_requests_page();
        let is_todos =
            self.home_visible && !self.relationships_visible && self.home.read(cx).is_todos_page();
        let is_reading = self.home_visible
            && !self.relationships_visible
            && self.home.read(cx).is_reading_page();
        let is_home_page = self.relationships_visible
            || is_artifacts
            || is_projects
            || is_pull_requests
            || is_todos
            || is_reading;
        let attention_count = self.attention_count();
        let attention_label = if attention_count == 1 {
            "⚠ 1 agent needs attention".to_owned()
        } else {
            format!("⚠ {attention_count} agents need attention")
        };
        // Center the field itself, independently of unequal header controls.
        // Retain the flexible layout in narrow windows so the adjacent badges
        // cannot overlap the workspace tabs.
        let command_width = 280.;
        let palette_width = 380.;
        let centered_min_width = if self.home_visible {
            960.
        } else if attention_count > 0 {
            1400.
        } else {
            1280.
        };
        let center_command = window.viewport_size().width >= px(centered_min_width);
        let command_left = (window.viewport_size().width - px(command_width)) / 2.;
        let git_status =
            visible_git_status(self.git_page_status.as_ref(), self.git_poll.status()).clone();

        v_flex()
            .relative()
            // The workspace's key context (`bind_app_keys` unbinds
            // Tab/Shift+Tab here): normal mode leaves both keys to the
            // focused component, component traversal lives in navigation
            // mode.
            .key_context(WORKSPACE_KEY_CONTEXT)
            .when(
                self.git_page_origin.is_some()
                    || self.home_visible
                    || self.active_tab == WorkspaceTab::Resources,
                |this| this.tab_group(),
            )
            .size_full()
            .bg(rgb(0x080909))
            .text_color(rgb(0xe7e7e7))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.close_navigation(cx);
            }))
            // Release bookkeeping for the navigation interceptor's held-key
            // set: an element bubble listener wires up during paint (unlike
            // `window.on_key_event`, which panics outside paint because view
            // render runs in layout). Key-up bubbles from any focused
            // descendant — terminals only handle key-down — so releases are
            // observed window-wide and repeats stay swallowed until then.
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, _| {
                let key = event.keystroke.key.to_ascii_lowercase();
                this.navigation_claimed_keys.remove(&key);
            }))
            // Redundant re-arm for the held-key set: if a key-up is ever
            // lost by the platform, the modifier release that always
            // follows a modified press (the trigger included) clears the
            // set once navigation mode is closed. Closed-only on purpose:
            // clearing while open would let a still-held key re-fire as a
            // fresh action. While closed the only entry that must survive
            // is the trigger's own flicker guard, and its repeats always
            // carry modifiers — an all-released event can never arrive
            // mid-flicker. Plain action-key repeats involve no modifiers,
            // so they cannot trip this either; a modifier tap mid-hold
            // merely degrades to typing a physically held key.
            //
            // Any modifier movement also marks every claim: a later
            // modifier press proves the previous hold ended, which re-arms
            // a re-pressed trigger (see `on_navigation_keystroke`) even
            // when no all-released event ever arrives.
            .on_modifiers_changed(cx.listener(|this, event: &ModifiersChangedEvent, _, _| {
                if !this.navigation_open && event.modifiers.number_of_modifiers() == 0 {
                    this.navigation_claimed_keys.clear();
                } else {
                    for marked in this.navigation_claimed_keys.values_mut() {
                        *marked = true;
                    }
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleActionsPalette, window, cx| {
                this.toggle_command_palette(PaletteMode::Actions, window, cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleProjectsPalette, window, cx| {
                this.toggle_command_palette(PaletteMode::Projects, window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoToAgent, window, cx| {
                this.go_to_agent(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoToEditor, window, cx| {
                this.go_to_editor(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoToTerminal, window, cx| {
                this.go_to_terminal(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoToReview, window, cx| {
                this.go_to_review(window, cx);
            }))
            .on_action(cx.listener(|this, _: &GoToResources, window, cx| {
                this.go_to_resources(window, cx);
            }))
            .on_action(cx.listener(|this, _: &NewAgentSession, window, cx| {
                // A dialog over the open bar would strand the query, so close
                // it first. The workspace entry happens when an agent is
                // picked, not here: cancelling must leave Home untouched.
                if this.command_open {
                    this.close_command_palette(window, cx);
                }
                this.prompt_new_agent_session(window, cx);
            }))
            .child(
                h_flex()
                    .relative()
                    .h(px(WORKSPACE_HEADER_HEIGHT))
                    .flex_none()
                    .px_3()
                    .gap_3()
                    .items_center()
                    .bg(rgb(0x0c0d0d))
                    .border_b_1()
                    .border_color(rgb(0x222525))
                    .when(self.home_visible && !is_home_page, |header| {
                        header.child(div().text_sm().font_semibold().child("Devcroft"))
                    })
                    .when(self.home_visible && !self.relationships_visible, |header| {
                        header.child(self.space_switcher(cx))
                    })
                    .when(self.relationships_visible, |header| {
                        header.child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("relationships-back")
                                        .ghost()
                                        .small()
                                        .label("‹ Back")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.back_from_relationships(window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child("Repository Relationships"),
                                ),
                        )
                    })
                    .when(is_artifacts, |header| {
                        header.child(
                            h_flex()
                                .flex_none()
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new("artifacts-back")
                                        .ghost()
                                        .small()
                                        .label(artifacts_back)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.go_back_from_artifacts(window, cx)
                                        })),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .font_semibold()
                                        .child(SharedString::from("Artifacts")),
                                ),
                        )
                    })
                    .when(
                        is_projects || is_pull_requests || is_todos || is_reading,
                        |header| {
                            header.child(
                                h_flex()
                                    .flex_none()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Button::new("projects-back")
                                            .ghost()
                                            .small()
                                            .label("‹ Home")
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.go_home(window, cx)
                                            })),
                                    )
                                    .child(div().text_sm().font_semibold().child(
                                        SharedString::from(if is_pull_requests {
                                            "Pull Requests"
                                        } else if is_todos {
                                            "Todos"
                                        } else if is_reading {
                                            "To Read"
                                        } else {
                                            "Projects"
                                        }),
                                    )),
                            )
                        },
                    )
                    .when(!self.home_visible, |header| {
                        header.child(
                            h_flex()
                                .min_w_0()
                                .max_w(px(360.))
                                .gap_2()
                                .items_center()
                                .child(
                                    Button::new(if self.git_page_origin.is_some() {
                                        "git-page-back"
                                    } else {
                                        "go-home"
                                    })
                                    .ghost()
                                    .small()
                                    .size(px(28.))
                                    .icon(IconName::ChevronLeft)
                                    .accessibility_label(if self.git_page_origin.is_some() {
                                        "Back from Git"
                                    } else {
                                        "Home"
                                    })
                                    .tooltip(if self.git_page_origin.is_some() {
                                        "Back to previous view"
                                    } else {
                                        "Back to Home"
                                    })
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            if this.git_page_origin.is_some() {
                                                this.back_from_git(window, cx);
                                            } else {
                                                this.go_home(window, cx);
                                            }
                                        },
                                    )),
                                )
                                .child(
                                    div()
                                        .min_w_0()
                                        .max_w(px(160.))
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_ellipsis()
                                        .text_sm()
                                        .font_semibold()
                                        .child(self.project_name.clone()),
                                )
                                .child(self.render_git_status_trigger(&git_status, cx)),
                        )
                    })
                    .when(center_command, |header| header.child(div().flex_1()))
                    .child(
                        h_flex()
                            .min_w_0()
                            .items_center()
                            .when(center_command, |bar| {
                                bar.absolute().top_0().h_full().left(command_left)
                            })
                            .when(!center_command, |bar| bar.flex_1().justify_center())
                            .gap_1()
                            .child(
                                div()
                                    .relative()
                                    .w(px(command_width))
                                    .min_w(px(140.))
                                    .when(center_command, |field| field.flex_none())
                                    .child(
                                        Button::new("workspace-command-trigger")
                                            .ghost()
                                            .small()
                                            .accessibility_label("Open command palette")
                                            .tooltip(if cfg!(target_os = "macos") {
                                                "Commands ⌘K · Projects ⌘P"
                                            } else {
                                                "Commands Ctrl+K · Projects Ctrl+P"
                                            })
                                            .w_full()
                                            .h(px(28.))
                                            .px_2()
                                            .items_center()
                                            .rounded_md()
                                            .border_1()
                                            .border_color(rgb(0x252828))
                                            .bg(rgb(0x121414))
                                            .text_xs()
                                            .text_color(rgb(0x737878))
                                            .cursor_pointer()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.toggle_command_palette(
                                                    PaletteMode::Actions,
                                                    window,
                                                    cx,
                                                );
                                            }))
                                            .child(
                                                Icon::new(IconName::Search)
                                                    .size(px(14.))
                                                    .text_color(rgb(0x737878)),
                                            )
                                            .child(
                                                div()
                                                    .flex_1()
                                                    .min_w_0()
                                                    .text_xs()
                                                    .text_ellipsis()
                                                    .child("Search commands…"),
                                            )
                                            .child(
                                                div()
                                                    .px_1()
                                                    .flex_none()
                                                    .rounded_sm()
                                                    .bg(rgb(0x1d2020))
                                                    .text_xs()
                                                    .text_color(rgb(0x858989))
                                                    .child(if cfg!(target_os = "macos") {
                                                        "⌘K"
                                                    } else {
                                                        "Ctrl+K"
                                                    }),
                                            )
                                            .child(
                                                div()
                                                    .px_1()
                                                    .flex_none()
                                                    .rounded_sm()
                                                    .bg(rgb(0x1d2020))
                                                    .text_xs()
                                                    .text_color(rgb(0x858989))
                                                    .child(if cfg!(target_os = "macos") {
                                                        "⌘P"
                                                    } else {
                                                        "Ctrl+P"
                                                    }),
                                            ),
                                    )
                                    // Keep the wider palette on the same center as
                                    // the trigger, leaving room for labels and hints.
                                    .when(self.command_open, |anchor| {
                                        anchor.child(
                                            deferred(
                                                div()
                                                    .absolute()
                                                    .top_0()
                                                    .left_0()
                                                    .when(center_command, |palette| {
                                                        palette.left(px((command_width
                                                            - palette_width)
                                                            / 2.))
                                                    })
                                                    .w(px(palette_width))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|_, _, _, cx| {
                                                            cx.stop_propagation()
                                                        }),
                                                    )
                                                    .child(self.render_command_bar(cx)),
                                            )
                                            .with_priority(1),
                                        )
                                    }),
                            )
                            .child(self.render_navigation_indicator())
                            .when(attention_count > 0, |bar| {
                                bar.child(
                                    Button::new("agent-attention-trigger")
                                        .ghost()
                                        .small()
                                        .accessibility_label(attention_label.clone())
                                        .tooltip(attention_label.clone())
                                        .h(px(28.))
                                        .px_2()
                                        .rounded_md()
                                        .border_1()
                                        .border_color(rgb(0x713f12))
                                        .bg(rgb(0x211609))
                                        .text_xs()
                                        .text_color(rgb(0xf59e0b))
                                        .icon(IconName::TriangleAlert)
                                        .label(attention_count.to_string())
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.open_command_palette(
                                                PaletteMode::Projects,
                                                window,
                                                cx,
                                            );
                                            this.attention_only = true;
                                            cx.notify();
                                        })),
                                )
                            }),
                    )
                    .when(self.home_visible && !is_home_page, |header| {
                        let status = self.portable_git_poll.status();
                        // Just the state tag — branch and counts live in the
                        // tooltip. Same hues as the project cards.
                        let (label, hue, detail) = if self.data_root.is_none() {
                            (
                                "Unavailable",
                                ColorName::Gray,
                                "Portable data unavailable".to_owned(),
                            )
                        } else {
                            match project_state_tag(Some(status)) {
                                Some((label, hue)) => {
                                    let mut detail = format!(
                                        "Portable · {} · {label}",
                                        status.branch.as_deref().unwrap_or("Git unavailable")
                                    );
                                    for counts in status
                                        .ahead_label()
                                        .into_iter()
                                        .chain(status.behind_label())
                                    {
                                        detail.push_str(" · ");
                                        detail.push_str(&counts);
                                    }
                                    (label, hue, detail)
                                }
                                None => (
                                    "Unavailable",
                                    ColorName::Gray,
                                    "Portable git status unavailable".to_owned(),
                                ),
                            }
                        };
                        header.child(
                            Button::new("portable-status")
                                .ghost()
                                .small()
                                .tooltip(detail)
                                .child(
                                    Tag::color(hue)
                                        .with_size(Size::Small)
                                        .rounded_full()
                                        .child(label),
                                )
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_settings(window, cx)
                                })),
                        )
                    })
                    .when(!self.home_visible, |header| {
                        header.child(
                            TabBar::new(if git_mode.is_some() {
                                "git-tabs"
                            } else {
                                "workspace-tabs"
                            })
                            .pill()
                            .small()
                            .flex_none()
                            .h(px(28.))
                            .p(px(2.))
                            .rounded_md()
                            .bg(rgb(0x171919))
                            .border_1()
                            .border_color(rgb(0x222525))
                            .selected_index(header_tab_index)
                            .on_click(cx.listener(|this, index: &usize, window, cx| {
                                if this.git_page_origin.is_some() {
                                    this.select_git_mode(*index, window, cx);
                                } else {
                                    this.select_tab(*index, window, cx);
                                }
                            }))
                            .children(
                                header_tabs.into_iter().enumerate().map(|(index, label)| {
                                    Tab::new()
                                        .label(label)
                                        .when(index == header_tab_index, |tab| tab.font_semibold())
                                        .when(git_mode.is_some(), |tab| {
                                            tab.track_focus(&self.git_tab_focus[index])
                                            .on_key_down(cx.listener(
                                            move |this,
                                                  event: &gpui_kit::KeyDownEvent,
                                                  window,
                                                  cx| {
                                                if event.keystroke.key == "enter"
                                                    || event.keystroke.key == "space"
                                                {
                                                    this.select_git_mode(index, window, cx);
                                                    cx.stop_propagation();
                                                }
                                            },
                                        ))
                                        })
                                }),
                            ),
                        )
                    })
                    .when(git_mode.is_some(), |header| {
                        header.when_some(self.git_view.as_ref(), |header, git| {
                            header.child(git.update(cx, |git, cx| git.render_header_actions(cx)))
                        })
                    }),
            )
            .child(div().flex_1().min_h_0().child(active_content))
            .when(self.command_open, |this| {
                this.child(deferred(
                    div()
                        .absolute()
                        .top(px(WORKSPACE_HEADER_HEIGHT))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, cx| {
                                this.close_command_palette(window, cx);
                            }),
                        )
                        .child(div().absolute().size_full().bg(rgb(0x000000)).opacity(0.4)),
                ))
            })
            .when(self.navigation_open, |this| {
                this.child(deferred(self.render_navigation_hud(window, cx)))
            })
            .into_any_element()
        // Dialogs, sheets, notifications, menus and tooltips are hosted
        // by the component `Root` automatically (registered by
        // `gpui_kit::init`), above this content.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Root;
    use std::path::PathBuf;

    #[gpui_kit::test]
    fn git_header_tabs_switch_modes_and_preserve_workspace_tab(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let directory = tempfile::tempdir().unwrap();
        for args in [
            vec!["init", "-b", "main"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "Initial",
            ],
        ] {
            let output = std::process::Command::new("git")
                .current_dir(directory.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let root = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, &root));
            workspace.update(cx, |workspace, cx| {
                workspace.home_visible = false;
                workspace.active_tab = WorkspaceTab::Resources;
                workspace.open_git_page(window, cx);
            });
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find(1usize).is_some());
            assert!(window.try_find(4usize).is_none());
            assert!(window.try_find(("git-dialog-mode", 0usize)).is_none());
            window.click(1usize, cx);
        });
        test_cx.run_until_parked();
        workspace.read_with(test_cx, |workspace, cx| {
            assert_eq!(
                workspace.git_view.as_ref().unwrap().read(cx).mode(),
                GitMode::History
            );
            assert_eq!(workspace.active_tab, WorkspaceTab::Resources);
        });
        // The header tabs retain keyboard activation in their new location.
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.focus_prev(cx);
            assert!(workspace.read(cx).git_tab_focus[0].is_focused(window));
        });
        test_cx.simulate_keystrokes("space");
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("space").unwrap(),
        });
        workspace.read_with(test_cx, |workspace, cx| {
            assert_eq!(
                workspace.git_view.as_ref().unwrap().read(cx).mode(),
                GitMode::Changes
            );
        });
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| workspace.back_from_git(window, cx));
            window.render_frame(cx);
            assert!(window.try_find(4usize).is_some());
        });
    }

    #[gpui_kit::test]
    fn git_page_navigation_retains_checkout_view_without_opening_a_dialog(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, &root));
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                let previous_tab = workspace.active_tab;
                let previous_focus = workspace.home.read(cx).focus_handle.clone();
                previous_focus.focus(window, cx);
                workspace.open_git_page(window, cx);
                assert!(workspace.git_page_origin.is_some());
                assert_eq!(workspace.active_tab, previous_tab);
                assert!(workspace.tabs.iter().all(Option::is_none));
                let git = workspace.git_view.clone().unwrap();
                assert_eq!(workspace.navigation_panes(cx).len(), 1);
                assert!(!window.has_active_dialog(cx));
                assert!(git.focus_handle(cx).contains_focused(window, cx));
                workspace.open_git_page(window, cx);
                workspace.run_navigation_command(NavigationCommand::Back, window, cx);
                assert!(workspace.git_page_origin.is_none());
                assert!(workspace.home_visible);
                assert_eq!(workspace.active_tab, previous_tab);
                assert!(previous_focus.contains_focused(window, cx));
                workspace.open_git_page(window, cx);
                workspace.go_home(window, cx);
                workspace.sync_git_view(cx);
                workspace.open_git_page(window, cx);
                assert_eq!(workspace.git_view.as_ref(), Some(&git));
                assert!(!window.has_active_dialog(cx));
                let original = workspace.working_directory.clone();
                let other = original.join("another-checkout");
                workspace.inactive_repositories.insert(
                    other.clone(),
                    RepositoryTabs {
                        git_view: None,
                        active_tab: WorkspaceTab::Editor,
                        tabs: vec![None; WorkspaceTab::ALL.len()],
                        editor_instance: Some(EditorChoice::BuiltIn),
                        native_editor: None,
                        review: Workspace::new_review(&other, cx),
                        agent: workspace.default_agent,
                    },
                );
                workspace.restore_repository_tabs(&other, cx);
                assert!(workspace.git_view.is_none());
                workspace.restore_repository_tabs(&original, cx);
                assert_eq!(workspace.git_view.as_ref(), Some(&git));
            });
        });
    }

    #[gpui_kit::test]
    fn git_header_entry_and_back_preserve_resource_tab_and_focus(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, &root));
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        let previous_focus = test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.active_tab = WorkspaceTab::Resources;
                workspace.home_visible = false;
                workspace
                    .resources
                    .update(cx, |view, cx| view.set_active(true, cx));
                workspace.focus_active_pane(window, cx);
                window.focused(cx).unwrap()
            })
        });
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("git-branch-switcher", cx);
            window.render_frame(cx);
            window.click("git-picker-open-page", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                assert!(workspace.git_page_origin.is_some());
                assert_eq!(workspace.active_tab, WorkspaceTab::Resources);
                assert_eq!(workspace.navigation_context(cx), NavigationContext::Git);
                assert!(!workspace.navigation_item_available(cx));
                assert!(!workspace.sessions_visible());
                assert!(!window.has_active_dialog(cx));
            });
            window.render_frame(cx);
            window.click("git-page-back", cx);
            workspace.update(cx, |workspace, cx| {
                assert!(workspace.git_page_origin.is_none());
                assert_eq!(workspace.active_tab, WorkspaceTab::Resources);
                assert!(!workspace.home_visible);
                assert!(previous_focus.contains_focused(window, cx));
                assert!(workspace.tabs.iter().all(Option::is_none));
                // The palette input takes focus, but must never become the
                // Git page's Back target after that input is unmounted.
                workspace.open_command_palette(PaletteMode::Actions, window, cx);
                assert!(
                    workspace
                        .command_state
                        .read(cx)
                        .focus_handle(cx)
                        .contains_focused(window, cx)
                );
                workspace.palette_model = vec![PaletteSection {
                    heading: "Actions",
                    items: vec![PaletteItem::Command(PaletteCommand::OpenGit)],
                }];
                workspace.on_palette_confirm(IndexPath::new(0), window, cx);
                assert!(!workspace.command_open);
                assert!(workspace.git_page_origin.is_some());
                workspace.back_from_git(window, cx);
                assert!(previous_focus.contains_focused(window, cx));
                assert_eq!(workspace.active_tab, WorkspaceTab::Resources);
                // Opening from the global browser must restore that exact
                // page rather than the dashboard or repository Resources.
                workspace.browse_artifacts(window, cx);
                let origin = workspace.artifacts_origin;
                workspace.open_git_page(window, cx);
                workspace.back_from_git(window, cx);
                assert!(workspace.home_visible);
                assert!(workspace.home.read(cx).is_artifacts_page());
                assert_eq!(workspace.artifacts_origin, origin);
            });
        });
    }

    #[gpui_kit::test]
    fn shared_branch_picker_protects_drafts_and_restores_editor_focus(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Git UI Test"]);
        git(&["config", "user.email", "git-ui@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["config", "core.hooksPath", ".git/hooks"]);
        let file = root.join("file.txt");
        std::fs::write(&file, "main\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-m", "main"]);
        git(&["switch", "-c", "feature"]);
        std::fs::write(&file, "feature\n").unwrap();
        git(&["commit", "-am", "feature"]);
        git(&["switch", "main"]);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(window, cx, &root);
                workspace.home_visible = false;
                workspace.active_tab = WorkspaceTab::Editor;
                workspace.editor_instance = Some(EditorChoice::BuiltIn);
                let editor =
                    cx.new(|cx| NativeEditor::new(&root, HashMap::new(), None, window, cx));
                editor.update(cx, |editor, cx| editor.request_open(file.clone(), None, cx));
                workspace.native_editor = Some(editor);
                workspace
            });
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        test_cx.run_until_parked();
        let editor_focus = workspace.read_with(test_cx, |workspace, cx| {
            workspace
                .native_editor
                .as_ref()
                .unwrap()
                .read(cx)
                .editor_focus(cx)
        });
        let pick = |test_cx: &mut gpui_kit::VisualTestContext, id: &str| {
            test_cx.update(|window, cx| {
                window.render_frame(cx);
                window.click("git-branch-switcher", cx);
                window.render_frame(cx);
                window.click(id.to_owned(), cx);
            });
        };
        test_cx.update(|window, cx| editor_focus.focus(window, cx));
        pick(test_cx, "git-picker-open-page");
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, _| {
                assert!(workspace.git_page_origin.is_some());
                assert_eq!(
                    workspace.git_page_origin.as_ref().unwrap().focus.as_ref(),
                    Some(&editor_focus)
                );
            });
            window.render_frame(cx);
            // The same menu is still available on the Git page.
            window.click("git-branch-switcher", cx);
            window.render_frame(cx);
            window.click("git-picker-open-page", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.back_from_git(window, cx);
                assert!(editor_focus.contains_focused(window, cx));
            });
        });
        // Editing after the repository snapshot loaded must still protect
        // the latest draft when the header menu starts a switch.
        test_cx.simulate_keystrokes("x");
        test_cx.run_until_parked();
        assert!(
            !workspace
                .read_with(test_cx, |workspace, cx| workspace
                    .native_editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dirty_paths())
                .is_empty()
        );
        let switch_feature = |test_cx: &mut gpui_kit::VisualTestContext| {
            test_cx.update(|window, cx| {
                window.render_frame(cx);
                window.click("git-branch-switcher", cx);
                window.render_frame(cx);
                // The current main branch is pinned first.
                window.click(("git-branch-switcher-item", 1usize), cx);
            });
        };
        switch_feature(test_cx);
        test_cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "main\n");
        workspace.read_with(test_cx, |workspace, cx| {
            assert!(workspace.git_page_origin.is_none());
            assert!(!workspace.git_header_busy);
            assert!(workspace.git_header_notice.is_some());
            assert!(
                !workspace
                    .native_editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dirty_paths()
                    .is_empty()
            );
        });
        test_cx.update(|window, cx| editor_focus.focus(window, cx));
        test_cx.simulate_keystrokes(if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        });
        test_cx.run_until_parked();
        assert!(
            workspace
                .read_with(test_cx, |workspace, cx| workspace
                    .native_editor
                    .as_ref()
                    .unwrap()
                    .read(cx)
                    .dirty_paths())
                .is_empty()
        );
        pick(test_cx, "git-branch-switcher-new");
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(window.has_active_dialog(cx));
            window.render_frame(cx);
            window.click("cancel-new-branch", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(!window.has_active_dialog(cx));
            workspace.read_with(cx, |workspace, _| {
                assert!(workspace.git_page_origin.is_none())
            });
            editor_focus.focus(window, cx);
        });
        switch_feature(test_cx);
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                assert!(workspace.git_header_busy);
                assert!(workspace.git_header_busy_focus.contains_focused(window, cx));
                workspace.go_home(window, cx);
                assert!(!workspace.home_visible);
            });
        });
        test_cx.run_until_parked();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "feature\n");
        test_cx.update(|window, cx| {
            workspace.read_with(cx, |workspace, cx| {
                assert!(workspace.git_page_origin.is_none());
                assert_eq!(workspace.active_tab, WorkspaceTab::Editor);
                assert!(!workspace.git_header_busy);
                assert!(editor_focus.contains_focused(window, cx));
                assert_eq!(
                    workspace
                        .git_page_status
                        .as_ref()
                        .unwrap()
                        .branch
                        .as_deref(),
                    Some("feature")
                );
            });
        });
        test_cx.update(|window, cx| editor_focus.focus(window, cx));
        pick(test_cx, "git-branch-switcher-new");
        test_cx.run_until_parked();
        test_cx.simulate_keystrokes("t o p i c");
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("confirm-new-branch", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            assert!(!window.has_active_dialog(cx));
            workspace.read_with(cx, |workspace, cx| {
                assert!(workspace.git_page_origin.is_none());
                assert!(!workspace.git_header_busy);
                assert!(editor_focus.contains_focused(window, cx));
                assert_eq!(
                    workspace
                        .git_page_status
                        .as_ref()
                        .unwrap()
                        .branch
                        .as_deref(),
                    Some("topic")
                );
            });
        });
        pick(test_cx, "git-picker-open-page");
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            workspace.read_with(cx, |workspace, _| {
                assert!(workspace.git_page_origin.is_some())
            });
        });
    }

    #[gpui_kit::test]
    fn header_git_deactivates_on_global_pages_and_ignores_retained_view_notices(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(|window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, &root));
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        let old_git = test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.home_visible = false;
                workspace.ensure_git_view(window, cx);
                workspace.sync_git_view(cx);
                let old_git = workspace.git_view.clone().unwrap();
                assert!(old_git.read(cx).is_active());
                workspace.go_home(window, cx);
                assert!(!old_git.read(cx).is_active());
                workspace.home_visible = false;
                workspace.sync_git_view(cx);
                workspace.browse_artifacts(window, cx);
                assert!(!old_git.read(cx).is_active());
                workspace.home_visible = false;
                workspace.sync_git_view(cx);
                workspace.open_relationships(window, cx);
                assert!(!old_git.read(cx).is_active());
                let other = root.join("other-checkout");
                // Retain an empty checkout without spawning real PTY threads
                // into the deterministic GPUI test scheduler.
                workspace.inactive_repositories.insert(
                    other.clone(),
                    RepositoryTabs {
                        git_view: None,
                        active_tab: WorkspaceTab::Resources,
                        tabs: vec![None; WorkspaceTab::ALL.len()],
                        editor_instance: Some(EditorChoice::BuiltIn),
                        native_editor: None,
                        review: Workspace::new_review(&other, cx),
                        agent: workspace.default_agent,
                    },
                );
                workspace.restore_repository_tabs(&other, cx);
                workspace.ensure_git_view(window, cx);
                old_git
            })
        });
        test_cx.run_until_parked();
        workspace.update(test_cx, |workspace, cx| {
            workspace.git_header_notice = Some("current checkout notice".into());
            old_git.update(cx, |_, cx| cx.notify());
        });
        test_cx.run_until_parked();
        workspace.read_with(test_cx, |workspace, _| {
            assert_eq!(
                workspace.git_header_notice.as_deref(),
                Some("current checkout notice")
            );
            assert!(!workspace.git_header_busy);
        });
    }

    #[gpui_kit::test]
    fn busy_git_blocks_editor_home_and_artifact_navigation(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().to_owned();
        let git = |args: &[&str]| {
            let result = std::process::Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        };
        git(&["init", "-b", "main"]);
        git(&["config", "user.name", "Git UI Test"]);
        git(&["config", "user.email", "git-ui@example.invalid"]);
        git(&["config", "commit.gpgsign", "false"]);
        git(&["config", "core.hooksPath", ".git/hooks"]);
        std::fs::write(root.join("file.txt"), "changed\n").unwrap();
        git(&["add", "file.txt"]);
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(window, cx, &root);
                workspace.open_git_page(window, cx);
                workspace
            });
            *slot.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let workspace = holder.borrow().clone().unwrap();
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.git_view.as_ref().unwrap().update(cx, |git, cx| {
                    git.restore_commit_message("navigation guard fixture", window, cx);
                });
            });
            window.render_frame(cx);
            window.click("git-open-commit", cx);
            window.render_frame(cx);
            window.click("git-commit", cx);
            workspace.update(cx, |workspace, cx| {
                assert!(workspace.git_view.as_ref().unwrap().read(cx).busy());
                workspace.select_tab(WorkspaceTab::Editor as usize, window, cx);
                workspace.go_home(window, cx);
                workspace.browse_artifacts(window, cx);
                workspace.back_from_git(window, cx);
                assert!(workspace.git_page_origin.is_some());
                assert_eq!(workspace.active_tab, WorkspaceTab::Agent);
                assert!(!workspace.home_visible);
            });
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            workspace.update(cx, |workspace, cx| {
                assert!(!workspace.git_view.as_ref().unwrap().read(cx).busy());
                workspace.browse_artifacts(window, cx);
                assert!(workspace.home_visible);
            });
        });
    }

    #[gpui_kit::test]
    fn built_in_editor_navigation_keeps_mode_open_and_moves_between_visible_panes(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{KeyUpEvent, Keystroke, test::TestWindowExt as _};
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("hello.txt");
        std::fs::write(&file, "hello\nworld\n").unwrap();
        std::fs::create_dir(directory.path().join("nested")).unwrap();
        std::fs::write(directory.path().join("nested/child.txt"), "child\n").unwrap();
        let root = directory.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let captured = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| {
                let mut workspace = Workspace::new(window, cx, &root);
                workspace.home_visible = false;
                workspace.active_tab = WorkspaceTab::Editor;
                workspace.editor_instance = Some(EditorChoice::BuiltIn);
                let editor =
                    cx.new(|cx| NativeEditor::new(&root, HashMap::new(), None, window, cx));
                editor.update(cx, |editor, cx| editor.request_open(file, None, cx));
                workspace.native_editor = Some(editor);
                workspace
            });
            *captured.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        view.update(test_cx, |view, cx| {
            for tab in WorkspaceTab::ALL {
                view.active_tab = tab;
                let state = view.navigation_resource_state(cx);
                let rows = navigation::rows(view.navigation_context(cx), state);
                for (command, available) in [
                    (NavigationCommand::NewSession, tab == WorkspaceTab::Agent),
                    (NavigationCommand::OpenFile, tab == WorkspaceTab::Editor),
                    (NavigationCommand::SwitchBuffer, tab == WorkspaceTab::Editor),
                    (
                        NavigationCommand::SearchProject,
                        tab == WorkspaceTab::Editor,
                    ),
                ] {
                    assert_eq!(
                        rows.iter().any(|row| row.command == command),
                        available,
                        "{tab:?}: {command:?}"
                    );
                }
            }
            view.active_tab = WorkspaceTab::Editor;
            view.home_visible = true;
            assert_eq!(
                view.navigation_resource_state(cx),
                navigation::ResourceState::default()
            );
            view.home_visible = false;
            view.relationships_visible = true;
            assert_eq!(
                view.navigation_resource_state(cx),
                navigation::ResourceState::default()
            );
            view.relationships_visible = false;
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| window.render_frame(cx));
        let press = |cx: &mut gpui_kit::VisualTestContext, key: &str| {
            cx.simulate_keystrokes(key);
            cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).unwrap(),
            });
        };
        let trigger = if cfg!(target_os = "macos") {
            "cmd-j"
        } else {
            "ctrl-j"
        };
        press(test_cx, trigger);
        for (key, pane) in [
            ("h", 0),
            ("h", 0),
            ("j", 0),
            ("k", 0),
            ("l", 1),
            ("l", 1),
            ("j", 1),
            ("k", 1),
        ] {
            press(test_cx, key);
            view.read_with(test_cx, |view, cx| {
                assert!(view.navigation_open);
                assert_eq!(view.navigation_pane, pane);
                let panes = view.navigation_panes(cx);
                assert_eq!(panes.len(), 2);
            });
            let expected = view.update(test_cx, |view, cx| {
                view.navigation_panes(cx)[pane].1.clone()
            });
            assert_eq!(
                test_cx.update(|window, cx| window.focused(cx)),
                Some(expected)
            );
        }
        press(test_cx, "enter");
        assert!(!view.update(test_cx, |view, _| view.navigation_open));
        press(test_cx, trigger);
        press(test_cx, "h");
        press(test_cx, "enter");
        assert!(!view.update(test_cx, |view, _| view.navigation_open));
        let editor_focus = view.update(test_cx, |view, cx| {
            view.native_editor
                .as_ref()
                .unwrap()
                .read(cx)
                .editor_focus(cx)
        });
        assert_eq!(
            test_cx.update(|window, cx| window.focused(cx)),
            Some(editor_focus.clone()),
            "Enter opens the tree file and focuses its editor"
        );
        press(test_cx, trigger);
        press(test_cx, "h");
        press(test_cx, "k"); // Folders precede files: move from hello.txt to nested.
        let sidebar_focus = view.update(test_cx, |view, cx| view.navigation_panes(cx)[0].1.clone());
        for _ in 0..3 {
            // Expand, collapse, then expand again.
            press(test_cx, "enter");
            assert!(
                view.update(test_cx, |view, _| view.navigation_open),
                "folder toggles keep navigation mode open"
            );
            assert_eq!(
                test_cx.update(|window, cx| window.focused(cx)),
                Some(sidebar_focus.clone())
            );
        }
        press(test_cx, "j"); // Walk into the expanded folder without retoggling the mode.
        press(test_cx, "enter");
        assert!(
            !view.update(test_cx, |view, _| view.navigation_open),
            "opening a file exits navigation mode"
        );
        let child_focus = view.update(test_cx, |view, cx| {
            view.native_editor
                .as_ref()
                .unwrap()
                .read(cx)
                .editor_focus(cx)
        });
        assert_ne!(
            child_focus, editor_focus,
            "the child file has its own editor buffer"
        );
        assert_eq!(
            test_cx.update(|window, cx| window.focused(cx)),
            Some(child_focus)
        );
    }

    /// End-to-end toggle lifecycle through the real dispatch path: press the
    /// toggle, hold-repeat it (must not double-toggle), release, toggle off,
    /// release, toggle on again. Guards the reported "triggers once, then
    /// never again" regression.
    #[gpui_kit::test]
    fn navigation_mode_retriggers_after_key_release(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::{Capslock, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent};
        use std::cell::RefCell;
        use std::rc::Rc;
        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, directory.path()));
            *holder_for_window.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        let is_open = |test_cx: &mut gpui_kit::VisualTestContext| {
            view.update(test_cx, |view, _| view.navigation_open)
        };
        let claimed = |test_cx: &mut gpui_kit::VisualTestContext| {
            view.update(test_cx, |view, _| {
                view.navigation_claimed_keys
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
            })
        };
        let release = |test_cx: &mut gpui_kit::VisualTestContext, key: &str| {
            test_cx.simulate_event(KeyUpEvent {
                keystroke: Keystroke::parse(key).unwrap(),
            });
        };
        // OS-primary chords under test: `cmd-j`/`cmd-p` on macOS,
        // `ctrl-j`/`ctrl-p` on Linux/Windows.
        const TRIGGER: &str = if cfg!(target_os = "macos") {
            "cmd-j"
        } else {
            "ctrl-j"
        };
        const TRIGGER_KEY: &str = "j";
        const PALETTE: &str = if cfg!(target_os = "macos") {
            "cmd-p"
        } else {
            "ctrl-p"
        };
        const PALETTE_KEY: &str = "p";

        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx), "first toggle opens navigation mode");
        assert!(
            claimed(test_cx).iter().any(|key| key == TRIGGER_KEY),
            "trigger key is held until release"
        );
        // Held repeat before release: swallowed, must not toggle back off.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            is_open(test_cx),
            "held trigger repeat must not double-toggle"
        );
        release(test_cx, TRIGGER_KEY);
        assert!(
            claimed(test_cx).is_empty(),
            "release clears the held-key set"
        );
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(!is_open(test_cx), "second toggle exits navigation mode");
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            is_open(test_cx),
            "third toggle re-opens navigation mode after release"
        );
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes("escape");
        assert!(!is_open(test_cx));
        release(test_cx, "escape");

        // Exit via an overlay (add-repository dialog): closing the dialog
        // must not leave stale state that refuses the next toggle.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx));
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes("a");
        assert!(!is_open(test_cx), "action exits navigation mode");
        release(test_cx, "a");
        // Close the dialog the way a user does: Escape must reach the
        // dialog trap (the interceptor yields to active overlays).
        test_cx.simulate_keystrokes("escape");
        release(test_cx, "escape");
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            is_open(test_cx),
            "toggle re-opens after a dialog opened and closed"
        );
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes("escape");
        release(test_cx, "escape");

        // The palette shortcuts stay live inside navigation mode: they exit
        // to normal mode (in the app the keystroke continues to the
        // palette, which has no binding in this harness).
        test_cx.simulate_keystrokes(TRIGGER);
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes(PALETTE);
        assert!(!is_open(test_cx), "palette shortcut exits navigation mode");
        release(test_cx, PALETTE_KEY);
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx), "toggle re-opens after a palette shortcut");
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes("escape");
        release(test_cx, "escape");

        // Lost key-ups must not wedge the toggle: open and close with no
        // releases at all, then release only the modifiers. The
        // all-released signal re-arms the trigger on its own.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx));
        test_cx.simulate_keystrokes("escape");
        assert!(!is_open(test_cx));
        test_cx.simulate_event(ModifiersChangedEvent {
            modifiers: Modifiers::default(),
            capslock: Capslock { on: false },
        });
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            is_open(test_cx),
            "toggle re-opens after modifier release without key-ups"
        );
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes("escape");
        release(test_cx, "escape");

        // Lost key-ups with no all-released event: modifier movement alone
        // marks the claim, so the next trigger press is honored as fresh.
        // Shift, then Ctrl, then Shift release keeps the modifier set
        // non-empty throughout — only the marks can re-arm here.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx));
        test_cx.simulate_keystrokes("escape");
        assert!(!is_open(test_cx));
        let modifiers = |shift: bool, control: bool| ModifiersChangedEvent {
            modifiers: Modifiers {
                shift,
                control,
                ..Default::default()
            },
            capslock: Capslock { on: false },
        };
        test_cx.simulate_event(modifiers(true, false));
        test_cx.simulate_event(modifiers(true, true));
        test_cx.simulate_event(modifiers(false, true));
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            is_open(test_cx),
            "toggle re-opens after modifier movement without key-ups"
        );
        // Drain the simulated loss: real releases clear both claims, so the
        // next segment starts clean. (A marked non-trigger key stays
        // swallowed by design — a missed press beats an accidental edit.)
        release(test_cx, TRIGGER_KEY);
        release(test_cx, "escape");
        test_cx.simulate_keystrokes("escape");
        assert!(!is_open(test_cx));
        release(test_cx, "escape");

        // Re-arm must not flicker: a held trigger repeat with no modifier
        // movement in between stays swallowed after a toggle-close.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx));
        release(test_cx, TRIGGER_KEY);
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(!is_open(test_cx), "toggle closes");
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(
            !is_open(test_cx),
            "held trigger repeat without modifier movement stays swallowed"
        );
    }

    /// Component focus traversal is a navigation-mode command. In normal mode
    /// the workspace unbind (`bind_app_keys`) keeps gpui-component's `Root`
    /// traversal from stealing Tab, so the key stays with the focused
    /// component — a terminal sends it to the pty there. The mode itself
    /// traverses with Tab/Shift+Tab and stays open for repeats.
    #[gpui_kit::test]
    fn tab_traverses_components_only_in_navigation_mode(cx: &mut gpui_kit::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(gpui_kit::init);
        // The shipped keymap, so the unbind under test is the real one.
        cx.update(crate::bind_app_keys);
        const TRIGGER: &str = if cfg!(target_os = "macos") {
            "cmd-j"
        } else {
            "ctrl-j"
        };
        let directory = tempfile::tempdir().unwrap();
        let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, directory.path()));
            *holder_for_window.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        let home_focus = view.update(test_cx, |view, cx| view.home.read(cx).focus_handle.clone());
        let focused = |test_cx: &mut gpui_kit::VisualTestContext| {
            test_cx.update(|window, cx| window.focused(cx))
        };
        let claimed_tab = |test_cx: &mut gpui_kit::VisualTestContext| {
            view.update(test_cx, |view, _| {
                view.navigation_claimed_keys.contains_key("tab")
            })
        };
        let is_open = |test_cx: &mut gpui_kit::VisualTestContext| {
            view.update(test_cx, |view, _| view.navigation_open)
        };
        // Held-key bookkeeping: a claimed key stays swallowed until its
        // release, so consecutive Tab presses need a key-up in between.
        let release_tab = |test_cx: &mut gpui_kit::VisualTestContext| {
            test_cx.simulate_event(gpui_kit::KeyUpEvent {
                keystroke: gpui_kit::Keystroke::parse("tab").unwrap(),
            });
        };
        // Home's handle is not a tab stop, so traversal must visibly move
        // focus; a stolen-and-dropped key would leave it exactly here.
        test_cx.update(|window, cx| home_focus.focus(window, cx));

        // Normal mode: Tab must reach the focused component instead of
        // moving focus. Without the unbind, `Root`'s traversal would jump to
        // the first tab stop (the dashboard's add-project button) here.
        test_cx.simulate_keystrokes("tab");
        assert_eq!(
            focused(test_cx),
            Some(home_focus.clone()),
            "normal mode must not traverse focus"
        );
        assert!(
            !claimed_tab(test_cx),
            "normal mode must not consume Tab for navigation"
        );

        // Navigation mode: both directions traverse and keep the mode open.
        test_cx.simulate_keystrokes(TRIGGER);
        assert!(is_open(test_cx));
        test_cx.simulate_keystrokes("shift-tab");
        assert!(is_open(test_cx), "Shift+Tab keeps navigation mode open");
        assert!(
            claimed_tab(test_cx),
            "Shift+Tab is a navigation input, not a component key"
        );
        assert_ne!(
            focused(test_cx),
            Some(home_focus.clone()),
            "Shift+Tab moves focus to the previous component"
        );
        release_tab(test_cx);
        test_cx.update(|window, cx| home_focus.focus(window, cx));
        test_cx.simulate_keystrokes("tab");
        assert!(is_open(test_cx), "Tab keeps navigation mode open");
        assert_ne!(
            focused(test_cx),
            Some(home_focus.clone()),
            "Tab moves focus to the next component"
        );
    }

    /// Switching spaces through the real menu and rendering every surface
    /// (Home, the global Artifacts page, a repository workspace) stays
    /// stable. Regression coverage for the reported crash on a space pick.
    #[gpui_kit::test]
    fn switching_spaces_through_the_menu_stays_stable(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(gpui_kit::init);
        let directory = tempfile::tempdir().unwrap();
        let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, directory.path()));
            *holder_for_window.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        // Menu tests persist selections; give each one its own device store.
        let space_directory = tempfile::tempdir().unwrap();
        view.update(test_cx, |view, cx| {
            view.data_root = Some(DataRoot::new(space_directory.path().to_owned()));
            view.spaces = ensure_spaces(view.data_root.as_ref().unwrap()).unwrap();
            view.active_space = view.spaces.selection(None, false);
            view.reload_spaces(cx);
        });
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        let (active, other) = view.update(test_cx, |view, _| {
            let active = view.active_space.clone();
            let other = view
                .spaces
                .names()
                .into_iter()
                .find(|name| !active.matches(name));
            (active, other)
        });
        let Some(other) = other else {
            panic!("need two spaces for the switcher regression test");
        };
        // Switch to the other space through the titlebar menu. Escape first,
        // then reopen: dismissal must leave no stale menu behind.
        test_cx.update(|window, cx| window.click("space-switcher-trigger", cx));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        test_cx.simulate_keystrokes("escape");
        test_cx.run_until_parked();
        assert!(
            !view.update(test_cx, |view, _| view.space_menu_open),
            "Escape closes the switcher"
        );
        assert!(view.update(test_cx, |view, _| view.space_menu.is_none()));
        test_cx.update(|window, cx| window.click("space-switcher-trigger", cx));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        let other_index = view.update(test_cx, |view, _| {
            view.spaces
                .names()
                .iter()
                .position(|name| crate::data::space_eq(name, &other))
                .unwrap()
        });
        test_cx.update(|window, cx| window.click(other_index + 1, cx));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        assert_eq!(
            view.update(test_cx, |view, _| view.active_space.clone()),
            SpaceSelection::Named(other)
        );
        // Exercise the global Artifacts page in the new space before
        // switching back. (A repository workspace would spawn PTY reader
        // threads, which the test scheduler rejects as nondeterministic.)
        test_cx.update(|window, cx| view.update(cx, |view, cx| view.browse_artifacts(window, cx)));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        // Switch back through the menu trigger and the first item.
        test_cx.update(|window, cx| window.click("space-switcher-trigger", cx));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        let index = view.update(test_cx, |view, _| {
            view.spaces
                .names()
                .iter()
                .position(|name| active.matches(name))
                .unwrap()
        });
        test_cx.update(|window, cx| window.click(index + 1, cx));
        test_cx.run_until_parked();
        for _ in 0..3 {
            test_cx.update(|window, cx| window.render_frame(cx));
            test_cx.run_until_parked();
        }
        assert_eq!(
            view.update(test_cx, |view, _| view.active_space.clone()),
            active
        );
    }

    /// The switcher must be usable from the keyboard alone: `Cmd+J`, `s`,
    /// arrow keys, Enter.
    #[gpui_kit::test]
    fn space_switcher_navigates_with_arrow_keys(cx: &mut gpui_kit::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        const TRIGGER: &str = if cfg!(target_os = "macos") {
            "cmd-j"
        } else {
            "ctrl-j"
        };
        let directory = tempfile::tempdir().unwrap();
        let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, directory.path()));
            *holder_for_window.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        // Menu tests persist selections; give each one its own device store.
        let space_directory = tempfile::tempdir().unwrap();
        view.update(test_cx, |view, cx| {
            view.data_root = Some(DataRoot::new(space_directory.path().to_owned()));
            view.spaces = ensure_spaces(view.data_root.as_ref().unwrap()).unwrap();
            view.active_space = view.spaces.selection(None, false);
            view.reload_spaces(cx);
        });
        let release = |test_cx: &mut gpui_kit::VisualTestContext, key: &str| {
            test_cx.simulate_event(gpui_kit::KeyUpEvent {
                keystroke: gpui_kit::Keystroke::parse(key).unwrap(),
            });
        };

        // Open the switcher through the navigation-mode entry.
        test_cx.simulate_keystrokes(TRIGGER);
        release(test_cx, "j");
        test_cx.simulate_keystrokes("s");
        release(test_cx, "s");
        test_cx.run_until_parked();
        let names = view.update(test_cx, |view, _| view.spaces.names());
        let active = view.update(test_cx, |view, _| view.active_space.clone());
        assert!(view.update(test_cx, |view, _| view.space_menu_open));
        assert!(names.len() >= 2, "need two spaces for this test");
        let target_index = names.iter().position(|name| !active.matches(name)).unwrap();

        // The menu must hold focus for its key bindings to fire.
        let menu_focus = view.update(test_cx, |view, cx| {
            view.space_menu.as_ref().map(|menu| menu.focus_handle(cx))
        });
        let focused = test_cx.update(|window, cx| window.focused(cx));
        assert_eq!(
            focused, menu_focus,
            "the space menu must hold focus for arrow keys and Enter"
        );
        // Nothing is selected on open: the first Down selects the first
        // entry, each further Down moves one step.
        for _ in 0..=target_index + 1 {
            test_cx.simulate_keystrokes("down");
        }
        test_cx.simulate_keystrokes("enter");
        test_cx.run_until_parked();
        assert_eq!(
            view.update(test_cx, |view, _| view.active_space.clone()),
            SpaceSelection::Named(names[target_index].clone()),
            "arrow keys + Enter must pick the highlighted space"
        );
        assert!(!view.update(test_cx, |view, _| view.space_menu_open));
        // Closing must not leave focus on the dropped menu handle.
        let after = test_cx.update(|window, cx| window.focused(cx));
        assert!(after.is_some(), "focus must land somewhere after closing");
        assert_ne!(after, menu_focus, "focus must leave the dropped menu");
        // The first row is All and remains selectable from the keyboard.
        test_cx.update(|window, cx| view.update(cx, |view, cx| view.open_space_menu(window, cx)));
        test_cx.simulate_keystrokes("down");
        test_cx.simulate_keystrokes("enter");
        test_cx.run_until_parked();
        view.update(test_cx, |view, cx| {
            assert_eq!(view.active_space, SpaceSelection::All);
            assert!(!view.space_menu_open);
            view.reload_spaces(cx);
            assert_eq!(view.active_space, SpaceSelection::All);
            let state = DeviceStore::new(view.data_root.as_ref().unwrap())
                .load()
                .unwrap();
            assert_eq!(state.all_spaces, Some(true));
            assert_eq!(
                view.spaces.selection(
                    state.active_space.as_deref(),
                    state.all_spaces.unwrap_or(false)
                ),
                SpaceSelection::All
            );
        });
    }

    /// The navigation-mode `s` entry opens the titlebar space switcher and
    /// leaves navigation mode; pressing it again (or the switcher's own
    /// dismissal) closes it.
    #[gpui_kit::test]
    fn switch_space_entry_toggles_the_titlebar_switcher(cx: &mut gpui_kit::TestAppContext) {
        use std::cell::RefCell;
        use std::rc::Rc;

        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        const TRIGGER: &str = if cfg!(target_os = "macos") {
            "cmd-j"
        } else {
            "ctrl-j"
        };
        let directory = tempfile::tempdir().unwrap();
        let holder: Rc<RefCell<Option<Entity<Workspace>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let workspace = cx.new(|cx| Workspace::new(window, cx, directory.path()));
            *holder_for_window.borrow_mut() = Some(workspace.clone());
            Root::new(workspace, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        // Menu tests persist selections; give each one its own device store.
        let space_directory = tempfile::tempdir().unwrap();
        view.update(test_cx, |view, cx| {
            view.data_root = Some(DataRoot::new(space_directory.path().to_owned()));
            view.spaces = ensure_spaces(view.data_root.as_ref().unwrap()).unwrap();
            view.active_space = view.spaces.selection(None, false);
            view.reload_spaces(cx);
        });
        let state = |test_cx: &mut gpui_kit::VisualTestContext| {
            view.update(test_cx, |view, _| {
                (
                    view.space_menu_open,
                    view.navigation_open,
                    view.active_space.clone(),
                    view.spaces.names(),
                )
            })
        };

        // Start on Home in navigation mode.
        assert!(test_cx.update(|_, cx| view.read(cx).home_visible));
        test_cx.simulate_keystrokes(TRIGGER);
        let (menu_open, navigation_open, active, spaces) = state(test_cx);
        assert!(navigation_open);
        assert!(!menu_open);
        assert!(!spaces.is_empty(), "the catalog seeds at startup");
        assert!(spaces.iter().any(|name| active.matches(name)));
        // Held-key bookkeeping swallows repeats until a key-up.
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("j").unwrap(),
        });

        // `s` opens the switcher and returns to normal mode.
        test_cx.simulate_keystrokes("s");
        let (menu_open, navigation_open, ..) = state(test_cx);
        assert!(menu_open, "s opens the space switcher");
        assert!(!navigation_open, "an action key leaves navigation mode");

        // The same entry toggles it closed.
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("s").unwrap(),
        });
        test_cx.simulate_keystrokes(TRIGGER);
        test_cx.simulate_event(gpui_kit::KeyUpEvent {
            keystroke: gpui_kit::Keystroke::parse("j").unwrap(),
        });
        test_cx.simulate_keystrokes("s");
        let (menu_open, ..) = state(test_cx);
        assert!(!menu_open, "s toggles the switcher closed");
    }

    /// Two focusable tab stops inside the workspace key context, with the
    /// first one recording raw Tab key events. Isolates the unbind's
    /// mechanism from the dashboard's tab-stop ring.
    struct TabUnbindHarness {
        first: gpui_kit::FocusHandle,
        second: gpui_kit::FocusHandle,
        tab_events: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl gpui_kit::Render for TabUnbindHarness {
        fn render(
            &mut self,
            _: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{InteractiveElement as _, Styled as _};
            let tab_events = self.tab_events.clone();
            gpui_kit::div()
                .key_context(WORKSPACE_KEY_CONTEXT)
                .size_full()
                .child(
                    gpui_kit::div()
                        .id("first")
                        .track_focus(&self.first)
                        .size(px(8.))
                        .on_key_down(move |event, _, _| {
                            if event.keystroke.key == "tab" {
                                tab_events.set(tab_events.get() + 1);
                            }
                        }),
                )
                .child(
                    gpui_kit::div()
                        .id("second")
                        .track_focus(&self.second)
                        .size(px(8.)),
                )
        }
    }

    /// Open the harness window with `first` focused, returning the recorded
    /// Tab-event counter.
    fn open_tab_unbind_harness(
        cx: &mut gpui_kit::TestAppContext,
    ) -> (
        Entity<TabUnbindHarness>,
        std::rc::Rc<std::cell::Cell<usize>>,
        &mut gpui_kit::VisualTestContext,
    ) {
        use std::cell::RefCell;
        use std::rc::Rc;

        let tab_events = Rc::new(std::cell::Cell::new(0));
        let events = tab_events.clone();
        let holder: Rc<RefCell<Option<Entity<TabUnbindHarness>>>> = Rc::new(RefCell::new(None));
        let holder_for_window = holder.clone();
        let (_root, test_cx) = cx.add_window_view(move |window, cx| {
            let harness = cx.new(|cx| TabUnbindHarness {
                first: cx.focus_handle().tab_stop(true),
                second: cx.focus_handle().tab_stop(true),
                tab_events: events,
            });
            *holder_for_window.borrow_mut() = Some(harness.clone());
            // Wrapped like the app's workspace, so gpui-component's `Root`
            // traversal and its "Root" key context are in play.
            Root::new(harness, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        let first = view.update(test_cx, |view, _| view.first.clone());
        test_cx.update(|window, cx| first.focus(window, cx));
        (view, tab_events, test_cx)
    }

    /// Control for the unbind test below: without the shipped bindings,
    /// gpui-component's `Root` traversal fires first, moves focus to the
    /// second tab stop, and the key never reaches the focused component.
    /// This is the behavior `bind_app_keys` removes inside the workspace.
    #[gpui_kit::test]
    fn root_traversal_steals_tab_without_the_workspace_unbind(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        let (view, tab_events, test_cx) = open_tab_unbind_harness(cx);
        let second = view.update(test_cx, |view, _| view.second.clone());
        test_cx.simulate_keystrokes("tab");
        assert_eq!(
            test_cx.update(|window, cx| window.focused(cx)),
            Some(second),
            "Root's traversal owns Tab without the unbind"
        );
        assert_eq!(tab_events.get(), 0, "the component never sees the key");
    }

    /// Inside the workspace key context the unbind suppresses `Root`'s Tab
    /// binding: focus stays put and the raw key reaches the focused
    /// component's own listener — what terminal panes use to forward it to
    /// the pty.
    #[gpui_kit::test]
    fn workspace_context_unbinds_tab_for_the_focused_component(cx: &mut gpui_kit::TestAppContext) {
        cx.update(gpui_kit::init);
        cx.update(crate::bind_app_keys);
        let (view, tab_events, test_cx) = open_tab_unbind_harness(cx);
        let first = view.update(test_cx, |view, _| view.first.clone());
        test_cx.simulate_keystrokes("tab");
        assert_eq!(
            test_cx.update(|window, cx| window.focused(cx)),
            Some(first.clone()),
            "the unbind must keep focus on the component"
        );
        assert_eq!(tab_events.get(), 1, "the component receives the raw key");
        test_cx.simulate_keystrokes("shift-tab");
        assert_eq!(
            test_cx.update(|window, cx| window.focused(cx)),
            Some(first),
            "Shift+Tab is unbound the same way"
        );
        assert_eq!(tab_events.get(), 2, "the component receives Shift+Tab too");
    }

    #[test]
    fn navigation_tab_maps_to_component_traversal() {
        let input =
            |key: &str| Workspace::navigation_input(&gpui_kit::Keystroke::parse(key).unwrap());
        assert_eq!(input("tab"), NavigationInput::Tab);
        assert_eq!(input("shift-tab"), NavigationInput::BackTab);
        assert_eq!(input("space"), NavigationInput::Key(' '));
        // Every other modified form stays a consumed modified key: only
        // Shift+Tab is a navigation input.
        assert_eq!(input("cmd-tab"), NavigationInput::Modified);
        assert_eq!(input("ctrl-shift-tab"), NavigationInput::Modified);
    }

    #[test]
    fn checkout_identity_normalizes_paths_and_preserves_missing_paths() {
        let directory = tempfile::tempdir().unwrap();
        assert_eq!(
            checkout_identity(&directory.path().join(".")),
            checkout_identity(directory.path())
        );
        let missing = directory.path().join("missing");
        assert_eq!(checkout_identity(&missing), missing);
    }

    #[cfg(unix)]
    #[test]
    fn checkout_aliases_share_identity_but_distinct_checkouts_do_not() {
        let directory = tempfile::tempdir().unwrap();
        let checkout = directory.path().join("checkout");
        let other = directory.path().join("other");
        let alias = directory.path().join("alias");
        std::fs::create_dir(&checkout).unwrap();
        std::fs::create_dir(&other).unwrap();
        std::os::unix::fs::symlink(&checkout, &alias).unwrap();
        assert_eq!(checkout_identity(&checkout), checkout_identity(&alias));
        assert_ne!(checkout_identity(&checkout), checkout_identity(&other));
    }

    #[test]
    fn tab_contract_matches_requested_commands() {
        assert_eq!(WorkspaceTab::Agent.command(), Some("opencode"));
        assert_eq!(WorkspaceTab::Editor.command(), Some("nvim ."));
        assert_eq!(WorkspaceTab::Terminal.command(), None);
        assert_eq!(WorkspaceTab::Review.command(), None);
        assert_eq!(WorkspaceTab::Review.label(), "Review");
        assert!(!WorkspaceTab::Review.has_terminal());
        assert_eq!(WorkspaceTab::Resources.label(), "Resources");
        assert_eq!(WorkspaceTab::Resources.command(), None);
        assert!(!WorkspaceTab::Resources.has_terminal());
        assert_eq!(WorkspaceTab::ALL.len(), 5);
        assert!(
            WorkspaceTab::ALL
                .into_iter()
                .all(|tab| tab.label() != "Git")
        );
    }

    fn dirty_status() -> GitStatus {
        GitStatus {
            branch: Some("main".to_owned()),
            dirty: true,
            ..GitStatus::default()
        }
    }

    #[test]
    fn visible_git_page_snapshot_owns_header_status() {
        let polled = GitStatus {
            branch: Some("main".to_owned()),
            dirty: false,
            ..GitStatus::default()
        };
        let dialog = dirty_status();

        assert_eq!(visible_git_status(Some(&dialog), &polled), &dialog);
        assert_eq!(visible_git_status(None, &polled), &polled);
    }

    #[test]
    fn stale_git_loads_never_overwrite_newer_state() {
        let mut poll = GitPoll::default();
        // Tick for the old checkout starts first and finishes last.
        let (old_generation, old_path) = poll.begin_check(PathBuf::from("/old"));
        let (new_generation, new_path) = poll.begin_check(PathBuf::from("/new"));
        assert_ne!(old_generation, new_generation);
        assert_eq!(old_path, PathBuf::from("/old"));
        assert_eq!(new_path, PathBuf::from("/new"));

        // The stale old-checkout load lands after the switch: discarded.
        assert!(!poll.commit(old_generation, dirty_status()));
        assert_eq!(poll.status(), &GitStatus::default());
        // The current load still applies.
        assert!(poll.commit(new_generation, dirty_status()));
        assert!(poll.status().dirty);
    }

    #[test]
    fn git_commit_reports_change_for_notify() {
        let mut poll = GitPoll::default();
        let (generation, _) = poll.begin_check(PathBuf::from("/repo"));
        // Identical status is not a change: no repaint needed.
        assert!(!poll.commit(generation, GitStatus::default()));
        assert!(poll.commit(generation, dirty_status()));
        // Re-committing the same status is not a change either.
        assert!(!poll.commit(generation, dirty_status()));
    }

    #[test]
    fn git_reset_clears_status_without_invalidating_pending_load() {
        let mut poll = GitPoll::default();
        let (old_generation, _) = poll.begin_check(PathBuf::from("/old"));
        assert!(poll.commit(old_generation, dirty_status()));
        // Switch: a fresh check resets the display but the pending load
        // stays current.
        let (generation, _) = poll.begin_check(PathBuf::from("/new"));
        poll.reset();
        assert_eq!(poll.status(), &GitStatus::default());
        // Reset already shows default, so offering it reports no change —
        // but the generation is accepted, and the fresh dirty load applies.
        assert!(!poll.commit(generation, GitStatus::default()));
        assert!(poll.commit(generation, dirty_status()));
        // ...while the pre-switch load is still stale.
        assert!(!poll.commit(old_generation, dirty_status()));
    }
}
