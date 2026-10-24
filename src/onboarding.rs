//! Device-local first-run setup. Choices remain a draft until Finish; the
//! completion marker is written last so failed setup stays recoverable.

use anyhow::{Context as _, Result, ensure};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, Sizable as _, StyledExt as _,
    accordion::Accordion,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    scroll::ScrollableElement as _,
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable as _,
    InteractiveElement as _, IntoElement, KeyDownEvent, MouseButton, ParentElement,
    PathPromptOptions, Render, StatefulInteractiveElement as _, Styled, Window, div, px, rgb,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::agent::AgentKind;
use crate::agent_icons::{self, AgentIconTiles};
use crate::data::{self, DataRoot, DeviceStore};
use crate::editor::EditorChoice;
use crate::editor_icons::{self, EditorIcon, EditorIconTiles};
use crate::repository_form::{RepositoryDraft, RepositoryForm};
use crate::selection_card;

const STEP_SYNC: usize = 0;
const STEP_WORKSPACE: usize = 1;
const STEP_AGENTS: usize = 2;
const STEP_PREFERENCES: usize = 3;
const STEP_REPOSITORY: usize = 4;
const STEP_REVIEW: usize = 5;
const FLOW_VERSION: u8 = 2;
const DEFAULT_SYNC_INTERVAL_MINUTES: u64 = 15;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
struct SetupDraft {
    #[serde(default)]
    flow_version: u8,
    step: usize,
    space: String,
    checkout: String,
    /// Absent in older drafts, which inferred this choice from the checkout.
    add_repository: Option<bool>,
    repository: RepositoryDraft,
    repository_detection: Option<RepositoryDraft>,
    repository_details: bool,
    isolate_agents: bool,
    sync: bool,
    restore_backup: bool,
    remote: String,
    auto_sync: bool,
    sync_interval_minutes: u64,
    enabled_agents: Vec<String>,
    default_agent: String,
    editor: String,
    theme: String,
    /// Durable intent for repairing an interrupted repository creation.
    repository_intent: Option<(String, String)>,
    /// Metadata written by a creation attempt, for recognizing an unbound retry.
    repository_intent_metadata: Option<RepositoryDraft>,
}

impl Default for SetupDraft {
    fn default() -> Self {
        Self {
            flow_version: FLOW_VERSION,
            step: STEP_SYNC,
            space: data::DEFAULT_SPACE.into(),
            checkout: String::new(),
            add_repository: None,
            repository: RepositoryDraft::default(),
            repository_detection: None,
            repository_details: false,
            isolate_agents: false,
            sync: false,
            restore_backup: false,
            remote: String::new(),
            auto_sync: false,
            sync_interval_minutes: DEFAULT_SYNC_INTERVAL_MINUTES,
            enabled_agents: AgentKind::ALL
                .iter()
                .map(|agent| agent.id().into())
                .collect(),
            default_agent: AgentKind::DEFAULT.id().into(),
            editor: EditorChoice::BuiltIn.id().into(),
            theme: "dark".into(),
            repository_intent: None,
            repository_intent_metadata: None,
        }
    }
}

impl SetupDraft {
    fn restoring(&self) -> bool {
        self.sync && self.restore_backup
    }

    fn steps(&self) -> &'static [usize] {
        if self.restoring() {
            &[STEP_SYNC, STEP_AGENTS, STEP_PREFERENCES, STEP_REVIEW]
        } else {
            &[
                STEP_SYNC,
                STEP_WORKSPACE,
                STEP_AGENTS,
                STEP_PREFERENCES,
                STEP_REPOSITORY,
                STEP_REVIEW,
            ]
        }
    }

    fn next_step(&self) -> usize {
        self.steps()
            .iter()
            .copied()
            .find(|step| *step > self.step)
            .unwrap_or(STEP_REVIEW)
    }

    fn previous_step(&self) -> usize {
        self.steps()
            .iter()
            .copied()
            .rev()
            .find(|step| *step < self.step)
            .unwrap_or(STEP_SYNC)
    }

    fn wants_repository(&self) -> bool {
        !self.restoring()
            && self
                .add_repository
                .unwrap_or(!self.checkout.trim().is_empty())
    }

    fn load(root: &DataRoot) -> Self {
        let mut draft: Self = DeviceStore::new(root)
            .load()
            .ok()
            .and_then(|state| state.onboarding_draft)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default();
        if draft.flow_version < FLOW_VERSION {
            // Before sync-first setup, workspace included the initial repository.
            draft.step = [
                STEP_WORKSPACE,
                STEP_SYNC,
                STEP_AGENTS,
                STEP_PREFERENCES,
                STEP_REVIEW,
            ][draft.step.min(4)];
            draft.flow_version = FLOW_VERSION;
        }
        draft.step = draft
            .steps()
            .iter()
            .copied()
            .find(|step| *step >= draft.step)
            .unwrap_or(STEP_REVIEW);
        if !data::is_supported_sync_interval(draft.sync_interval_minutes) {
            draft.sync_interval_minutes = DEFAULT_SYNC_INTERVAL_MINUTES;
        }
        draft.enabled_agents = AgentKind::ALL
            .into_iter()
            .filter(|agent| {
                draft
                    .enabled_agents
                    .iter()
                    .any(|id| AgentKind::parse(id) == Some(*agent))
            })
            .map(|agent| agent.id().to_owned())
            .collect();
        if draft.enabled_agents.is_empty() {
            draft.enabled_agents.push(AgentKind::DEFAULT.id().into());
        }
        if !draft.enabled_agents.contains(&draft.default_agent) {
            draft.default_agent = draft.enabled_agents[0].clone();
        }
        if EditorChoice::parse(&draft.editor).is_none() {
            draft.editor = EditorChoice::BuiltIn.id().into();
        }
        if draft.theme != "light" {
            draft.theme = "dark".into();
        }
        draft
    }

    fn save(&self, root: &DataRoot) -> Result<()> {
        let value = serde_json::to_value(self)?;
        DeviceStore::new(root)
            .update(|state| state.onboarding_draft = Some(value))
            .map(|_| ())
    }

    fn validate_step(&self, step: usize) -> Result<()> {
        match step {
            STEP_WORKSPACE if !self.restoring() => {
                data::normalize_name(&self.space)?;
            }
            STEP_REPOSITORY => {
                if self.wants_repository() {
                    ensure!(
                        !self.checkout.trim().is_empty(),
                        "Choose a local Git checkout, or select Start empty."
                    );
                    data::require_repository_key(self.repository.key.trim())?;
                }
            }
            STEP_SYNC if self.sync => {
                let remote = self.remote.trim();
                ensure!(
                    !remote.is_empty(),
                    "Enter a Git remote URL, or choose Keep it local."
                );
                ensure!(
                    !remote.starts_with('-') && !remote.chars().any(char::is_whitespace),
                    "Use an SSH or HTTPS Git remote URL without spaces."
                );
                let valid = if let Some(address) = remote
                    .strip_prefix("https://")
                    .or_else(|| remote.strip_prefix("ssh://"))
                {
                    address.split_once('/').is_some_and(|(authority, path)| {
                        !authority.is_empty() && !authority.ends_with('@') && !path.is_empty()
                    })
                } else {
                    remote.split_once(':').is_some_and(|(authority, path)| {
                        !path.is_empty()
                            && authority
                                .split_once('@')
                                .is_some_and(|(user, host)| !user.is_empty() && !host.is_empty())
                    })
                };
                ensure!(
                    valid,
                    "Use an HTTPS URL or an SSH address such as git@host:you/devcroft-data.git."
                );
                ensure!(
                    !self.auto_sync || data::is_supported_sync_interval(self.sync_interval_minutes),
                    "Choose a supported automatic sync frequency."
                );
            }
            STEP_AGENTS => {
                ensure!(
                    !self.enabled_agents.is_empty(),
                    "Keep at least one agent enabled."
                );
                ensure!(
                    self.enabled_agents
                        .iter()
                        .all(|id| AgentKind::parse(id).is_some()),
                    "Unknown agent harness."
                );
                ensure!(
                    self.enabled_agents.contains(&self.default_agent),
                    "Choose an enabled agent as your default."
                );
            }
            STEP_PREFERENCES => {
                ensure!(
                    EditorChoice::parse(&self.editor).is_some(),
                    "Choose an editor."
                );
                ensure!(
                    matches!(self.theme.as_str(), "dark" | "light"),
                    "Choose an appearance."
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn set_agent_enabled(&mut self, agent: AgentKind, enabled: bool) {
        if enabled && !self.enabled_agents.iter().any(|id| id == agent.id()) {
            self.enabled_agents.push(agent.id().into());
        } else if !enabled && self.enabled_agents.len() > 1 {
            self.enabled_agents.retain(|id| id != agent.id());
        }
        if !self.enabled_agents.contains(&self.default_agent) {
            self.default_agent = self.enabled_agents[0].clone();
        }
    }

    fn apply(&mut self, root: &DataRoot) -> Result<()> {
        // Inspect before any mutations. The shared harvest also supports older drafts.
        let inspection = if self.wants_repository() {
            ensure!(
                !self.checkout.trim().is_empty(),
                "Choose a local Git checkout, or select Start empty."
            );
            let inspection = data::inspect_checkout(Path::new(self.checkout.trim()))?;
            self.repository_detection = Some(
                self.repository
                    .refresh_from_checkout(&inspection, self.repository_detection.as_ref()),
            );
            Some(inspection)
        } else {
            None
        };
        for &step in self.steps() {
            self.validate_step(step)?;
        }
        if self.sync && self.restore_backup {
            data::restore_onboarding_backup(root, self.remote.trim())?;
        }
        data::ensure_spaces(root)?;
        let mut spaces = data::Spaces::load(root)?;
        let name = if self.restoring() {
            // Keep the synced catalog and isolation profiles intact on another device.
            spaces
                .first()
                .context("The restored workspace has no spaces")?
                .to_owned()
        } else {
            let expected = spaces.clone();
            let name = data::normalize_name(&self.space)?;
            spaces.ensure_name(&name);
            let name = spaces
                .canonical(&name)
                .context("Could not create the space")?;
            spaces.set_agent_isolation(&name, self.isolate_agents)?;
            spaces.save(root, &expected)?;
            name
        };
        if let Some(inspection) = inspection {
            self.apply_repository(root, &inspection, &name)?;
        }
        if self.sync {
            data::set_origin(root, self.remote.trim())?;
        } else {
            data::clear_origin(root)?;
        }
        DeviceStore::new(root).update(|state| {
            state.active_space = Some(name);
            state.all_spaces = Some(false);
            state.enabled_agents = Some(self.enabled_agents.clone());
            state.default_agent = Some(self.default_agent.clone());
            state.editor_choice = Some(self.editor.clone());
            state.theme = Some(self.theme.clone());
            state.sync_interval_minutes =
                (self.sync && self.auto_sync).then_some(self.sync_interval_minutes);
            state.onboarding_complete = Some(true);
            state.onboarding_draft = None;
        })?;
        Ok(())
    }

    fn apply_repository(
        &mut self,
        root: &DataRoot,
        inspection: &data::CheckoutInspection,
        space: &str,
    ) -> Result<()> {
        let key = data::require_repository_key(self.repository.key.trim())?;
        let input = self.repository.input(space);
        let intent = (
            key.clone(),
            inspection.checkout_path.to_string_lossy().into_owned(),
        );
        if let Some(bound_key) = data::resolve_current_key(root, &inspection.checkout_path) {
            ensure!(
                bound_key == key,
                "This checkout is already linked to another repository. Start empty and manage it from Home."
            );
        }
        if data::repository_dir(root, &key).exists() {
            let metadata = data::get_repository_metadata(root, &key)?;
            let binding = data::checkout_for(root, &key);
            let original = self
                .repository_intent_metadata
                .as_ref()
                .map(|draft| draft.input(space));
            let original_clone_url = original
                .as_ref()
                .and_then(|input| input.clone_url.clone())
                .or_else(|| inspection.clone_url.clone());
            let original_name = original
                .as_ref()
                .and_then(|input| input.name.as_deref())
                .unwrap_or(&inspection.name);
            ensure!(
                self.repository_intent.as_ref() == Some(&intent)
                    && binding
                        .as_ref()
                        .is_none_or(|path| path == &inspection.checkout_path)
                    && (binding.is_some()
                        || (metadata.clone_url == original_clone_url
                            && metadata.name.as_deref() == Some(original_name))),
                "A different repository already uses this key. Choose a unique key in Repository details, or select Start empty."
            );
            data::link_repository(root, &key, &inspection.checkout_path)?;
            data::update_repository_metadata(root, &key, &input, &metadata.revision)?;
            data::record_repository_open(root, &key)?;
        } else {
            self.repository_intent = Some(intent);
            self.repository_intent_metadata = Some(self.repository.clone());
            self.save(root)?;
            data::create_repository(root, &key, &inspection.checkout_path, &input)?;
        }
        Ok(())
    }
}

pub(crate) struct SetupComplete;

#[derive(Clone, Copy)]
enum ChoiceGroup {
    Sync,
    Theme,
    Editor,
    Backup,
    InitialRepository,
}

impl ChoiceGroup {
    fn id(self) -> &'static str {
        match self {
            Self::Sync => "setup-sync",
            Self::Theme => "setup-theme",
            Self::Editor => "setup-editor",
            Self::Backup => "setup-backup-mode",
            Self::InitialRepository => "setup-initial-repository",
        }
    }
}

pub(crate) struct OnboardingView {
    root: DataRoot,
    draft: SetupDraft,
    space_input: Entity<InputState>,
    checkout_input: Entity<InputState>,
    remote_input: Entity<InputState>,
    repository_form: RepositoryForm,
    inspected_checkout: Option<String>,
    checking_repository: bool,
    icons: AgentIconTiles,
    editor_icons: EditorIconTiles,
    choice_focus: [[FocusHandle; 2]; 5],
    busy: bool,
    error: Option<String>,
}

impl EventEmitter<SetupComplete> for OnboardingView {}

impl OnboardingView {
    pub(crate) fn new(root: DataRoot, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = SetupDraft::load(&root);
        gpui_kit::component::Theme::change(
            crate::theme_mode_from_name(&draft.theme),
            Some(window),
            cx,
        );
        crate::apply_focus_theme(cx);
        let space_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Personal, Work, or something of your own")
                .default_value(&draft.space)
        });
        let checkout_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Path to a local Git checkout")
                .default_value(&draft.checkout)
        });
        let remote_input = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("git@github.com:you/devcroft-data.git")
                .default_value(&draft.remote)
        });
        let repository_form = RepositoryForm::new(&draft.repository, window, cx);
        for input in [&space_input, &checkout_input, &remote_input]
            .into_iter()
            .chain(repository_form.inputs())
        {
            cx.subscribe(input, |this, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) && !this.busy {
                    this.save_draft(cx);
                }
            })
            .detach();
        }
        let mut icons = AgentIconTiles::new();
        agent_icons::ensure_tiles(AgentKind::ALL, &mut icons, cx);
        let mut editor_icons = EditorIconTiles::new();
        editor_icons::ensure_tiles(
            [EditorIcon::Devcroft, EditorIcon::Neovim],
            &mut editor_icons,
        );
        let choice_focus: [[FocusHandle; 2]; 5] =
            std::array::from_fn(|_| std::array::from_fn(|_| cx.focus_handle()));
        let view = Self {
            root,
            draft,
            space_input,
            checkout_input,
            remote_input,
            repository_form,
            inspected_checkout: None,
            checking_repository: false,
            icons,
            editor_icons,
            choice_focus,
            busy: false,
            error: None,
        };
        view.focus_step(window, cx);
        view
    }

    fn save_draft(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.draft.space = self.space_input.read(cx).value().to_string();
        self.draft.checkout = self.checkout_input.read(cx).value().to_string();
        self.draft.remote = self.remote_input.read(cx).value().to_string();
        self.draft.repository = self.repository_form.snapshot(cx);
        self.error = self
            .draft
            .save(&self.root)
            .err()
            .map(|error| format!("Could not save setup progress: {error:#}"));
        cx.notify();
    }

    fn go_to(&mut self, step: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.checking_repository {
            return;
        }
        self.save_draft(cx);
        if self.error.is_some() {
            return;
        }
        if step > self.draft.step
            && self.draft.step == STEP_REPOSITORY
            && self.draft.wants_repository()
            && !self.draft.checkout.trim().is_empty()
            && self.inspected_checkout.as_deref() != Some(self.draft.checkout.trim())
        {
            self.inspect_repository(Some(step), window, cx);
            return;
        }
        if step > self.draft.step
            && let Err(error) = self.draft.validate_step(self.draft.step)
        {
            if self.draft.step == STEP_REPOSITORY && self.draft.wants_repository() {
                self.draft.repository_details = true;
            }
            self.error = Some(error.to_string());
            cx.notify();
            return;
        }
        self.draft.step = self
            .draft
            .steps()
            .iter()
            .copied()
            .find(|active| *active >= step)
            .unwrap_or(STEP_REVIEW);
        self.save_draft(cx);
        self.focus_step(window, cx);
    }

    fn focus_step(&self, window: &mut Window, cx: &mut Context<Self>) {
        match self.draft.step {
            STEP_WORKSPACE => self.space_input.read(cx).focus_handle(cx).focus(window, cx),
            STEP_SYNC if self.draft.sync => self
                .remote_input
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx),
            STEP_SYNC => self.choice_focus[ChoiceGroup::Sync as usize][0].focus(window, cx),
            STEP_REPOSITORY if self.draft.wants_repository() => self
                .checkout_input
                .read(cx)
                .focus_handle(cx)
                .focus(window, cx),
            STEP_REPOSITORY => {
                self.choice_focus[ChoiceGroup::InitialRepository as usize][0].focus(window, cx)
            }
            STEP_PREFERENCES => self.choice_focus[ChoiceGroup::Theme as usize]
                [usize::from(self.draft.theme == "light")]
            .focus(window, cx),
            _ => window.blur(cx),
        }
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.checking_repository {
            return;
        }
        self.save_draft(cx);
        if self.error.is_some() {
            return;
        }
        let mut draft = self.draft.clone();
        let root = self.root.clone();
        self.busy = true;
        cx.notify();
        cx.spawn_in(window, async move |view, cx| {
            let (draft, result) = cx.background_spawn(async move {
                let result = draft.apply(&root);
                (draft, result)
            }).await;
            let _ = view.update(cx, |this, cx| {
                this.busy = false;
                this.draft = draft;
                match result {
                    Ok(()) => cx.emit(SetupComplete),
                    Err(error) => this.error = Some(format!("Setup could not finish: {}. Your choices are saved; go back to adjust them or retry. Any backup already imported stays available, with original files kept in a local onboarding-backup folder.", data::sanitize_git_message(&format!("{error:#}")))),
                }
                cx.notify();
            });
        }).detach();
    }

    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.checking_repository {
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose a Git repository".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let result = receiver.await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| match result {
                    Ok(Ok(Some(paths))) => {
                        if !this.busy
                            && this.draft.wants_repository()
                            && let Some(path) = paths.first()
                        {
                            this.checkout_input.update(cx, |input, cx| {
                                input.set_value(path.to_string_lossy().to_string(), window, cx)
                            });
                            this.inspect_repository(None, window, cx);
                        }
                    }
                    Ok(Err(error)) => {
                        this.error = Some(format!("Could not open the folder picker: {error:#}"));
                        cx.notify();
                    }
                    _ => {}
                });
            });
        })
        .detach();
    }

    fn inspect_repository(
        &mut self,
        next_step: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy || self.checking_repository {
            return;
        }
        self.save_draft(cx);
        if self.error.is_some() {
            return;
        }
        let requested_path = self.draft.checkout.trim().to_owned();
        self.checking_repository = true;
        cx.notify();
        cx.spawn_in(window, async move |view, cx| {
            let path = requested_path.clone();
            let result = cx
                .background_spawn(async move { data::inspect_checkout(Path::new(&path)) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.checking_repository = false;
                    // The user may have changed the path or selected Start empty.
                    if !this.draft.wants_repository()
                        || this.checkout_input.read(cx).value().trim() != requested_path
                    {
                        cx.notify();
                        return;
                    }
                    match result {
                        Ok(inspection) => {
                            this.draft.repository_detection =
                                Some(this.repository_form.apply_inspection(
                                    &inspection,
                                    this.draft.repository_detection.as_ref(),
                                    window,
                                    cx,
                                ));
                            let path = inspection.checkout_path.to_string_lossy().into_owned();
                            this.checkout_input
                                .update(cx, |input, cx| input.set_value(path.clone(), window, cx));
                            this.inspected_checkout = Some(path);
                            this.save_draft(cx);
                            if let Some(step) = next_step {
                                this.go_to(step, window, cx);
                            }
                        }
                        Err(error) => {
                            this.error =
                                Some(format!("Could not read this Git checkout: {error:#}"));
                            cx.notify();
                        }
                    }
                });
            });
        })
        .detach();
    }

    fn heading(&self, title: &str, subtitle: &str, cx: &Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_2()
            .mb_2()
            .child(div().text_2xl().font_semibold().child(title.to_owned()))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(subtitle.to_owned()),
            )
    }

    fn note(&self, text: &str, cx: &Context<Self>) -> impl IntoElement {
        div()
            .p_3()
            .rounded_lg()
            .bg(cx.theme().secondary)
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(text.to_owned())
    }

    fn workspace(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex().gap_4()
            .child(self.heading("A space for your best work.", "Start with one workspace. In Devcroft, we call it a space.", cx))
            .child(v_flex().gap_2().child(div().font_semibold().text_sm().child("Space name"))
                .child(Input::new(&self.space_input).aria_label("Space name")))
            .child(self.note("A space groups your projects and activity. You can add more spaces later; agent accounts are configured in the next step.", cx))
            .into_any_element()
    }

    fn initial_repository(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex().gap_4()
            .child(self.heading("Bring your first project.", "Add an existing Git checkout to your space, or start empty and add one later from Home.", cx))
            .child(v_flex().gap_3()
                .child(div().font_semibold().text_sm().child("Your first repository"))
                .child(selection_card::group("setup-initial-repository-options").aria_label("Initial repository").children([
                    (gpui_kit::assets::IconName::Monitor, "Start empty", "Settle in first. Add repositories from Home whenever you're ready."),
                    (gpui_kit::assets::IconName::GitBranch, "Add your first repository", "Connect a local Git checkout and review its details."),
                ].into_iter().enumerate().map(|(index, (icon, title, detail))| {
                    self.choice(ChoiceGroup::InitialRepository, index, Icon::new(icon).size(px(20.)), (title, detail), cx)
                })))
                .when(self.draft.wants_repository(), |view| view.child(self.repository_fields(cx))))
            .into_any_element()
    }

    fn agent_isolation(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3()
            .child(div().p_4().rounded_lg().border_1().border_color(cx.theme().border)
                .child(h_flex().gap_4().justify_between()
                    .child(v_flex().flex_1().min_w_0().gap_1().child(div().font_semibold().text_sm().child("Separate agent accounts"))
                        .child(div().text_sm().text_color(cx.theme().muted_foreground).child("Give this space its own agent logins, settings, and session history. Useful for separating work and personal accounts.")))
                    .child(Switch::new("setup-isolation").flex_none().accessibility_label("Separate agent accounts").checked(self.draft.isolate_agents)
                        .on_change(cx.listener(|this, enabled, _, cx| { this.draft.isolate_agents = *enabled; this.save_draft(cx); })))))
            .child(self.note(if self.draft.isolate_agents { "You'll sign in to each agent once in this space. This separates agent accounts; it does not sandbox repository files." } else { "Projects are grouped by space. Agents use your existing shared accounts. You can create more spaces and change isolation in Settings." }, cx))
    }

    fn repository_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex().gap_3()
            .child(v_flex().gap_1()
                .child(div().text_xs().child("Local Git checkout"))
                .child(h_flex().gap_2()
                    .child(Input::new(&self.checkout_input).aria_label("Local Git repository folder"))
                    .child(Button::new("setup-browse").outline().label("Choose folder…")
                        .disabled(self.busy || self.checking_repository)
                        .on_click(cx.listener(|this, _, window, cx| this.browse(window, cx)))))
                .child(div().text_xs().text_color(cx.theme().muted_foreground)
                    .child("Choose an existing checkout to detect its details. Pasted paths are checked when you continue.")))
            .child(h_flex().gap_3().items_start()
                .child(div().flex_1().min_w_0().child(RepositoryForm::field("Display name", &self.repository_form.display_name)))
                .child(div().flex_1().min_w_0().child(RepositoryForm::field("Base branch", &self.repository_form.base_branch))))
            .child(div().text_xs().text_color(cx.theme().muted_foreground)
                .child(format!("Added to the {} space. You can move it later.", self.draft.space.trim())))
            .child(Accordion::new("setup-repository-details").w_full().h_auto().flex_none()
                .disabled(self.busy)
                .item(|item| item.title("Repository details").open(self.draft.repository_details)
                    .child(v_flex().id("setup-repository-fields").gap_3().pt_2()
                .child(RepositoryForm::field("Repository key", &self.repository_form.key))
                .child(div().text_xs().text_color(cx.theme().muted_foreground).child("A unique identifier for this repository. It cannot be renamed after creation."))
                .child(h_flex().gap_3().items_start()
                    .child(div().flex_1().min_w_0().child(RepositoryForm::field("Owner", &self.repository_form.owner)))
                    .child(div().flex_1().min_w_0().child(RepositoryForm::field("Name", &self.repository_form.name))))
                .child(RepositoryForm::field("Clone URL", &self.repository_form.clone_url))
                .child(RepositoryForm::field("Description", &self.repository_form.description))
                .child(RepositoryForm::field("Tags", &self.repository_form.tags))))
                .on_toggle_click(cx.listener(|this, open: &[usize], _, cx| {
                    this.draft.repository_details = open.contains(&0);
                    this.save_draft(cx);
                })))
    }

    fn sync(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex().gap_4()
            .child(self.heading("Your workspace, wherever you are.", "Keep everything on this device, or connect a Git repository for backup and sync.", cx))
            .child(selection_card::group("setup-sync-options").aria_label("Backup and sync").children([
                (gpui_kit::assets::IconName::Monitor, "Keep it local", "Everything stays on this device. You can connect sync later."),
                (gpui_kit::assets::IconName::CloudSync, "Connect Git sync", "Back up portable data and share it between your devices."),
            ].into_iter().enumerate().map(|(index, (icon, title, detail))| {
                self.choice(ChoiceGroup::Sync, index, Icon::new(icon).size(px(20.)), (title, detail), cx)
            })))
            .when(self.draft.sync, |view| view
                .child(v_flex().gap_2()
                    .child(h_flex().justify_between().gap_3().flex_wrap()
                        .child(div().id("setup-repository-title").text_sm().font_semibold().child("Private Git repository URL"))
                        .child(gpui_kit::base::RadioGroup::new("setup-backup-options")
                            .axis(gpui_kit::Axis::Horizontal).aria_label("Repository setup")
                            .flex().items_center().gap_3()
                            .children(["New repository", "Existing repository"].into_iter().enumerate().map(|(index, label)| {
                                self.choice_radio(ChoiceGroup::Backup, index, cx)
                                    .accessibility_label(label)
                                    .child(div().text_sm().child(label))
                            }))))
                    .child(Input::new(&self.remote_input).aria_label("Private Git repository URL"))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground).child(if self.draft.restore_backup { "Use the backup repository from your other device. Finishing setup downloads its portable data with your existing Git credentials. Original setup files are kept locally for recovery." } else { "Use an empty private Git repository for this installation. Your first sync uploads the backup; use Sync now in Settings after setup to verify access. SSH and HTTPS are supported." })))
                .child(v_flex().gap_3()
                    .child(h_flex().justify_between().gap_3()
                        .child(div().text_sm().child("Automatically sync"))
                        .child(Switch::new("setup-auto-sync").accessibility_label("Automatically sync").checked(self.draft.auto_sync)
                            .on_change(cx.listener(|this, enabled, _, cx| { this.draft.auto_sync = *enabled; this.save_draft(cx); }))))
                    .when(self.draft.auto_sync, |view| view.child(
                        h_flex().id("setup-sync-frequency").flex_wrap().gap_2()
                            .child(div().text_sm().mr_1().child("Frequency"))
                            .children(data::SYNC_INTERVAL_OPTIONS.into_iter().map(|minutes| {
                                let selected = self.draft.sync_interval_minutes == minutes;
                                Button::new(("setup-sync-interval", minutes as usize)).small().outline()
                                    .label(format!("Every {minutes} min"))
                                    .accessibility_label(format!("Sync every {minutes} minutes{}", if selected { ", selected" } else { "" }))
                                    .when(selected, |button| button.border_color(cx.theme().ring).bg(cx.theme().accent))
                                    .on_click(cx.listener(move |this, _, _, cx| { this.draft.sync_interval_minutes = minutes; this.save_draft(cx); }))
                            })))))
                .when(self.draft.restore_backup, |view| view.child(self.note("Connecting another device? Your spaces, repositories, and space isolation settings come from this backup. Setup will skip creating a space or adding a repository. Link local checkouts from Home after restoring.", cx)))
                .child(self.note("Sync includes project metadata, spaces, notes, artifacts, and review comments. Source checkouts, credentials, and device settings stay local.", cx)))
            .when(!self.draft.sync, |view| view.child(self.note("No account or remote repository is needed to get started. Your data is stored locally and remains available offline.", cx)))
            .into_any_element()
    }

    fn select_choice(
        &mut self,
        group: ChoiceGroup,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            return;
        }
        match group {
            ChoiceGroup::Sync => self.draft.sync = index == 1,
            ChoiceGroup::Backup => self.draft.restore_backup = index == 1,
            ChoiceGroup::InitialRepository => self.draft.add_repository = Some(index == 1),
            ChoiceGroup::Theme => {
                self.draft.theme = ["dark", "light"][index].into();
                gpui_kit::component::Theme::change(
                    crate::theme_mode_from_name(&self.draft.theme),
                    Some(window),
                    cx,
                );
                crate::apply_focus_theme(cx);
            }
            ChoiceGroup::Editor => {
                self.draft.editor = [EditorChoice::BuiltIn, EditorChoice::Neovim][index]
                    .id()
                    .into()
            }
        }
        self.save_draft(cx);
    }

    fn choice(
        &self,
        group: ChoiceGroup,
        index: usize,
        icon: impl IntoElement,
        text: (&str, &str),
        cx: &mut Context<Self>,
    ) -> gpui_kit::base::Radio {
        self.choice_radio(group, index, cx)
            .accessibility_label(text.0.to_owned())
            .child(selection_card::content(icon, text.0, text.1, cx))
    }

    fn choice_radio(
        &self,
        group: ChoiceGroup,
        index: usize,
        cx: &mut Context<Self>,
    ) -> gpui_kit::base::Radio {
        let selected = match group {
            ChoiceGroup::Sync => self.draft.sync == (index == 1),
            ChoiceGroup::Backup => self.draft.restore_backup == (index == 1),
            ChoiceGroup::InitialRepository => self.draft.wants_repository() == (index == 1),
            ChoiceGroup::Theme => self.draft.theme == ["dark", "light"][index],
            ChoiceGroup::Editor => {
                self.draft.editor == [EditorChoice::BuiltIn, EditorChoice::Neovim][index].id()
            }
        };
        let focus = self.choice_focus[group as usize][index].clone();
        let view = cx.entity().downgrade();
        selection_card::radio((group.id(), index), selected, self.busy, cx)
            .when(matches!(group, ChoiceGroup::Backup), |radio| {
                radio
                    .flex_none()
                    .px_1()
                    .py_1()
                    .bg(gpui_kit::transparent_black())
                    .border_color(gpui_kit::transparent_black())
            })
            .track_focus(&focus)
            .set_position(index + 1, 2)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                focus.focus(window, cx);
                cx.stop_propagation();
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let next = match event.keystroke.key.as_str() {
                    "space" | "enter" => index,
                    "arrowdown" | "arrowright" | "down" | "right" => 1,
                    "arrowup" | "arrowleft" | "up" | "left" => 0,
                    _ => return,
                };
                this.select_choice(group, next, window, cx);
                this.choice_focus[group as usize][next].focus(window, cx);
                window.prevent_default();
                cx.stop_propagation();
            }))
            .on_change(move |_, _, window, cx| {
                let _ = view.update(cx, |this, cx| this.select_choice(group, index, window, cx));
            })
    }

    fn agents(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex().gap_4()
            .child(self.heading("Work with the agents you trust.", "Choose the harnesses and default agent for this device.", cx))
            .child(v_flex().gap_2().children(AgentKind::ALL.into_iter().enumerate().map(|(index, agent)| {
                let enabled = self.draft.enabled_agents.iter().any(|id| id == agent.id());
                let default = self.draft.default_agent == agent.id();
                h_flex().gap_3().p_3().rounded_lg().border_1().border_color(if default { rgb(0x2f81f7).into() } else { cx.theme().border })
                    .child(agent_icons::agent_icon(agent, &self.icons, 28.))
                    .child(v_flex().gap_1().flex_1().min_w_0()
                        .child(div().font_semibold().text_sm().child(agent.label()))
                        .child(div().text_xs().text_color(cx.theme().muted_foreground).child(agent.description())))
                    .child(Button::new(("setup-default-agent", index)).small().ghost()
                        .label(if default { "✓ Default" } else { "Make default" }).disabled(!enabled)
                        .on_click(cx.listener(move |this, _, _, cx| { this.draft.default_agent = agent.id().into(); this.save_draft(cx); })))
                    .child(Switch::new(("setup-agent", index)).accessibility_label(format!("Enable {}", agent.label())).checked(enabled)
                        .disabled(enabled && self.draft.enabled_agents.len() == 1)
                        .on_change(cx.listener(move |this, enabled, _, cx| { this.draft.set_agent_enabled(agent, *enabled); this.save_draft(cx); })))
            })))
            .when(!self.draft.restoring(), |view| view.child(self.agent_isolation(cx)))
            .when(self.draft.restoring(), |view| view.child(self.note("Your saved spaces and their agent isolation settings will be restored. Sign in to each agent on this device after setup. You can change space isolation in Settings.", cx)))
            .child(self.note("At least one harness stays enabled. Install your selected CLIs and sign in before starting a session. Setup does not install or launch agents, and you can change these choices in Settings → Agent.", cx))
            .into_any_element()
    }

    fn preferences(&self, cx: &mut Context<Self>) -> AnyElement {
        v_flex().gap_4()
            .child(self.heading("Make yourself at home.", "A couple of everyday defaults. Everything can be changed in Settings.", cx))
            .child(div().font_semibold().text_sm().child("Appearance"))
            .child(selection_card::group("setup-theme-options").aria_label("Appearance").children([
                (gpui_kit::assets::IconName::Moon, "Dark", "A quieter canvas for focused work."),
                (gpui_kit::assets::IconName::Sun, "Light", "A bright workspace for your day."),
            ].into_iter().enumerate().map(|(index, (icon, title, detail))| {
                    self.choice(ChoiceGroup::Theme, index, Icon::new(icon).size(px(20.)), (title, detail), cx)
                })))
            .child(div().font_semibold().text_sm().child("Default editor"))
            .child(selection_card::group("setup-editor-options").aria_label("Default editor").children([
                (EditorIcon::Devcroft, "Built in · Ready to use", "File tabs, project search, and syntax highlighting. No installation needed."),
                (EditorIcon::Neovim, "Neovim", "Your terminal editor and configuration. Requires nvim on your shell's PATH."),
            ].into_iter().enumerate().map(|(index, (icon, title, detail))| {
                self.choice(ChoiceGroup::Editor, index, editor_icons::editor_icon(icon, &self.editor_icons, editor_icons::ICON_PX), (title, detail), cx)
            })))
            .into_any_element()
    }

    fn review(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = [
            (
                "Space",
                if self.draft.restoring() {
                    "Restore saved spaces".into()
                } else {
                    self.draft.space.trim().to_owned()
                },
                if self.draft.restoring() {
                    STEP_SYNC
                } else {
                    STEP_WORKSPACE
                },
            ),
            (
                "Repository",
                if self.draft.restoring() {
                    "Restore saved repositories; link local checkouts from Home".into()
                } else if !self.draft.wants_repository() {
                    "Add one later".into()
                } else {
                    format!(
                        "{} · {}\n{}",
                        self.draft.repository.display_name.trim(),
                        self.draft.repository.base_branch.trim(),
                        self.draft.checkout.trim()
                    )
                },
                if self.draft.restoring() {
                    STEP_SYNC
                } else {
                    STEP_REPOSITORY
                },
            ),
            (
                "Agent accounts",
                if self.draft.restoring() {
                    "Use saved space isolation settings"
                } else if self.draft.isolate_agents {
                    "Separate for this space"
                } else {
                    "Use existing shared accounts"
                }
                .into(),
                STEP_AGENTS,
            ),
            (
                "Sync",
                if self.draft.sync {
                    format!(
                        "{} · {}",
                        if self.draft.restore_backup {
                            "Restore existing backup"
                        } else {
                            "New Git backup"
                        },
                        if self.draft.auto_sync {
                            format!("Every {} minutes", self.draft.sync_interval_minutes)
                        } else {
                            "Manual sync".into()
                        }
                    )
                } else {
                    "Local only".into()
                },
                STEP_SYNC,
            ),
            (
                "Agents",
                format!(
                    "{} enabled · {} by default",
                    self.draft.enabled_agents.len(),
                    self.draft.default_agent
                ),
                2,
            ),
            (
                "Environment",
                format!(
                    "{} · {} theme",
                    EditorChoice::parse(&self.draft.editor)
                        .unwrap_or(EditorChoice::BuiltIn)
                        .label(),
                    self.draft.theme
                ),
                3,
            ),
        ];
        v_flex().gap_4()
            .child(self.heading("You're ready to build.", "Here's your starting point. Review your choices, then make it official.", cx))
            .child(v_flex().rounded_lg().border_1().border_color(cx.theme().border)
                .children(rows.into_iter().enumerate().map(|(index, (label, value, step))| {
                    h_flex().gap_3().p_3().border_b_1().border_color(cx.theme().border)
                        .child(div().w(px(115.)).text_sm().text_color(cx.theme().muted_foreground).child(label))
                        .child(div().flex_1().min_w_0().text_sm().child(value))
                        .child(Button::new(("setup-edit", index)).ghost().small().label("Edit").disabled(self.busy)
                            .accessibility_label(format!("Edit {label}"))
                            .on_click(cx.listener(move |this, _, window, cx| this.go_to(step, window, cx))))
                })))
            .child(self.note("Next stop: Home. Add projects, start a session, or explore your workspace. Your settings remain a click away in the actions palette.", cx))
            .into_any_element()
    }
}

impl Render for OnboardingView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let step = self.draft.step;
        let steps = self.draft.steps();
        let position = steps.iter().position(|active| *active == step).unwrap_or(0);
        let next_step = self.draft.next_step();
        let previous_step = self.draft.previous_step();
        let body = match step {
            STEP_SYNC => self.sync(cx),
            STEP_WORKSPACE => self.workspace(cx),
            STEP_AGENTS => self.agents(cx),
            STEP_PREFERENCES => self.preferences(cx),
            STEP_REPOSITORY => self.initial_repository(cx),
            _ => self.review(cx),
        };
        div()
            .id("onboarding")
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .flex()
            .items_center()
            .justify_center()
            .p_6()
            .child(
                v_flex()
                    .id("setup-panel")
                    .w_full()
                    .max_w(px(760.))
                    .max_h(
                        (window.viewport_size().height - px(48.))
                            .max(px(0.))
                            .min(px(600.)),
                    )
                    .rounded_2xl()
                    .border_1()
                    .border_color(cx.theme().border)
                    .shadow_lg()
                    .overflow_hidden()
                    .child(
                        h_flex()
                            .flex_none()
                            .px_6()
                            .py_4()
                            .justify_between()
                            .border_b_1()
                            .border_color(cx.theme().border)
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(rgb(0x2f81f7))
                                    .child("WELCOME TO DEVCROFT"),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Step {} of {}", position + 1, steps.len())),
                            ),
                    )
                    .child(
                        v_flex().flex_1().min_h_0().overflow_hidden().child(
                            div()
                                .size_full()
                                .overflow_y_scrollbar()
                                .id("setup-content")
                                .p_6()
                                .child(body)
                                .when_some(self.error.clone(), |view, error| {
                                    view.child(
                                        div()
                                            .id("setup-error")
                                            .mt_4()
                                            .p_3()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(cx.theme().danger)
                                            .text_sm()
                                            .text_color(cx.theme().danger)
                                            .child(error),
                                    )
                                }),
                        ),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .px_6()
                            .py_4()
                            .gap_3()
                            .border_t_1()
                            .border_color(cx.theme().border)
                            .child(
                                Button::new("setup-back")
                                    .ghost()
                                    .label("Back")
                                    .disabled(step == 0 || self.busy)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.go_to(previous_step, window, cx)
                                    })),
                            )
                            .when(step == 0, |row| {
                                row.child(
                                    Button::new("setup-defaults")
                                        .ghost()
                                        .label("Use defaults")
                                        .disabled(self.busy || self.checking_repository)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.draft = SetupDraft::default();
                                            gpui_kit::component::Theme::change(
                                                crate::theme_mode_from_name(&this.draft.theme),
                                                Some(window),
                                                cx,
                                            );
                                            crate::apply_focus_theme(cx);
                                            this.space_input.update(cx, |input, cx| {
                                                input.set_value(data::DEFAULT_SPACE, window, cx)
                                            });
                                            this.checkout_input.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                            this.remote_input.update(cx, |input, cx| {
                                                input.set_value("", window, cx)
                                            });
                                            this.repository_form.set_values(
                                                &RepositoryDraft::default(),
                                                window,
                                                cx,
                                            );
                                            this.inspected_checkout = None;
                                            this.go_to(STEP_REVIEW, window, cx);
                                        })),
                                )
                            })
                            .child(div().flex_1())
                            .child(
                                Button::new("setup-next")
                                    .custom(
                                        ButtonCustomVariant::new(cx)
                                            .color(rgb(0x2f81f7).into())
                                            .foreground(rgb(0xffffff).into())
                                            .hover(rgb(0x236bce).into())
                                            .active(rgb(0x1b55a6).into()),
                                    )
                                    .bg(rgb(0x2f81f7))
                                    .label(if self.busy {
                                        "Saving setup…"
                                    } else if self.checking_repository {
                                        "Reading repository…"
                                    } else if step == STEP_REVIEW {
                                        "Open Devcroft"
                                    } else {
                                        "Continue →"
                                    })
                                    .disabled(self.busy || self.checking_repository)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if step == STEP_REVIEW {
                                            this.finish(window, cx);
                                        } else {
                                            this.go_to(next_step, window, cx);
                                        }
                                    })),
                            ),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::NewRepositoryInput;

    fn fresh() -> (tempfile::TempDir, DataRoot) {
        let directory = tempfile::tempdir().unwrap();
        let root =
            data::ensure_ready_with_override(Some(directory.path().to_owned()), None).unwrap();
        (directory, root)
    }

    #[test]
    fn draft_survives_restart_without_applying_settings() {
        let (_directory, root) = fresh();
        let draft = SetupDraft {
            step: 3,
            space: "Client work".into(),
            add_repository: Some(true),
            checkout: "/tmp/my-project".into(),
            repository: RepositoryDraft {
                key: "client-project".into(),
                display_name: "Client project".into(),
                description: "Keep in draft".into(),
                base_branch: "develop".into(),
                ..Default::default()
            },
            repository_details: true,
            isolate_agents: true,
            sync: true,
            remote: "git@example.com:you/data.git".into(),
            auto_sync: true,
            sync_interval_minutes: 5,
            ..Default::default()
        };
        draft.save(&root).unwrap();
        let resumed = SetupDraft::load(&root);
        assert_eq!(resumed.space, "Client work");
        assert_eq!(resumed.step, 3);
        assert!(resumed.isolate_agents);
        assert!(resumed.auto_sync);
        assert_eq!(resumed.sync_interval_minutes, 5);
        assert!(resumed.wants_repository());
        assert_eq!(resumed.repository, draft.repository);
        assert!(resumed.repository_details);
        let state = DeviceStore::new(&root).load().unwrap();
        assert_eq!(state.onboarding_complete, Some(false));
        assert_eq!(state.active_space, None);
        assert_eq!(data::get_origin(&root).unwrap(), None);
        assert!(
            !root
                .portable_dir()
                .join("repositories/client-project")
                .exists()
        );
    }

    #[test]
    fn start_empty_ignores_dormant_repository_fields() {
        let (_directory, root) = fresh();
        let mut draft = SetupDraft {
            add_repository: Some(true),
            ..Default::default()
        };
        assert!(
            draft
                .validate_step(STEP_REPOSITORY)
                .unwrap_err()
                .to_string()
                .contains("Choose a local Git checkout")
        );
        draft.add_repository = Some(false);
        draft.checkout = "/not/a/git/checkout".into();
        draft.repository.key = "Invalid KEY".into();
        draft.apply(&root).unwrap();
        assert!(
            data::list_repositories(&root, 10)
                .unwrap()
                .repositories
                .is_empty()
        );
        assert_eq!(
            DeviceStore::new(&root).load().unwrap().onboarding_complete,
            Some(true)
        );
    }

    #[test]
    fn initial_repository_applies_custom_metadata_in_the_onboarding_space() {
        let (_directory, root) = fresh();
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(checkout.path())
                .status()
                .unwrap()
                .success()
        );
        let mut draft = SetupDraft {
            space: "Client work".into(),
            add_repository: Some(true),
            checkout: checkout.path().to_string_lossy().into_owned(),
            repository: RepositoryDraft {
                key: "client-project".into(),
                display_name: "Client project".into(),
                owner: "client".into(),
                name: "project".into(),
                clone_url: "https://example.com/client/project.git".into(),
                base_branch: "develop".into(),
                description: "The client application".into(),
                tags: "rust, desktop, rust".into(),
            },
            ..Default::default()
        };
        draft.save(&root).unwrap();
        assert!(
            data::list_repositories(&root, 10)
                .unwrap()
                .repositories
                .is_empty()
        );
        draft.apply(&root).unwrap();
        // Retrying completion uses the durable intent and keeps a single record.
        draft.apply(&root).unwrap();
        let metadata = data::get_repository_metadata(&root, "client-project").unwrap();
        assert_eq!(metadata.display_name.as_deref(), Some("Client project"));
        assert_eq!(metadata.owner.as_deref(), Some("client"));
        assert_eq!(metadata.name.as_deref(), Some("project"));
        assert_eq!(
            metadata.clone_url.as_deref(),
            Some("https://example.com/client/project.git")
        );
        assert_eq!(metadata.base_branch.as_deref(), Some("develop"));
        assert_eq!(
            metadata.description.as_deref(),
            Some("The client application")
        );
        assert_eq!(metadata.tags, Some(vec!["rust".into(), "desktop".into()]));
        assert_eq!(metadata.space.as_deref(), Some("Client work"));
        assert_eq!(
            data::checkout_for(&root, "client-project"),
            Some(std::fs::canonicalize(checkout.path()).unwrap())
        );
        assert_eq!(
            data::list_repositories(&root, 10)
                .unwrap()
                .repositories
                .len(),
            1
        );
    }

    #[test]
    fn changing_checkout_after_restart_refreshes_detected_fields_and_preserves_edits() {
        let (_directory, root) = fresh();
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        for (path, remote) in [
            (first.path(), "https://example.com/first/project-a.git"),
            (second.path(), "https://example.com/second/project-b.git"),
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(["init", "-q"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(path)
                    .args(["remote", "add", "origin", remote])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let inspection = data::inspect_checkout(first.path()).unwrap();
        let mut draft = SetupDraft {
            add_repository: Some(true),
            checkout: second.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        draft.repository_detection =
            Some(draft.repository.refresh_from_checkout(&inspection, None));
        draft.repository.display_name = "My edited project".into();
        draft.repository.base_branch = "develop".into();
        draft.save(&root).unwrap();
        let mut resumed = SetupDraft::load(&root);
        resumed.apply(&root).unwrap();
        let expected = data::inspect_checkout(second.path()).unwrap();
        let metadata = data::get_repository_metadata(&root, &expected.suggested_key).unwrap();
        assert_eq!(metadata.display_name.as_deref(), Some("My edited project"));
        assert_eq!(metadata.base_branch.as_deref(), Some("develop"));
        assert_eq!(metadata.owner.as_deref(), Some("second"));
        assert_eq!(metadata.name.as_deref(), Some("project-b"));
        assert_eq!(metadata.clone_url, expected.clone_url);
        assert!(!data::repository_dir(&root, &inspection.suggested_key).exists());
    }

    #[test]
    fn interrupted_repository_creation_accepts_metadata_edits_before_retry() {
        let (_directory, root) = fresh();
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(checkout.path())
                .status()
                .unwrap()
                .success()
        );
        let inspection = data::inspect_checkout(checkout.path()).unwrap();
        let original = RepositoryDraft {
            key: "custom-key".into(),
            name: "original-name".into(),
            clone_url: "https://example.com/original/project.git".into(),
            ..Default::default()
        };
        data::create_repository(
            &root,
            "custom-key",
            checkout.path(),
            &original.input(data::DEFAULT_SPACE),
        )
        .unwrap();
        data::unlink_repository(&root, "custom-key").unwrap();
        let mut draft = SetupDraft {
            add_repository: Some(true),
            checkout: checkout.path().to_string_lossy().into_owned(),
            repository: RepositoryDraft {
                name: "edited-name".into(),
                clone_url: "https://example.com/edited/project.git".into(),
                ..original.clone()
            },
            repository_intent: Some((
                "custom-key".into(),
                inspection.checkout_path.to_string_lossy().into_owned(),
            )),
            repository_intent_metadata: Some(original),
            ..Default::default()
        };
        draft.save(&root).unwrap();
        draft = SetupDraft::load(&root);
        draft.apply(&root).unwrap();
        let metadata = data::get_repository_metadata(&root, "custom-key").unwrap();
        assert_eq!(metadata.name.as_deref(), Some("edited-name"));
        assert_eq!(
            metadata.clone_url.as_deref(),
            Some("https://example.com/edited/project.git")
        );
        draft.repository.name = "edited-again".into();
        draft.apply(&root).unwrap();
        assert_eq!(
            data::get_repository_metadata(&root, "custom-key")
                .unwrap()
                .name
                .as_deref(),
            Some("edited-again")
        );
    }

    #[test]
    fn sync_frequency_applies_settings_options_and_respects_manual_or_local_mode() {
        for (sync, automatic, minutes, expected) in data::SYNC_INTERVAL_OPTIONS
            .into_iter()
            .map(|minutes| (true, true, minutes, Some(minutes)))
            .chain([(true, false, 5, None), (false, true, 5, None)])
        {
            let (_directory, root) = fresh();
            let mut draft = SetupDraft {
                sync,
                remote: "git@example.com:you/data.git".into(),
                auto_sync: automatic,
                sync_interval_minutes: minutes,
                ..Default::default()
            };
            draft.apply(&root).unwrap();
            assert_eq!(
                DeviceStore::new(&root)
                    .load()
                    .unwrap()
                    .sync_interval_minutes,
                expected
            );
        }
    }

    #[test]
    fn older_drafts_and_unsupported_frequencies_resume_with_fifteen_minutes() {
        let (_directory, root) = fresh();
        for draft in [
            serde_json::json!({ "auto_sync": true }),
            serde_json::json!({ "auto_sync": true, "sync_interval_minutes": 0 }),
        ] {
            DeviceStore::new(&root)
                .update(|state| state.onboarding_draft = Some(draft))
                .unwrap();
            let resumed = SetupDraft::load(&root);
            assert!(resumed.auto_sync);
            assert_eq!(resumed.sync_interval_minutes, 15);
        }
    }

    #[test]
    fn completion_applies_choices_and_preserves_unrelated_device_state() {
        let (_directory, root) = fresh();
        DeviceStore::new(&root)
            .update(|state| {
                state.app_font_size = Some(17.);
                state.extra.insert("future".into(), serde_json::json!(42));
            })
            .unwrap();
        let mut draft = SetupDraft {
            space: "  Client   work ".into(),
            isolate_agents: true,
            sync: true,
            remote: "git@example.com:you/data.git".into(),
            auto_sync: true,
            enabled_agents: vec!["codex".into()],
            default_agent: "codex".into(),
            theme: "light".into(),
            ..Default::default()
        };
        draft.save(&root).unwrap();
        draft.apply(&root).unwrap();
        let state = DeviceStore::new(&root).load().unwrap();
        assert_eq!(state.onboarding_complete, Some(true));
        assert!(state.onboarding_draft.is_none());
        assert_eq!(state.active_space.as_deref(), Some("Client work"));
        assert_eq!(state.default_agent_or_default(), AgentKind::Codex);
        assert_eq!(state.enabled_agents_or_default(), vec![AgentKind::Codex]);
        assert_eq!(state.editor_choice_or_default(), EditorChoice::BuiltIn);
        assert_eq!(state.theme.as_deref(), Some("light"));
        assert_eq!(state.sync_interval_minutes, Some(15));
        assert_eq!(state.app_font_size, Some(17.));
        assert_eq!(state.extra["future"], serde_json::json!(42));
        assert_eq!(
            data::get_origin(&root).unwrap().as_deref(),
            Some("git@example.com:you/data.git")
        );
        let spaces = data::Spaces::load(&root).unwrap();
        assert!(spaces.spaces.iter().any(|space| space.name == "Client work"
            && space.isolate_agents
            && space.agent_account_id.is_some()));
        // No network fetch or initial commit is part of setup.
        assert!(!root.portable_dir().join(".git/FETCH_HEAD").exists());
        assert!(!root.portable_dir().join(".git/refs/heads/main").exists());
    }

    #[test]
    fn invalid_choices_do_not_complete_or_mutate_portable_data() {
        let (_directory, root) = fresh();
        let before = DeviceStore::new(&root).load().unwrap();
        for mut draft in [
            SetupDraft {
                space: " ".into(),
                ..Default::default()
            },
            SetupDraft {
                sync: true,
                ..Default::default()
            },
            SetupDraft {
                sync: true,
                remote: "-bad-url".into(),
                ..Default::default()
            },
            SetupDraft {
                sync: true,
                remote: "git@example.com:you/data.git".into(),
                auto_sync: true,
                sync_interval_minutes: 0,
                ..Default::default()
            },
            SetupDraft {
                enabled_agents: vec![],
                ..Default::default()
            },
            SetupDraft {
                enabled_agents: vec!["codex".into()],
                ..Default::default()
            },
            SetupDraft {
                checkout: root
                    .root()
                    .join("not-a-checkout")
                    .to_string_lossy()
                    .into_owned(),
                ..Default::default()
            },
        ] {
            assert!(draft.apply(&root).is_err());
            assert_eq!(DeviceStore::new(&root).load().unwrap(), before);
            assert!(!root.portable_dir().join("spaces.json").exists());
        }
    }

    #[test]
    fn last_harness_is_retained_and_default_follows_enabled_set() {
        let mut draft = SetupDraft::default();
        for agent in AgentKind::ALL {
            draft.set_agent_enabled(agent, false);
        }
        assert_eq!(draft.enabled_agents, vec!["omp"]);
        assert_eq!(draft.default_agent, "omp");
        draft.set_agent_enabled(AgentKind::Codex, true);
        draft.set_agent_enabled(AgentKind::Omp, false);
        assert_eq!(draft.default_agent, "codex");
    }

    #[test]
    fn repository_retries_repair_interrupted_binding_and_do_not_duplicate_records() {
        let (_directory, root) = fresh();
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(checkout.path())
                .status()
                .unwrap()
                .success()
        );
        let inspection = data::inspect_checkout(checkout.path()).unwrap();
        let draft = SetupDraft {
            checkout: inspection.checkout_path.to_string_lossy().into_owned(),
            repository_intent: Some((
                inspection.suggested_key.clone(),
                inspection.checkout_path.to_string_lossy().into_owned(),
            )),
            ..Default::default()
        };
        data::create_repository(
            &root,
            &inspection.suggested_key,
            checkout.path(),
            &NewRepositoryInput::default(),
        )
        .unwrap();
        data::unlink_repository(&root, &inspection.suggested_key).unwrap();
        draft.save(&root).unwrap();
        let mut draft = SetupDraft::load(&root);
        draft.apply(&root).unwrap();
        draft.apply(&root).unwrap();
        assert_eq!(
            data::checkout_for(&root, &inspection.suggested_key),
            Some(inspection.checkout_path)
        );
        assert_eq!(
            std::fs::read_dir(root.portable_dir().join("repositories"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            DeviceStore::new(&root).load().unwrap().onboarding_complete,
            Some(true)
        );
    }

    #[test]
    fn repository_collision_is_not_adopted_without_creation_intent() {
        let (_directory, root) = fresh();
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(checkout.path())
                .status()
                .unwrap()
                .success()
        );
        let inspection = data::inspect_checkout(checkout.path()).unwrap();
        data::create_repository(
            &root,
            &inspection.suggested_key,
            checkout.path(),
            &NewRepositoryInput::default(),
        )
        .unwrap();
        data::unlink_repository(&root, &inspection.suggested_key).unwrap();
        let mut draft = SetupDraft {
            checkout: checkout.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        assert!(
            draft
                .apply(&root)
                .unwrap_err()
                .to_string()
                .contains("different repository")
        );
        assert_eq!(
            DeviceStore::new(&root).load().unwrap().onboarding_complete,
            Some(false)
        );
        assert!(data::checkout_for(&root, &inspection.suggested_key).is_none());
    }

    #[test]
    fn step_order_skips_fresh_configuration_when_restoring() {
        let mut draft = SetupDraft::default();
        assert_eq!(draft.steps(), &[0, 1, 2, 3, 4, 5]);
        draft.sync = true;
        draft.restore_backup = true;
        draft.add_repository = Some(true);
        draft.checkout = "/missing-checkout".into();
        draft.space = " ".into();
        assert_eq!(draft.steps(), &[0, 2, 3, 5]);
        assert!(!draft.wants_repository());
        assert_eq!(draft.next_step(), STEP_AGENTS);
        draft.step = STEP_REVIEW;
        assert_eq!(draft.previous_step(), STEP_PREFERENCES);
        assert!(draft.validate_step(STEP_WORKSPACE).is_ok());
        assert!(draft.validate_step(STEP_REPOSITORY).is_ok());
        draft.sync = false;
        assert!(draft.wants_repository());
        assert_eq!(draft.previous_step(), STEP_REPOSITORY);
    }

    #[test]
    fn legacy_drafts_resume_at_the_equivalent_visible_step() {
        let (_directory, root) = fresh();
        for (old, new) in [
            STEP_WORKSPACE,
            STEP_SYNC,
            STEP_AGENTS,
            STEP_PREFERENCES,
            STEP_REVIEW,
        ]
        .into_iter()
        .enumerate()
        {
            DeviceStore::new(&root)
                .update(|state| state.onboarding_draft = Some(serde_json::json!({"step": old})))
                .unwrap();
            let resumed = SetupDraft::load(&root);
            assert_eq!(resumed.step, new);
            assert_eq!(resumed.flow_version, FLOW_VERSION);
        }
        DeviceStore::new(&root)
            .update(|state| {
                state.onboarding_draft =
                    Some(serde_json::json!({"step": 0, "sync": true, "restore_backup": true}))
            })
            .unwrap();
        assert_eq!(SetupDraft::load(&root).step, STEP_AGENTS);
    }

    #[test]
    fn restore_preserves_saved_spaces_isolation_and_repositories() {
        let (_directory, root) = fresh();
        let checkout = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(checkout.path())
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://example.com/team/project.git",
        ]);
        let mut spaces = data::Spaces::default();
        spaces.ensure_name("Original space");
        spaces.set_agent_isolation("Original space", true).unwrap();
        spaces.save(&root, &data::Spaces::default()).unwrap();
        let spaces = data::Spaces::load(&root).unwrap();
        data::create_repository(
            &root,
            "saved-custom-key",
            checkout.path(),
            &NewRepositoryInput {
                display_name: Some("Saved project".into()),
                description: Some("Restored description".into()),
                space: Some("Original space".into()),
                tags: vec!["saved-tag".into()],
                base_branch: Some("release".into()),
                ..Default::default()
            },
        )
        .unwrap();
        data::unlink_repository(&root, "saved-custom-key").unwrap();
        let original = data::get_repository_metadata(&root, "saved-custom-key").unwrap();
        let remote = "https://example.com/you/devcroft-backup.git";
        // Model a restart after the validated clone was adopted, before the
        // local checkout binding and completion write succeeded.
        std::fs::write(
            root.portable_dir().join(".git/devcroft-onboarding-source"),
            remote,
        )
        .unwrap();
        let mut draft = SetupDraft {
            sync: true,
            restore_backup: true,
            remote: remote.into(),
            space: " ".into(),
            isolate_agents: false,
            add_repository: Some(true),
            checkout: "/missing/dormant/checkout".into(),
            ..Default::default()
        };
        draft.apply(&root).unwrap();
        assert_eq!(
            data::get_repository_metadata(&root, "saved-custom-key").unwrap(),
            original
        );
        assert!(data::checkout_for(&root, "saved-custom-key").is_none());
        assert_eq!(data::Spaces::load(&root).unwrap(), spaces);
        assert_eq!(
            DeviceStore::new(&root)
                .load()
                .unwrap()
                .active_space
                .as_deref(),
            Some("Original space")
        );
        assert_eq!(
            DeviceStore::new(&root).load().unwrap().onboarding_complete,
            Some(true)
        );
        assert_eq!(
            std::fs::read_dir(root.portable_dir().join("repositories"))
                .unwrap()
                .count(),
            1
        );
    }

    #[gpui_kit::test]
    fn initial_repository_choice_detects_details_and_keeps_edits_in_draft(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (_directory, root) = fresh();
        SetupDraft {
            step: STEP_REPOSITORY,
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let checkout = tempfile::tempdir().unwrap();
        assert!(
            std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(checkout.path())
                .status()
                .unwrap()
                .success()
        );
        let inspection = data::inspect_checkout(checkout.path()).unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|cx| OnboardingView::new(root, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(!view.read(cx).draft.wants_repository());
            assert!(window.find_all("setup-browse").is_empty());
            let empty = window.find(("setup-initial-repository", 0usize)).bounds();
            let add = window.find(("setup-initial-repository", 1usize)).bounds();
            assert_eq!(empty.top(), add.top());
            assert!(empty.right() <= add.left());
            window.click(("setup-initial-repository", 1usize), cx);
            assert!(view.read(cx).draft.wants_repository());
            assert!(window.find("setup-browse").visible());
            assert!(window.find_all("setup-repository-fields").is_empty());
            window.scroll(
                "setup-browse",
                gpui_kit::ScrollDelta::Lines(gpui_kit::point(0., -20.)),
                cx,
            );
            assert!(window.find(("trigger", 0usize)).visible());
            assert!(
                window.find(("trigger", 0usize)).bounds().bottom()
                    < window.find("setup-next").bounds().top(),
                "The accordion must be fully above the fixed navigation"
            );
            window.click(("trigger", 0usize), cx);
            assert!(view.read(cx).draft.repository_details);
            view.update(cx, |this, cx| {
                this.checkout_input.update(cx, |input, cx| {
                    input.set_value(checkout.path().to_string_lossy().to_string(), window, cx)
                });
                this.repository_form.display_name.update(cx, |input, cx| {
                    input.set_value("My edited project", window, cx)
                });
                this.go_to(STEP_REVIEW, window, cx);
            });
            assert!(view.read(cx).checking_repository);
            assert_eq!(view.read(cx).draft.step, STEP_REPOSITORY);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            let this = view.read(cx);
            assert!(!this.checking_repository);
            assert!(this.error.is_none(), "{:?}", this.error);
            assert_eq!(this.draft.step, STEP_REVIEW);
            assert_eq!(this.draft.repository.key, inspection.suggested_key);
            assert_eq!(this.draft.repository.base_branch, inspection.base_branch);
            assert_eq!(this.draft.repository.display_name, "My edited project");
            assert_eq!(
                SetupDraft::load(&this.root).repository,
                this.draft.repository
            );
            assert!(
                data::list_repositories(&this.root, 10)
                    .unwrap()
                    .repositories
                    .is_empty()
            );
            window.click("setup-back", cx);
            window.click(("setup-initial-repository", 0usize), cx);
            assert!(!view.read(cx).draft.wants_repository());
            assert!(window.find_all("setup-browse").is_empty());
            window.press("right", cx);
            assert!(view.read(cx).draft.wants_repository());
            assert_eq!(
                view.read(cx)
                    .repository_form
                    .display_name
                    .read(cx)
                    .value()
                    .as_ref(),
                "My edited project"
            );
        });
    }

    #[gpui_kit::test]
    fn restoring_skips_creation_steps_and_route_changes_restore_them(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (_directory, root) = fresh();
        SetupDraft {
            sync: true,
            restore_backup: true,
            remote: "https://example.com/you/backup.git".into(),
            add_repository: Some(true),
            checkout: "/missing/dormant-checkout".into(),
            ..Default::default()
        }
        .save(&root)
        .unwrap();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|cx| OnboardingView::new(root, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_AGENTS);
            assert!(window.find_all("setup-isolation").is_empty());
            assert!(
                window
                    .find_all("setup-initial-repository-options")
                    .is_empty()
            );
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_PREFERENCES);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_REVIEW);
            window.click("setup-back", cx);
            assert_eq!(view.read(cx).draft.step, STEP_PREFERENCES);
            window.click("setup-back", cx);
            window.click("setup-back", cx);
            assert_eq!(view.read(cx).draft.step, STEP_SYNC);
            window.click(("setup-backup-mode", 0usize), cx);
            assert!(!view.read(cx).draft.restoring());
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_WORKSPACE);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_AGENTS);
            window.scroll(
                ("setup-default-agent", 0usize),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-1000.))),
                cx,
            );
            assert!(window.find("setup-isolation").visible());
            window.click("setup-next", cx);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_REPOSITORY);
            assert!(window.find("setup-browse").visible());
            assert_eq!(view.read(cx).draft.checkout, "/missing/dormant-checkout");
            assert!(
                data::list_repositories(&view.read(cx).root, 10)
                    .unwrap()
                    .repositories
                    .is_empty()
            );
            assert_eq!(
                DeviceStore::new(&view.read(cx).root)
                    .load()
                    .unwrap()
                    .onboarding_complete,
                Some(false)
            );
        });
    }

    #[gpui_kit::test]
    fn wizard_navigation_validates_sync_and_keeps_draft(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_kit::init);
        let (_directory, root) = fresh();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let view = cx.new(|cx| OnboardingView::new(root, window, cx));
            *slot.borrow_mut() = Some(view.clone());
            gpui_kit::component::Root::new(view, window, cx)
        });
        let view = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(view.read(cx).draft.step, STEP_SYNC);
            let local = window.find(("setup-sync", 0usize)).bounds();
            let sync = window.find(("setup-sync", 1usize)).bounds();
            assert_eq!(local.top(), sync.top());
            assert!((local.size.width - sync.size.width).abs() < px(1.));
            assert!(local.right() <= sync.left());
            window.click(("setup-sync", 1usize), cx);
            let new_repository = window.find(("setup-backup-mode", 0usize)).bounds();
            let existing_repository = window.find(("setup-backup-mode", 1usize)).bounds();
            assert_eq!(new_repository.top(), existing_repository.top());
            assert!(new_repository.right() <= existing_repository.left());
            window.click(("setup-backup-mode", 1usize), cx);
            assert!(view.read(cx).draft.restore_backup);
            assert!(SetupDraft::load(&view.read(cx).root).restore_backup);
            window.press("left", cx);
            assert!(!view.read(cx).draft.restore_backup);
            assert!(!SetupDraft::load(&view.read(cx).root).restore_backup);
            assert!(window.find_all(("setup-sync-interval", 5usize)).is_empty());
            window.click("setup-auto-sync", cx);
            assert!(window.find(("setup-sync-interval", 5usize)).visible());
            window.click(("setup-sync-interval", 5usize), cx);
            assert_eq!(view.read(cx).draft.sync_interval_minutes, 5);
            assert_eq!(
                SetupDraft::load(&view.read(cx).root).sync_interval_minutes,
                5
            );
            window.click("setup-auto-sync", cx);
            assert!(window.find_all(("setup-sync-interval", 5usize)).is_empty());
            window.click("setup-auto-sync", cx);
            assert_eq!(view.read(cx).draft.sync_interval_minutes, 5);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_SYNC);
            assert!(
                view.read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("remote URL")
            );
            window.click(("setup-sync", 0usize), cx);
            assert!(window.find_all(("setup-sync-interval", 5usize)).is_empty());
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_WORKSPACE);
            assert!(window.find_all("setup-isolation").is_empty());
            assert!(
                window
                    .find_all("setup-initial-repository-options")
                    .is_empty()
            );
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_AGENTS);
            window.scroll(
                ("setup-default-agent", 0usize),
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-1000.))),
                cx,
            );
            assert!(window.find("setup-isolation").visible());
            window.click("setup-isolation", cx);
            assert!(view.read(cx).draft.isolate_agents);
            window.scroll(
                "setup-isolation",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(1000.))),
                cx,
            );
            window.click(("setup-default-agent", 2usize), cx);
            assert_eq!(view.read(cx).draft.default_agent, "codex");
            window.click("setup-back", cx);
            assert_eq!(view.read(cx).draft.step, 1);
            window.click("setup-next", cx);
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, 3);
            for group in ["setup-theme", "setup-editor"] {
                let first = window.find((group, 0usize)).bounds();
                let second = window.find((group, 1usize)).bounds();
                assert_eq!(first.top(), second.top());
                assert!((first.size.width - second.size.width).abs() < px(1.));
                assert!(first.right() <= second.left());
            }
            window.click(("setup-theme", 1usize), cx);
            assert_eq!(view.read(cx).draft.theme, "light");
            assert!(!cx.theme().is_dark());
            window.press("left", cx);
            assert_eq!(view.read(cx).draft.theme, "dark");
            assert!(cx.theme().is_dark());
            window.press("right", cx);
            assert_eq!(view.read(cx).draft.theme, "light");
            assert_eq!(SetupDraft::load(&view.read(cx).root).theme, "light");
            window.click(("setup-editor", 1usize), cx);
            assert_eq!(view.read(cx).draft.editor, "neovim");
            window.press("left", cx);
            assert_eq!(view.read(cx).draft.editor, "built_in");
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_REPOSITORY);
            assert!(window.find(("setup-initial-repository", 0usize)).visible());
            assert!(window.find_all("setup-isolation").is_empty());
            window.click("setup-next", cx);
            assert_eq!(view.read(cx).draft.step, STEP_REVIEW);
            assert!(window.find("setup-next").visible());
            assert_eq!(SetupDraft::load(&view.read(cx).root).default_agent, "codex");
        });
        let handle = test_cx.update(|window, _| window.window_handle());
        test_cx.simulate_window_resize(handle, gpui_kit::size(px(760.), px(620.)));
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            let next = window.find("setup-next").bounds();
            assert!(next.bottom() <= window.viewport_size().height);
            assert!(next.right() <= window.viewport_size().width);
        });
        let completed = Rc::new(std::cell::Cell::new(false));
        let result = completed.clone();
        test_cx.update(|_, cx| {
            cx.subscribe(&view, move |_, _: &SetupComplete, _| result.set(true))
                .detach();
        });
        test_cx.update(|window, cx| window.click("setup-next", cx));
        test_cx.run_until_parked();
        assert!(completed.get());
        test_cx.update(|_, cx| {
            let state = DeviceStore::new(&view.read(cx).root).load().unwrap();
            assert_eq!(state.onboarding_complete, Some(true));
            assert!(state.onboarding_draft.is_none());
            assert_eq!(state.sync_interval_minutes, None);
        });
    }
}
