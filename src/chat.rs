//! App-level standalone threads. A single live pane is indexed by durable
//! thread ID, so tag cross-listing and navigation can never launch duplicates.
use crate::{
    agent::AgentKind,
    agent_activity::AgentActivityStore,
    agent_sessions::{Catalog, Snapshot},
    data::{
        DataRoot, SpaceSelection, Spaces,
        chat::{Registry, Thread, ViewState, normalized},
    },
    pane::TerminalPane,
};
use anyhow::{Context as _, Result, ensure};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, ParentElement, PathPromptOptions,
    Render, StatefulInteractiveElement, Styled, WeakEntity, Window,
    component::{
        ActiveTheme as _, Sizable, WindowExt as _,
        button::{Button, ButtonVariants as _},
        dialog::Confirm,
        h_flex,
        input::{Input, InputEvent, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, rgb,
};
use std::{collections::HashMap, path::PathBuf};

pub(crate) enum ChatEvent {
    Space(SpaceSelection),
    Refresh,
    New,
}
impl EventEmitter<ChatEvent> for ChatView {}
#[derive(Clone)]
enum Stop {
    Group(String),
    Thread(String),
}
pub(crate) struct ChatView {
    pub active: bool,
    root: Option<DataRoot>,
    pub registry: Registry,
    error: Option<String>,
    ownership_available: bool,
    spaces: Spaces,
    selection: SpaceSelection,
    pending_scope: Option<(Spaces, SpaceSelection)>,
    view_key: String,
    view: ViewState,
    search: Entity<InputState>,
    activity: AgentActivityStore,
    catalog: Catalog,
    snapshot: Snapshot,
    pub live: HashMap<String, Entity<TerminalPane>>,
    pub focus: FocusHandle,
    cursor: Option<usize>,
    navigation: bool,
    opening: Option<String>,
}
impl ChatView {
    pub fn new(
        root: Option<DataRoot>,
        activity: AgentActivityStore,
        catalog: Catalog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles and tags"));
        cx.subscribe(&search, |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                this.view.search = input.read(cx).value().to_string();
                this.persist_view();
                this.cursor = None;
                cx.notify();
            }
        })
        .detach();
        let loaded = root.as_ref().map(Registry::load).transpose();
        let (registry, error) = match loaded {
            Ok(r) => (r.unwrap_or_default(), None),
            Err(e) => (Registry::default(), Some(format!("{e:#}"))),
        };
        Self {
            active: false,
            root,
            ownership_available: error.is_none(),
            registry,
            error,
            spaces: Spaces::default(),
            selection: SpaceSelection::All,
            pending_scope: None,
            view_key: String::new(),
            view: ViewState::default(),
            search,
            activity,
            catalog,
            snapshot: Snapshot::default(),
            live: HashMap::new(),
            focus: cx.focus_handle(),
            cursor: None,
            navigation: false,
            opening: None,
        }
    }
    fn root(&self) -> Result<&DataRoot> {
        self.root.as_ref().context("Chat data is unavailable")
    }
    fn change<T>(&mut self, f: impl FnOnce(&mut Registry) -> Result<T>) -> Result<T> {
        let root = self.root()?.clone();
        let result = Registry::update(&root, f)?;
        self.registry = Registry::load(&root)?;
        self.error = None;
        Ok(result)
    }
    fn persist_view(&mut self) -> bool {
        let key = self.view_key.clone();
        let view = self.view.clone();
        if let Err(e) = self.change(|r| {
            r.views.insert(key, view);
            Ok(())
        }) {
            self.error = Some(format!("{e:#}"));
            return false;
        }
        true
    }
    fn reload_ownership(&mut self) -> Result<()> {
        self.ownership_available = false;
        let root = self.root()?;
        let registry = Registry::load(root)?;
        let spaces = Spaces::load(root)?;
        self.registry = registry;
        self.spaces = spaces;
        self.ownership_available = true;
        Ok(())
    }
    pub fn sync_space(
        &mut self,
        spaces: Spaces,
        selection: SpaceSelection,
        cx: &mut Context<Self>,
    ) {
        self.pending_scope = Some((spaces, selection));
        cx.notify();
    }
    pub fn set_space(
        &mut self,
        spaces: Spaces,
        selection: SpaceSelection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_scope = None;
        let key = selection
            .name()
            .and_then(|n| {
                spaces
                    .spaces
                    .iter()
                    .find(|s| crate::data::space_eq(&s.name, n))
            })
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "all".into());
        self.spaces = spaces;
        self.selection = selection;
        if self.view_key != key {
            self.persist_view();
            self.view_key = key;
            self.view = self
                .registry
                .views
                .get(&self.view_key)
                .cloned()
                .unwrap_or_default();
            self.search.update(cx, |s, cx| {
                s.set_value(self.view.search.clone(), window, cx)
            });
            self.cursor = None;
        }
        cx.notify();
    }
    pub fn refresh(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.snapshot = snapshot;
        if let Err(e) = self.reload_ownership() {
            self.error = Some(format!("{e:#}"));
            cx.notify();
            return;
        }
        self.error = None;
        // Only provider signals may bind automatically. No recency fallback.
        let binds: Vec<_> = self
            .live
            .iter()
            .filter_map(|(id, pane)| {
                let t = self.registry.threads.iter().find(|t| &t.id == id)?;
                if t.native.is_some() {
                    return None;
                }
                let native = pane.read(cx).native_session()?;
                let candidates = self.registry.candidates(id, &self.snapshot.sessions);
                let s = candidates.into_iter().find(|s| s.key.id == native)?;
                Some((id.clone(), s.clone()))
            })
            .collect();
        let updates: Vec<_> = self
            .registry
            .threads
            .iter()
            .filter_map(|t| {
                let s = self
                    .snapshot
                    .sessions
                    .iter()
                    .find(|s| Some(&s.key) == t.native.as_ref())?;
                (s.title != t.title || s.updated > t.updated)
                    .then(|| (t.id.clone(), s.title.clone(), s.updated))
            })
            .collect();
        if (!binds.is_empty() || !updates.is_empty())
            && let Err(e) = self.change(|r| {
                for (id, s) in binds {
                    r.bind(&id, &s)?;
                }
                for (id, title, time) in updates {
                    let t = r.thread_mut(&id)?;
                    t.title = title;
                    t.updated = t.updated.max(time);
                }
                Ok(())
            })
        {
            self.error = Some(format!("{e:#}"));
        }
        cx.notify();
    }
    fn space_id(&self) -> Option<&str> {
        (self.view_key != "all").then_some(self.view_key.as_str())
    }
    fn rows(&self) -> Vec<&Thread> {
        self.registry.results(self.space_id(), &self.view)
    }
    fn selected(&self) -> Option<&Thread> {
        self.registry.threads.iter().find(|t| {
            Some(&t.id) == self.view.selected.as_ref()
                && self.space_id().is_none_or(|s| s == t.space)
        })
    }
    pub fn pane(&self) -> Option<&Entity<TerminalPane>> {
        if !self.ownership_available {
            return None;
        }
        self.selected()
            .filter(|t| {
                self.root()
                    .is_ok_and(|r| t.account_matches(r, &self.spaces))
            })
            .and_then(|t| self.live.get(&t.id))
    }
    pub fn launch_id(&self, cx: &App) -> Option<u64> {
        self.pane().and_then(|p| p.read(cx).launch_id())
    }
    pub fn panes(&self, cx: &App) -> Vec<(&'static str, FocusHandle)> {
        let mut p = vec![("Threads", self.focus.clone())];
        if let Some(pane) = self.pane() {
            p.push(("Chat", pane.read(cx).focus_handle.clone()));
        }
        p
    }
    pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.focus_handle(cx).focus(window, cx);
    }
    pub fn busy_space(&self, id: &str) -> bool {
        self.registry
            .threads
            .iter()
            .any(|t| t.space == id && self.live.contains_key(&t.id))
    }
    pub fn set_navigation(&mut self, active: bool, cx: &mut Context<Self>) {
        self.navigation = active;
        if !active {
            self.cursor = None;
        }
        cx.notify();
    }
    fn stops(&self) -> Vec<Stop> {
        let rows = self.rows();
        if self.view.recent {
            return rows.iter().map(|t| Stop::Thread(t.id.clone())).collect();
        }
        let mut stops = Vec::new();
        for (group, rows) in self.registry.groups(&rows, &self.view) {
            stops.push(Stop::Group(group.clone()));
            if !self.view.collapsed.contains(&group) {
                stops.extend(rows.into_iter().map(|t| Stop::Thread(t.id.clone())));
            }
        }
        stops
    }
    pub fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let len = self.stops().len();
        if len > 0 {
            let cur = self.cursor.unwrap_or(0);
            self.cursor = Some(if down {
                if self.cursor.is_none() {
                    0
                } else {
                    (cur + 1).min(len - 1)
                }
            } else {
                cur.saturating_sub(1)
            });
        }
        cx.notify();
    }
    pub fn accept_cursor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.cursor.and_then(|i| self.stops().get(i).cloned()) {
            Some(Stop::Group(g)) => {
                self.toggle_group(g, cx);
                true
            }
            Some(Stop::Thread(id)) => {
                self.open(&id, window, cx);
                false
            }
            None => false,
        }
    }
    fn toggle_group(&mut self, g: String, cx: &mut Context<Self>) {
        if !self.view.collapsed.remove(&g) {
            self.view.collapsed.insert(g);
        }
        self.persist_view();
        cx.notify();
    }
    pub fn open(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening.is_some() {
            return;
        }
        if let Err(e) = self.reload_ownership() {
            self.error = Some(format!("{e:#}"));
            cx.notify();
            return;
        }
        let Some(t) = self.registry.threads.iter().find(|t| t.id == id).cloned() else {
            return;
        };
        self.view.selected = Some(id.into());
        if !self.persist_view() {
            self.ownership_available = false;
            cx.notify();
            return;
        }
        let valid = self
            .root()
            .is_ok_and(|r| t.account_matches(r, &self.spaces));
        if !valid {
            cx.notify();
            return;
        }
        if let Some(p) = self.live.get(id) {
            p.read(cx).focus_handle.clone().focus(window, cx);
            cx.notify();
            return;
        }
        if t.native.is_none() || !t.cwd.is_dir() {
            cx.notify();
            return;
        }
        self.opening = Some(id.into());
        let catalog = self.catalog.clone();
        let thread_id = id.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let outcome = cx
                .background_spawn(async move { catalog.prepare_chat(&t).map(|_| t) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.opening.as_deref() != Some(&thread_id) {
                    return;
                }
                this.opening = None;
                match outcome {
                    Ok(t) => {
                        if let Err(e) = this.reload_ownership() {
                            this.error = Some(format!("{e:#}"));
                            cx.notify();
                            return;
                        }
                        let unchanged = this.registry.threads.iter().any(|current| {
                            current.id == t.id
                                && current.native == t.native
                                && current.cwd == t.cwd
                                && current.account == t.account
                                && current.store == t.store
                                && current.space == t.space
                                && current.archived == t.archived
                                && this
                                    .root()
                                    .is_ok_and(|r| current.account_matches(r, &this.spaces))
                        });
                        if unchanged {
                            let pane = cx.new(|cx| {
                                TerminalPane::with_context(&t.context(), &this.activity, cx)
                            });
                            if pane.read(cx).launch_failed() {
                                window.push_notification("Could not resume chat", cx);
                            } else {
                                if this.active && this.view.selected.as_deref() == Some(&thread_id)
                                {
                                    pane.read(cx).focus_handle.clone().focus(window, cx);
                                }
                                this.live.insert(thread_id, pane);
                            }
                        }
                    }
                    Err(e) => {
                        this.error = Some(format!("{e:#}"));
                        window.push_notification(format!("Could not resume chat: {e}"), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn clear_filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.view.search.clear();
        self.view.tags.clear();
        self.view.archived = false;
        self.view.harness = None;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.persist_view();
        cx.notify();
    }
    fn launch(
        &mut self,
        space_id: &str,
        agent: AgentKind,
        folder: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let root = self.root()?.clone();
        let spaces = crate::data::ensure_spaces(&root)?;
        let space = spaces
            .spaces
            .iter()
            .find(|s| s.id == space_id)
            .context("Space was removed")?
            .clone();
        let chosen_cwd = folder.as_ref().and_then(|p| p.canonicalize().ok());
        let baseline = self
            .snapshot
            .sessions
            .iter()
            .filter(|s| s.key.provider == agent.id() && chosen_cwd.as_ref() == Some(&s.cwd))
            .map(|s| s.key.clone())
            .collect();
        let id = self.change(|r| r.create(&root, &space, agent, folder, baseline))?;
        // Ownership is on disk before PTY creation.
        let thread = self
            .registry
            .threads
            .iter()
            .find(|t| t.id == id)
            .unwrap()
            .clone();
        self.set_space(spaces, space.name.clone().into(), window, cx);
        self.clear_filters(window, cx);
        self.view.collapsed.remove("general");
        self.view.selected = Some(id.clone());
        self.persist_view();
        let pane = cx.new(|cx| TerminalPane::with_context(&thread.context(), &self.activity, cx));
        if pane.read(cx).launch_failed() {
            self.change(|r| {
                r.thread_mut(&id)?.launch_error = Some(
                    "Launch failed. Start a new chat or link any history created by this launch."
                        .into(),
                );
                Ok(())
            })?;
        } else {
            pane.read(cx).focus_handle.clone().focus(window, cx);
            self.live.insert(id, pane);
        }
        cx.emit(ChatEvent::Space(space.name.into()));
        cx.emit(ChatEvent::Refresh);
        cx.notify();
        Ok(())
    }
    pub fn new_chat(
        &mut self,
        enabled: Vec<AgentKind>,
        default: AgentKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.root().and_then(crate::data::ensure_spaces);
        let spaces = match result {
            Ok(s) => s,
            Err(e) => {
                window.push_notification(format!("Could not create chat: {e:#}"), cx);
                return;
            }
        };
        let preselect = self
            .selection
            .name()
            .and_then(|name| {
                spaces
                    .spaces
                    .iter()
                    .position(|s| crate::data::space_eq(&s.name, name))
            })
            .or_else(|| {
                spaces
                    .spaces
                    .iter()
                    .position(|s| Some(&s.id) == self.registry.last_creation_space.as_ref())
            })
            .unwrap_or(0);
        let agents = if enabled.is_empty() {
            AgentKind::ALL.to_vec()
        } else {
            enabled
        };
        let selected_agent = agents.iter().position(|a| *a == default).unwrap_or(0);
        let owner = cx.entity().downgrade();
        let dialog = cx.new(|cx| CreateChat {
            owner,
            spaces: spaces.spaces,
            agents,
            space: preselect,
            agent: selected_agent,
            step: CreationStep::Space,
            folder: None,
            folder_open: false,
            focus: cx.focus_handle(),
            error: None,
            enter_down: false,
        });
        let content = dialog.clone();
        window.open_dialog(cx, move |d, _, _| {
            d.title("New chat").w(px(480.)).child(content.clone())
        });
        dialog.read(cx).focus.clone().focus(window, cx);
    }
    fn close_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.opening.as_deref() == Some(id) {
            self.opening = None;
        }
        if let Some(p) = self.live.remove(id) {
            p.update(cx, |p, cx| p.close(cx));
        }
        cx.notify();
    }
    fn archive(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.opening.as_deref() == Some(id) {
            self.opening = None;
        }
        if self.live.contains_key(id) {
            window.push_notification("Close this chat before archiving it", cx);
            return;
        }
        let result = self.change(|r| {
            let t = r.thread_mut(id)?;
            t.archived = !t.archived;
            Ok(())
        });
        if let Err(e) = result {
            window.push_notification(format!("{e:#}"), cx);
        }
        cx.notify();
    }
    fn text_dialog(&mut self, kind: TextEdit, window: &mut Window, cx: &mut Context<Self>) {
        let initial = match &kind {
            TextEdit::Title(id) => self
                .registry
                .threads
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.label().to_string())
                .unwrap_or_default(),
            TextEdit::Tag(id) => self
                .registry
                .tags
                .iter()
                .find(|t| &t.id == id)
                .map(|t| t.name.clone())
                .unwrap_or_default(),
        };
        let owner = cx.entity().downgrade();
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |s, cx| s.set_value(initial, window, cx));
        let content = input.clone();
        let focus = input.focus_handle(cx);
        window.open_dialog(cx, move |d, _, _| {
            let input = input.clone();
            let owner = owner.clone();
            let kind = kind.clone();
            d.title("Rename")
                .child(Input::new(&content))
                .on_ok(move |_, window, cx| {
                    let value = input.read(cx).value().to_string();
                    let result = owner.update(cx, |this, cx| {
                        let result = this.change(|r| match &kind {
                            TextEdit::Title(id) => {
                                ensure!(
                                    !value.trim().is_empty() && value.chars().count() <= 240,
                                    "Use a title of 1–240 characters"
                                );
                                r.thread_mut(id)?.title_override = Some(value.trim().into());
                                Ok(())
                            }
                            TextEdit::Tag(id) => r.rename_tag(id, &value),
                        });
                        cx.notify();
                        result
                    });
                    match result {
                        Ok(Ok(())) => true,
                        Ok(Err(e)) => {
                            window.push_notification(format!("{e:#}"), cx);
                            false
                        }
                        Err(_) => true,
                    }
                })
                .footer(
                    gpui_kit::component::dialog::DialogFooter::new().child(
                        Button::new("rename-chat-save")
                            .primary()
                            .label("Save")
                            .on_click(|_, w, cx| {
                                w.dispatch_action(Box::new(Confirm { secondary: false }), cx)
                            }),
                    ),
                )
        });
        focus.focus(window, cx);
    }
    fn tags_dialog(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search or create a tag"));
        let owner = cx.entity().downgrade();
        let dialog = cx.new(|cx| {
            cx.observe(&search, |_, _, cx| cx.notify()).detach();
            TagDialog { owner, id, search }
        });
        let content = dialog.clone();
        window.open_dialog(cx, move |d, _, _| {
            d.title("Edit tags").w(px(460.)).child(content.clone())
        });
        dialog.read(cx).search.focus_handle(cx).focus(window, cx);
    }
    pub fn filters(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = cx.entity().downgrade();
        let dialog = cx.new(|_| FilterDialog { owner });
        let content = dialog.clone();
        window.open_dialog(cx, move |d, _, _| {
            d.title("Chat filters · match any tag")
                .w(px(420.))
                .child(content.clone())
        });
    }
    fn link_history(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let candidates: Vec<_> = self
            .registry
            .candidates(&id, &self.snapshot.sessions)
            .into_iter()
            .cloned()
            .collect();
        let owner = cx.entity().downgrade();
        window.open_dialog(cx,move|d,_,_|{
            let mut list=v_flex().gap_2().child("Select this chat's exact native conversation. Nothing is linked automatically from timestamps.");
            for (i,s) in candidates.iter().enumerate(){let owner=owner.clone();let id=id.clone();let s=s.clone();list=list.child(Button::new(("link-chat",i)).ghost().label(format!("{} · {} · {}",s.title,s.age(),s.key.id)).on_click(move|_,window,cx|{let result=owner.update(cx,|this,cx|{let result=this.change(|r|r.bind(&id,&s));cx.notify();result});match result{Ok(Ok(()))=>window.close_dialog(cx),Ok(Err(e))=>window.push_notification(format!("{e:#}"),cx),Err(_)=>{}}}));}
            if candidates.is_empty(){list=list.child("No eligible history yet. Send a prompt in the harness, then refresh. Existing history from before launch is excluded.");}
            d.title("Link history").w(px(620.)).child(list)
        });
    }
}
#[derive(Clone)]
enum TextEdit {
    Title(String),
    Tag(String),
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum CreationStep {
    Space,
    Harness,
    Launching,
}
struct CreateChat {
    owner: WeakEntity<ChatView>,
    spaces: Vec<crate::data::spaces::Space>,
    agents: Vec<AgentKind>,
    space: usize,
    agent: usize,
    step: CreationStep,
    folder: Option<PathBuf>,
    folder_open: bool,
    focus: FocusHandle,
    error: Option<String>,
    enter_down: bool,
}
impl CreateChat {
    fn advance(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.step == CreationStep::Space {
            self.step = CreationStep::Harness;
            cx.notify();
            return;
        }
        if self.step == CreationStep::Launching {
            return;
        }
        let Some(space) = self.spaces.get(self.space) else {
            return;
        };
        let Some(agent) = self.agents.get(self.agent).copied() else {
            return;
        };
        self.step = CreationStep::Launching;
        let result = self.owner.update(cx, |this, cx| {
            this.launch(&space.id, agent, self.folder.clone(), window, cx)
        });
        match result {
            Ok(Ok(())) => window.close_dialog(cx),
            Ok(Err(e)) => {
                self.step = CreationStep::Harness;
                self.error = Some(format!("{e:#}"));
            }
            Err(_) => window.close_dialog(cx),
        }
        cx.notify();
    }
    fn choose_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose chat working folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(mut paths))) = picker.await {
                let _ = this.update_in(cx, |this, _, cx| {
                    this.folder = paths.pop();
                    cx.notify();
                });
            }
        })
        .detach();
    }
}
impl Render for CreateChat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let space_step = self.step == CreationStep::Space;
        let mut root = v_flex()
            .gap_3()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Confirm, window, cx| {
                cx.stop_propagation();
                if !this.enter_down {
                    this.enter_down = true;
                    this.advance(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, window, cx| {
                match e.keystroke.key.as_str() {
                    "enter" => {
                        cx.stop_propagation();
                        if !this.enter_down && !e.is_held {
                            this.enter_down = true;
                            this.advance(window, cx);
                        }
                    }
                    "up" | "down" => {
                        cx.stop_propagation();
                        let (selected, len) = if this.step == CreationStep::Space {
                            (&mut this.space, this.spaces.len())
                        } else {
                            (&mut this.agent, this.agents.len())
                        };
                        if len > 0 {
                            *selected = if e.keystroke.key == "down" {
                                (*selected + 1) % len
                            } else {
                                (*selected + len - 1) % len
                            };
                        }
                        cx.notify();
                    }
                    "backspace" if this.step == CreationStep::Harness => {
                        this.step = CreationStep::Space;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "escape" => {
                        cx.stop_propagation();
                        window.close_dialog(cx);
                    }
                    _ => {}
                }
            }))
            .on_key_up(cx.listener(|this, e: &KeyUpEvent, _, _| {
                if e.keystroke.key == "enter" {
                    this.enter_down = false;
                }
            }))
            .child(div().text_sm().child(if space_step {
                "1 · Choose space".into()
            } else {
                format!(
                    "2 · Choose harness · {}",
                    self.spaces
                        .get(self.space)
                        .map(|s| s.name.as_str())
                        .unwrap_or("Unavailable")
                )
            }));
        if space_step {
            for (i, s) in self.spaces.iter().enumerate() {
                root = root.child(
                    Button::new(("chat-space", i))
                        .ghost()
                        .label(format!(
                            "{}{}",
                            if i == self.space { "● " } else { "" },
                            s.name
                        ))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.space = i;
                            this.advance(window, cx);
                        })),
                );
            }
        } else {
            for (i, a) in self.agents.iter().enumerate() {
                root = root.child(
                    Button::new(("chat-agent", i))
                        .ghost()
                        .label(format!(
                            "{}{}{}",
                            if i == self.agent { "● " } else { "" },
                            a.label(),
                            if *a == AgentKind::Omp {
                                " · Launch only — resume unavailable"
                            } else {
                                ""
                            }
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.agent = i;
                            cx.notify();
                        })),
                );
            }
            root = root.child(
                Button::new("chat-folder-disclosure")
                    .ghost()
                    .small()
                    .label(if self.folder.is_some() {
                        "Working folder: Chosen folder"
                    } else {
                        "Working folder: Managed by Devcroft"
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.folder_open = !this.folder_open;
                        cx.notify();
                    })),
            );
            if self.folder_open {
                root = root
                    .child(
                        Button::new("choose-chat-folder")
                            .small()
                            .label("Choose folder…")
                            .on_click(cx.listener(|this, _, w, cx| this.choose_folder(w, cx))),
                    )
                    .child(
                        Button::new("reset-chat-folder")
                            .ghost()
                            .small()
                            .label("Use managed folder")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.folder = None;
                                cx.notify();
                            })),
                    );
            }
            if let Some(folder) = &self.folder {
                root=root.child(div().text_xs().child(folder.display().to_string())).child(div().text_xs().child(if folder.ancestors().any(|p|p.join(".git").exists()){"Project context · Continue as standalone in the selected space. To start in the project, cancel and use its Agent page."}else{"Harness permissions and folder instructions apply normally."}));
            }
        }
        root.when_some(self.error.as_ref(), |d, e| {
            d.child(div().text_sm().text_color(rgb(0xf59e0b)).child(e.clone()))
        })
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("chat-create-cancel")
                        .label("Cancel")
                        .on_click(|_, w, cx| w.close_dialog(cx)),
                )
                .when(!space_step, |d| {
                    d.child(
                        Button::new("chat-create-back")
                            .label("Back")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.step = CreationStep::Space;
                                cx.notify();
                            })),
                    )
                })
                .child(
                    Button::new("chat-create-next")
                        .primary()
                        .label(if space_step { "Continue" } else { "Start chat" })
                        .on_click(cx.listener(|this, _, w, cx| this.advance(w, cx))),
                ),
        )
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("↑↓ Select · Enter Continue · Backspace Back · Esc Cancel"),
        )
    }
}
struct TagDialog {
    owner: WeakEntity<ChatView>,
    id: String,
    search: Entity<InputState>,
}
impl Render for TagDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = v_flex().gap_2().child(Input::new(&self.search));
        let query = self.search.read(cx).value().to_string();
        let Some(owner) = self.owner.upgrade() else {
            return root;
        };
        let chat = owner.read(cx);
        let Some(t) = chat.registry.threads.iter().find(|t| t.id == self.id) else {
            return root;
        };
        let space = t.space.clone();
        let tags: Vec<_> = chat
            .registry
            .tags
            .iter()
            .filter(|tag| tag.space == space && normalized(&tag.name).contains(&normalized(&query)))
            .map(|tag| (tag.clone(), t.tags.contains(&tag.id)))
            .collect();
        for (i, (tag, assigned)) in tags.into_iter().enumerate() {
            let owner = self.owner.clone();
            let rename_owner = owner.clone();
            let delete_owner = owner.clone();
            let id = self.id.clone();
            let tid = tag.id.clone();
            let rename_id = tid.clone();
            let delete_id = tid.clone();
            root = root.child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new(("assign-chat-tag", i))
                            .ghost()
                            .label(format!("{} {}", if assigned { "☑" } else { "☐" }, tag.name))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    if let Err(e) = this.change(|r| {
                                        let t = r.thread_mut(&id)?;
                                        if assigned {
                                            t.tags.retain(|x| x != &tid);
                                        } else {
                                            t.tags.push(tid.clone());
                                        }
                                        Ok(())
                                    }) {
                                        this.error = Some(format!("{e:#}"));
                                    }
                                    cx.notify();
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new(("rename-chat-tag", i))
                            .ghost()
                            .small()
                            .label("Rename")
                            .on_click(move |_, w, cx| {
                                let _ = rename_owner.update(cx, |this, cx| {
                                    this.text_dialog(TextEdit::Tag(rename_id.clone()), w, cx)
                                });
                            }),
                    )
                    .child(
                        Button::new(("delete-chat-tag", i))
                            .ghost()
                            .small()
                            .label("Delete")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                let _ = delete_owner.update(cx, |this, cx| {
                                    if let Err(e) = this.change(|r| {
                                        r.delete_tag(&delete_id);
                                        Ok(())
                                    }) {
                                        this.error = Some(format!("{e:#}"));
                                    }
                                    cx.notify();
                                });
                                cx.notify();
                            })),
                    ),
            );
        }
        if !query.trim().is_empty() {
            let owner = self.owner.clone();
            let id = self.id.clone();
            root = root.child(
                Button::new("create-chat-tag")
                    .label(format!("Add tag “{}”", query.trim()))
                    .on_click(cx.listener(move |_, _, w, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            let result = this.change(|r| {
                                let tag = r.tag(&space, &query)?;
                                let t = r.thread_mut(&id)?;
                                if !t.tags.contains(&tag) {
                                    t.tags.push(tag);
                                }
                                Ok(())
                            });
                            if let Err(e) = result {
                                w.push_notification(format!("{e:#}"), cx);
                            }
                            cx.notify();
                        });
                        cx.notify();
                    })),
            );
        }
        root.child(
            div()
                .text_xs()
                .child("No custom tags means General. Tags belong to this chat's space."),
        )
    }
}
struct FilterDialog {
    owner: WeakEntity<ChatView>,
}
impl Render for FilterDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut root = v_flex().gap_2();
        let Some(owner) = self.owner.upgrade() else {
            return root;
        };
        let chat = owner.read(cx);
        let mut tags: Vec<_> = chat
            .registry
            .tags
            .iter()
            .filter(|t| chat.space_id().is_none_or(|s| s == t.space))
            .map(|t| normalized(&t.name))
            .collect();
        tags.push("general".into());
        tags.sort();
        tags.dedup();
        let archived = chat.view.archived;
        let harness = chat.view.harness;
        for (i, tag) in tags.into_iter().enumerate() {
            let owner = self.owner.clone();
            let selected = chat.view.tags.contains(&tag);
            root = root.child(
                Button::new(("filter-chat-tag", i))
                    .ghost()
                    .label(format!("{} {}", if selected { "☑" } else { "☐" }, tag))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            if !this.view.tags.remove(&tag) {
                                this.view.tags.insert(tag.clone());
                            }
                            this.persist_view();
                            cx.notify();
                        });
                        cx.notify();
                    })),
            );
        }
        let owner = self.owner.clone();
        root = root.child(
            Button::new("filter-chat-archived")
                .ghost()
                .label(if archived {
                    "☑ Include archived"
                } else {
                    "☐ Include archived"
                })
                .on_click(cx.listener(move |_, _, _, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.view.archived = !this.view.archived;
                        this.persist_view();
                        cx.notify();
                    });
                    cx.notify();
                })),
        );
        let options = std::iter::once(None).chain(AgentKind::ALL.into_iter().map(Some));
        for (i, a) in options.enumerate() {
            let owner = self.owner.clone();
            root = root.child(
                Button::new(("filter-chat-harness", i))
                    .ghost()
                    .label(format!(
                        "{} {}",
                        if harness == a { "●" } else { "○" },
                        a.map(|a| a.label()).unwrap_or("All harnesses")
                    ))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.view.harness = a;
                            this.persist_view();
                            cx.notify();
                        });
                        cx.notify();
                    })),
            );
        }
        root
    }
}
impl ChatView {
    fn row(&self, t: &Thread, index: usize, cursor: bool, cx: &mut Context<Self>) -> AnyElement {
        let id = t.id.clone();
        let selected = self.view.selected.as_ref() == Some(&t.id);
        let activity = self
            .live
            .get(&id)
            .and_then(|p| p.read(cx).launch_id())
            .and_then(|id| self.activity.snapshot().for_launch(id).cloned());
        let owner = t
            .available_space(&self.spaces)
            .map(|s| s.name.as_str())
            .unwrap_or("Unavailable space");
        v_flex()
            .id(("chat-thread", index))
            .px_3()
            .py_2()
            .gap_1()
            .min_w_0()
            .rounded_md()
            .bg(if selected {
                rgb(0x183035)
            } else if cursor {
                rgb(0x22282a)
            } else {
                rgb(0x101313)
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, w, cx| this.open(&id, w, cx)))
            .child(div().text_sm().truncate().child(t.label().to_string()))
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x939f9f))
                    .truncate()
                    .child(format!(
                        "{} · {}{}",
                        t.agent.label(),
                        activity
                            .map(|a| a.state.label().to_string())
                            .unwrap_or_else(|| if t.archived {
                                "Archived".into()
                            } else {
                                crate::relative_time::relative_duration_label(
                                    crate::relative_time::current_unix_secs()
                                        .saturating_sub(t.updated),
                                )
                            }),
                        if self.space_id().is_none() {
                            format!(" · {owner}")
                        } else {
                            String::new()
                        }
                    )),
            )
            .when(self.view.recent, |d| {
                d.child(
                    div()
                        .text_xs()
                        .truncate()
                        .child(self.registry.labels(t).join(" · ")),
                )
            })
            .into_any_element()
    }
}
impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((spaces, selection)) = self.pending_scope.take() {
            self.set_space(spaces, selection, window, cx);
        }
        let rows: Vec<_> = self.rows().into_iter().cloned().collect();
        let mut list = v_flex().gap_1();
        let mut index = 0;
        let mut stop = 0;
        if self.view.recent {
            for t in &rows {
                list = list.child(self.row(
                    t,
                    index,
                    self.navigation && self.cursor == Some(stop),
                    cx,
                ));
                index += 1;
                stop += 1;
            }
        } else {
            let refs: Vec<_> = rows.iter().collect();
            for (group, threads) in self.registry.groups(&refs, &self.view) {
                let closed = self.view.collapsed.contains(&group);
                let name = if group == "general" {
                    "General".into()
                } else {
                    self.registry
                        .labels(threads[0])
                        .into_iter()
                        .find(|l| normalized(l) == group)
                        .unwrap_or(group.clone())
                };
                let toggle = group.clone();
                let cursor = self.navigation && self.cursor == Some(stop);
                list =
                    list.child(
                        Button::new(("chat-group", index))
                            .ghost()
                            .small()
                            .label(format!(
                                "{} {} {} ({})",
                                if cursor { "›" } else { "" },
                                if closed { "▸" } else { "▾" },
                                name,
                                threads.len()
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.toggle_group(toggle.clone(), cx)
                            })),
                    );
                index += 1;
                stop += 1;
                if !closed {
                    for t in threads {
                        list = list.child(self.row(
                            t,
                            index,
                            self.navigation && self.cursor == Some(stop),
                            cx,
                        ));
                        index += 1;
                        stop += 1;
                    }
                }
            }
        }
        if rows.is_empty() {
            list = list.child(
                div()
                    .p_3()
                    .text_sm()
                    .child(if self.registry.threads.is_empty() {
                        "No chats yet. Start a new chat."
                    } else {
                        "No chats match these filters."
                    }),
            );
        }
        let mut sidebar = v_flex()
            .w(px(280.))
            .min_w(px(210.))
            .max_w(px(320.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(0x293030))
            .track_focus(&self.focus)
            .gap_2()
            .p_2()
            .child(Input::new(&self.search))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("chat-tags-view")
                            .ghost()
                            .small()
                            .label(if !self.view.recent {
                                "● Tags"
                            } else {
                                "Tags"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view.recent = false;
                                this.cursor = None;
                                this.persist_view();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("chat-recent-view")
                            .ghost()
                            .small()
                            .label(if self.view.recent {
                                "● Recent"
                            } else {
                                "Recent"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.view.recent = true;
                                this.cursor = None;
                                this.persist_view();
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("chat-filters")
                            .ghost()
                            .small()
                            .label("Filters")
                            .on_click(cx.listener(|this, _, w, cx| this.filters(w, cx))),
                    ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .child(format!("{} unique chats", rows.len())),
                    )
                    .child(
                        Button::new("clear-chat-filters")
                            .ghost()
                            .small()
                            .label("Clear filters")
                            .on_click(cx.listener(|this, _, w, cx| this.clear_filters(w, cx))),
                    ),
            )
            .child(
                div()
                    .id("chat-thread-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(list),
            );
        // Workspace owns Settings and passes its current harness preferences.
        sidebar = sidebar.child(
            Button::new("new-chat")
                .primary()
                .label("New chat")
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::New))),
        );
        let mut content = v_flex().flex_1().min_w_0().h_full();
        if let Some(t) = self.selected().cloned() {
            let id = t.id.clone();
            let tags_id = id.clone();
            let title_id = id.clone();
            let archive_id = id.clone();
            let close_id = id.clone();
            let folder = t.cwd.clone();
            let owner = t
                .available_space(&self.spaces)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| "Unavailable space".into());
            content = content.child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(rgb(0x293030))
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .items_center()
                            .child(div().flex_1().min_w_0().text_sm().truncate().child(format!(
                                "{} · {} · {}",
                                t.label(),
                                t.agent.label(),
                                owner
                            )))
                            .child(
                                Button::new("rename-chat")
                                    .ghost()
                                    .small()
                                    .label("Rename")
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.text_dialog(TextEdit::Title(title_id.clone()), w, cx)
                                    })),
                            )
                            .child(
                                Button::new("edit-chat-tags")
                                    .ghost()
                                    .small()
                                    .label(format!("Tags: {}", self.registry.labels(&t).join(", ")))
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.tags_dialog(tags_id.clone(), w, cx)
                                    })),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(
                                Button::new("reveal-chat-folder")
                                    .ghost()
                                    .small()
                                    .label("Reveal folder")
                                    .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                            )
                            .child(
                                Button::new("close-chat")
                                    .ghost()
                                    .small()
                                    .label("Close session")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.close_thread(&close_id, cx)
                                    })),
                            )
                            .child(
                                Button::new("archive-chat")
                                    .ghost()
                                    .small()
                                    .label(if t.archived { "Restore" } else { "Archive" })
                                    .on_click(cx.listener(move |this, _, w, cx| {
                                        this.archive(&archive_id, w, cx)
                                    })),
                            ),
                    ),
            );
            if !rows.iter().any(|r| r.id == t.id) {
                content = content.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .child("Outside current filters · Clear filters to show this chat"),
                );
            }
            let account_ok = self.ownership_available
                && self
                    .root()
                    .is_ok_and(|r| t.account_matches(r, &self.spaces));
            if !account_ok {
                content=content.child(div().p_5().child("Unavailable space or changed account. Original history and account files are retained. Close this session, or start a new chat in an available space."));
            } else if let Some(pane) = self.live.get(&id) {
                content = content.child(div().flex_1().min_h_0().child(pane.clone()));
            } else {
                let message = if !t.cwd.is_dir() {
                    "Working folder unavailable. Restore the original folder or start a new chat."
                } else if t.agent == AgentKind::Omp {
                    "Launch only — resume unavailable for this harness."
                } else if t.native.is_none() {
                    "History pending. Link the exact native conversation to enable resume."
                } else if !self
                    .snapshot
                    .sessions
                    .iter()
                    .any(|s| Some(&s.key) == t.native.as_ref())
                {
                    "History unavailable. Refresh or archive this chat; Resume never starts a new conversation."
                } else if self.opening.as_deref() == Some(&id) {
                    "Resuming chat…"
                } else {
                    "Session closed. Select the thread to resume its exact history."
                };
                content = content.child(div().p_5().text_sm().child(message));
            }
            if t.native.is_none() && t.agent != AgentKind::Omp {
                content = content.child(
                    Button::new("link-chat-history")
                        .ghost()
                        .small()
                        .label("Link history…")
                        .on_click(
                            cx.listener(move |this, _, w, cx| this.link_history(id.clone(), w, cx)),
                        ),
                );
            }
            if let Some(error) = t.launch_error {
                content = content.child(div().p_3().text_sm().child(error));
            }
        } else {
            content = content.child(div().p_6().text_sm().child(
                "Select a chat or start a new one. Conversations run in the selected harness.",
            ));
        }
        let _ = window;
        h_flex()
            .size_full()
            .min_h_0()
            .bg(rgb(0x0c1010))
            .child(sidebar)
            .child(content.when_some(self.error.as_ref(), |d, e| {
                d.child(
                    div()
                        .p_3()
                        .text_sm()
                        .text_color(rgb(0xf59e0b))
                        .child(e.clone()),
                )
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::component::Root;
    use std::{cell::RefCell, rc::Rc};

    #[gpui_kit::test]
    fn creation_requires_space_confirmation_and_cancellation_creates_nothing(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::new(dir.path().into());
        let spaces = crate::data::ensure_spaces(&root).unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let test_root = root.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let (activity, _) = AgentActivityStore::new();
            let catalog = Catalog::new(Some(&test_root));
            let chat =
                cx.new(|cx| ChatView::new(Some(test_root.clone()), activity, catalog, window, cx));
            let create = cx.new(|cx| CreateChat {
                owner: chat.downgrade(),
                spaces: spaces.spaces,
                agents: vec![AgentKind::Codex],
                space: 0,
                agent: 0,
                step: CreationStep::Space,
                folder: None,
                folder_open: false,
                focus: cx.focus_handle(),
                error: None,
                enter_down: false,
            });
            *slot.borrow_mut() = Some((chat.clone(), create.clone()));
            Root::new(chat, window, cx)
        });
        let (_, create) = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            let dialog = create.clone();
            window.open_dialog(cx, move |d, _, _| d.title("New chat").child(dialog.clone()));
            create.read(cx).focus.clone().focus(window, cx);
        });
        test_cx.simulate_keystrokes("enter");
        create.update(test_cx, |view, _| {
            assert!(view.step == CreationStep::Harness);
        });
        assert!(Registry::load(&root).unwrap().threads.is_empty());
        test_cx.simulate_keystrokes("backspace");
        create.update(test_cx, |view, _| assert!(view.step == CreationStep::Space));
        test_cx.simulate_keystrokes("escape");
        test_cx.update(|window, cx| assert!(!window.has_active_dialog(cx)));
        assert!(Registry::load(&root).unwrap().threads.is_empty());
    }
}
