//! The Add Repository dialog: pick a local git checkout with the native
//! directory picker, review the harvested metadata, and save a portable
//! record plus device binding.
//!
//! The view is created fresh for every dialog opening (unlike Settings,
//! whose edits survive reopenings): a half-filled add form going stale
//! behind the dialog would be worse than re-harvesting. `Browse` refreshes
//! detected defaults while preserving explicit edits. Clearing a field and
//! re-browsing re-harvests that field.
//!
//! Why a dialog plus the OS picker instead of the sheet file-browser
//! example from gpui-kit docs: the dialog layer is already rendered by
//! [`Workspace`](crate::workspace::Workspace) while the sheet layer is not,
//! the docs' "file browser" is a layout example (a custom list, not a
//! picker component), and the native picker (`prompt_for_paths`) matches
//! the Electron `showOpenDialog` behavior including permissions and
//! accessibility.

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::InputState;
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::{WindowExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement, IntoElement,
    KeyDownEvent, ParentElement, PathPromptOptions, Render, Styled, Window, div, rgb,
};

use crate::repository_form::{RepositoryDraft, RepositoryForm};

use crate::command_palette::{
    PaletteMode, ToggleActionsPalette, ToggleProjectsPalette, palette_mode_for_shortcut,
};
use crate::data::{
    CheckoutInspection, DataRoot, NewRepositoryInput, RepositoryMetadata, create_repository,
    inspect_checkout, link_repository, update_repository_metadata,
};

/// What the dialog does on submit. Create writes a portable record plus a
/// device binding; Link binds an existing portable record to a checkout
/// (recovery for synced-but-unlinked records); Edit overwrites portable
/// metadata in place. Key renames remain deferred, so Link/Edit carry a
/// fixed key shown as static text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RepositoryDialogMode {
    Create,
    Link { key: String },
    Edit { key: String },
}

pub(crate) struct AddRepositoryView {
    workspace: Option<gpui_kit::WeakEntity<crate::workspace::Workspace>>,
    original_space: Option<String>,
    focus_handle: FocusHandle,
    mode: RepositoryDialogMode,
    data_root: Option<DataRoot>,
    checkout_path: Option<PathBuf>,
    form: RepositoryForm,
    detected: Option<RepositoryDraft>,
    /// Isolation profile for Create/Edit, chosen from the catalog; new
    /// repositories default to the active space.
    space: String,
    spaces: Vec<String>,
    revision: String,
    busy: bool,
    error: Option<String>,
}

pub(crate) struct RepositoryAdded(pub String);
impl gpui_kit::EventEmitter<RepositoryAdded> for AddRepositoryView {}

impl AddRepositoryView {
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        data_root: Option<DataRoot>,
        spaces: Vec<String>,
        default_space: String,
    ) -> Self {
        let form = RepositoryForm::new(&RepositoryDraft::default(), window, cx);
        Self {
            workspace: None,
            original_space: None,
            focus_handle: cx.focus_handle(),
            mode: RepositoryDialogMode::Create,
            data_root,
            checkout_path: None,
            form,
            detected: None,
            space: default_space,
            spaces,
            revision: String::new(),
            busy: false,
            error: None,
        }
    }

    /// Link mode: bind an existing portable record to a checkout. Only the
    /// checkout browser matters; the key is fixed and rendered as static
    /// text.
    pub(crate) fn for_link(
        window: &mut Window,
        cx: &mut Context<Self>,
        data_root: Option<DataRoot>,
        key: String,
        spaces: Vec<String>,
        default_space: String,
    ) -> Self {
        let mut view = Self::new(window, cx, data_root, spaces, default_space);
        view.mode = RepositoryDialogMode::Link { key };
        view
    }

    /// Edit mode: overwrite portable metadata in place. Fields prefill from
    /// the stored record, so a cleared field means "clear it"; the key is
    /// fixed (renames deferred) and rendered as static text.
    pub(crate) fn for_edit(
        window: &mut Window,
        cx: &mut Context<Self>,
        data_root: Option<DataRoot>,
        key: String,
        metadata: &RepositoryMetadata,
        spaces: Vec<String>,
        default_space: String,
    ) -> Self {
        let mut view = Self::new(window, cx, data_root, spaces, default_space);
        view.revision = metadata.revision.clone();
        let mut prefill = |state: &Entity<InputState>, value: &str| {
            if !value.is_empty() {
                state.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        };
        prefill(
            &view.form.display_name,
            metadata.display_name.as_deref().unwrap_or(""),
        );
        prefill(&view.form.owner, metadata.owner.as_deref().unwrap_or(""));
        prefill(&view.form.name, metadata.name.as_deref().unwrap_or(""));
        prefill(
            &view.form.description,
            metadata.description.as_deref().unwrap_or(""),
        );
        view.space = metadata
            .space
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(view.space);
        prefill(
            &view.form.tags,
            &metadata.tags.clone().unwrap_or_default().join(", "),
        );
        prefill(
            &view.form.clone_url,
            metadata.clone_url.as_deref().unwrap_or(""),
        );
        prefill(
            &view.form.base_branch,
            metadata.base_branch.as_deref().unwrap_or(""),
        );
        view.mode = RepositoryDialogMode::Edit { key };
        view.original_space = Some(view.space.clone());
        view
    }

    pub(crate) fn set_workspace(
        &mut self,
        workspace: gpui_kit::WeakEntity<crate::workspace::Workspace>,
    ) {
        self.workspace = Some(workspace);
    }

    fn values(&self, cx: &App) -> (String, NewRepositoryInput) {
        let draft = self.form.snapshot(cx);
        (draft.key.trim().to_owned(), draft.input(&self.space))
    }

    /// Open the native directory picker, then harvest the checkout on a
    /// background thread and fill empty fields with the mouse still free —
    /// the future is polled on the main thread but git runs off it.
    fn browse(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.error = None;
        cx.notify();
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Select a local Git checkout".into()),
        });
        cx.spawn_in(window, async move |view, cx| {
            let selected: Option<PathBuf> = match receiver.await {
                Ok(Ok(Some(mut paths))) => paths.pop(),
                Ok(Ok(None)) => return, // Cancelled: leave the form untouched.
                Ok(Err(error)) => {
                    let _ = cx.update(|_, cx| {
                        view.update(cx, |this, cx| {
                            this.error = Some(format!("could not open the file picker: {error:#}"));
                            cx.notify();
                        })
                        .ok();
                    });
                    return;
                }
                Err(_) => return, // Picker went away with the window.
            };
            let Some(path) = selected else {
                return;
            };
            let inspected = cx
                .background_spawn(async move {
                    let inspection = inspect_checkout(&path);
                    (path, inspection)
                })
                .await;
            let _ = cx.update(|window, cx| {
                view.update(cx, |this, cx| {
                    this.apply_inspection(inspected, window, cx);
                })
                .ok();
            });
        })
        .detach();
    }

    /// Record the picked directory and fill empty fields from the harvest.
    /// A failed harvest keeps the path visible and surfaces why, so the user
    /// sees which directory was rejected.
    fn apply_inspection(
        &mut self,
        inspected: (PathBuf, anyhow::Result<CheckoutInspection>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let window = &mut *window;
        let (path, inspection) = inspected;
        self.checkout_path = Some(path);
        match inspection {
            Ok(inspection) => {
                self.error = None;
                self.detected = Some(self.form.apply_inspection(
                    &inspection,
                    self.detected.as_ref(),
                    window,
                    cx,
                ));
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
            }
        }
        cx.notify();
    }

    /// Validate, then persist off the main thread. Create writes the record
    /// plus binding, Link binds an existing record, Edit overwrites metadata.
    /// The dialog stays open with the error on failure; success notifies
    /// and closes.
    fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        if let RepositoryDialogMode::Edit { key } = &self.mode
            && self
                .original_space
                .as_ref()
                .is_some_and(|original| !crate::data::space_eq(original, &self.space))
            && let Some(workspace) = self.workspace.clone()
        {
            let result = workspace
                .update(cx, |workspace, cx| {
                    workspace.ensure_repository_agent_idle(key, cx)
                })
                .and_then(|result| result);
            if let Err(error) = result {
                self.error = Some(format!("{error:#}"));
                cx.notify();
                return;
            }
        }
        let Some(root) = self.data_root.clone() else {
            self.error = Some("portable data is unavailable".to_owned());
            cx.notify();
            return;
        };
        let mode = self.mode.clone();
        // Edit needs no checkout; Create/Link require one.
        let checkout = if mode == RepositoryDialogMode::Create
            || matches!(mode, RepositoryDialogMode::Link { .. })
        {
            match self.checkout_path.clone() {
                Some(checkout) => Some(checkout),
                None => {
                    self.error = Some("select a checkout directory first".to_owned());
                    cx.notify();
                    return;
                }
            }
        } else {
            None
        };
        let (key_text, input) = self.values(cx);
        // Resolve the fixed key for Link/Edit; validate the typed key for
        // Create.
        let key = match &mode {
            RepositoryDialogMode::Create => {
                if key_text.is_empty() {
                    self.error = Some("enter a repository key".to_owned());
                    cx.notify();
                    return;
                }
                key_text
            }
            RepositoryDialogMode::Link { key } | RepositoryDialogMode::Edit { key } => key.clone(),
        };
        let verb = match &mode {
            RepositoryDialogMode::Create => "added",
            RepositoryDialogMode::Link { .. } => "linked",
            RepositoryDialogMode::Edit { .. } => "saved",
        }
        .to_owned();
        let revision = self.revision.clone();
        self.busy = true;
        self.error = None;
        cx.notify();
        cx.spawn_in(window, async move |view, cx| {
            let outcome = cx
                .background_spawn(async move {
                    match mode {
                        RepositoryDialogMode::Create => {
                            let checkout = checkout.expect("create requires a checkout");
                            create_repository(&root, &key, &checkout, &input)
                                .map(|created| created.key)
                        }
                        RepositoryDialogMode::Link { .. } => {
                            let checkout = checkout.expect("link requires a checkout");
                            link_repository(&root, &key, &checkout).map(|linked| linked.key)
                        }
                        RepositoryDialogMode::Edit { .. } => {
                            update_repository_metadata(&root, &key, &input, &revision)
                                .map(|_| key.clone())
                        }
                    }
                })
                .await;
            let _ = cx.update(|window, cx| match outcome {
                Ok(key) => {
                    let _ = view.update(cx, |_, cx| cx.emit(RepositoryAdded(key.clone())));
                    window.push_notification(format!("Repository \"{key}\" {verb}"), cx);
                    window.close_dialog(cx);
                }
                Err(error) => {
                    view.update(cx, |this, cx| {
                        this.busy = false;
                        this.error = Some(format!("{error:#}"));
                        cx.notify();
                    })
                    .ok();
                }
            });
        })
        .detach();
    }

    /// Mirror Settings/terminal panes: the palette toggles keep working
    /// while the dialog has focus; tab jumps and session creation live in
    /// navigation mode and the palettes now. Everything else bubbles
    /// normally.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(mode) = palette_mode_for_shortcut(
            &event.keystroke.key,
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
            event.keystroke.modifiers.alt,
        ) {
            match mode {
                PaletteMode::Actions => {
                    window.dispatch_action(Box::new(ToggleActionsPalette), cx);
                }
                PaletteMode::Projects => {
                    window.dispatch_action(Box::new(ToggleProjectsPalette), cx);
                }
            }
            window.prevent_default();
            cx.stop_propagation();
        }
    }

    fn field(&self, label: &str, state: &Entity<InputState>) -> impl IntoElement {
        RepositoryForm::field(label, state)
    }

    /// Space picker for the metadata form: the catalog's spaces, plus the
    /// record's own value when the catalog does not know it. New repositories
    /// default to the active space; Edit prefills the stored one.
    fn space_field(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.space.clone();
        let mut choices = self.spaces.clone();
        if !choices
            .iter()
            .any(|name| crate::data::space_eq(name, &current))
        {
            choices.push(current.clone());
        }
        let view = cx.entity().downgrade();
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(rgb(0x858989)).child("Space"))
            .child(
                Button::new("add-repository-space")
                    .w_full()
                    .label(current.clone())
                    .dropdown_caret(true)
                    .dropdown_menu(move |mut menu, _, _| {
                        for name in &choices {
                            let name = name.clone();
                            let selected = crate::data::space_eq(&current, &name);
                            let view = view.clone();
                            menu = menu.item(
                                PopupMenuItem::new(name.clone()).checked(selected).on_click(
                                    move |_, _, cx| {
                                        let name = name.clone();
                                        let _ = view.update(cx, |this, cx| {
                                            this.space = name;
                                            cx.notify();
                                        });
                                    },
                                ),
                            );
                        }
                        menu
                    }),
            )
    }

    fn render_checkout(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let path_text = self
            .checkout_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "No checkout selected".to_owned());
        let picked = self.checkout_path.is_some();
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x858989))
                    .child("Checkout directory (required)"),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px_3()
                            .py_2()
                            .rounded_md()
                            .border_1()
                            .border_color(rgb(0x292b2b))
                            .bg(rgb(0x0e0f0f))
                            .text_sm()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_color(if picked { rgb(0xe7e7e7) } else { rgb(0x555a5a) })
                            .child(path_text),
                    )
                    .child(
                        Button::new("add-repository-browse")
                            .label("Browse")
                            .on_click(move |_, window, cx| {
                                view.update(cx, |this, cx| this.browse(window, cx)).ok();
                            }),
                    ),
            )
    }

    fn render_fixed_key(&self, key: &str) -> impl IntoElement {
        v_flex()
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .text_color(rgb(0x858989))
                    .child("Repository key"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0xe7e7e7))
                    .child(key.to_owned()),
            )
    }

    fn render_metadata_fields(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .gap_3()
            .w_full()
            .child(self.field("Display name", &self.form.display_name))
            .child(
                h_flex()
                    .gap_3()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.field("Owner", &self.form.owner)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.field("Name", &self.form.name)),
                    ),
            )
            .child(self.field("Description", &self.form.description))
            .child(
                h_flex()
                    .gap_3()
                    .child(div().flex_1().min_w_0().child(self.space_field(cx)))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.field("Tags (comma-separated)", &self.form.tags)),
                    ),
            )
            .child(self.field("Clone URL", &self.form.clone_url))
            .child(self.field("Base branch", &self.form.base_branch))
    }

    fn render_footer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let confirm = view.clone();
        let (busy_label, idle_label) = match self.mode {
            RepositoryDialogMode::Create => ("Adding…", "Add repository"),
            RepositoryDialogMode::Link { .. } => ("Linking…", "Link checkout"),
            RepositoryDialogMode::Edit { .. } => ("Saving…", "Save changes"),
        };
        h_flex()
            .gap_2()
            .justify_end()
            .pt_4()
            .child(
                Button::new("add-repository-cancel")
                    .label("Cancel")
                    .ghost()
                    .on_click(|_, window, cx| {
                        window.close_dialog(cx);
                    }),
            )
            .child(
                Button::new("add-repository-submit")
                    .label(if self.busy { busy_label } else { idle_label })
                    .primary()
                    .on_click(move |_, window, cx| {
                        confirm.update(cx, |this, cx| this.submit(window, cx)).ok();
                    }),
            )
    }
}

impl Focusable for AddRepositoryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for AddRepositoryView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let hint = match &self.mode {
            RepositoryDialogMode::Create => {
                "Browse fills empty fields from the checkout — your edits are never overwritten."
            }
            RepositoryDialogMode::Link { .. } => {
                "Pick the local checkout to bind. Portable metadata stays untouched."
            }
            RepositoryDialogMode::Edit { .. } => {
                "Clearing a field removes it from the portable record. The key cannot be renamed."
            }
        };
        let mut body = v_flex().gap_3().w_full();
        match &self.mode {
            RepositoryDialogMode::Create => {
                body = body
                    .child(self.render_checkout(cx))
                    .child(self.field("Repository key (required, lowercase)", &self.form.key))
                    .child(self.render_metadata_fields(cx));
            }
            RepositoryDialogMode::Link { key } => {
                body = body
                    .child(self.render_fixed_key(key))
                    .child(self.render_checkout(cx));
            }
            RepositoryDialogMode::Edit { key } => {
                body = body
                    .child(self.render_fixed_key(key))
                    .child(self.render_metadata_fields(cx));
            }
        }
        body = body.when_some(self.error.clone(), |this, error| {
            this.child(div().text_sm().text_color(rgb(0xf87171)).child(error))
        });
        v_flex()
            .size_full()
            .px_6()
            .py_5()
            .gap_3()
            .bg(rgb(0x090a0a))
            .text_color(rgb(0xe7e7e7))
            .child(div().text_xs().text_color(rgb(0x737878)).child(hint))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scrollbar()
                    .child(body),
            )
            .child(self.render_footer(cx))
            .on_key_down(cx.listener(Self::on_key_down))
    }
}
