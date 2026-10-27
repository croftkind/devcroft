//! Shared settings surface, embedded in Settings and the editor status dialog.
use super::{
    catalog::{Preference, ServerId},
    install::{self, Action},
};
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::{ActiveTheme as _, Disableable as _};
use gpui_kit::component::{
    Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    dialog::{Confirm, DialogFooter},
    h_flex,
    input::{Input, InputState},
    switch::Switch,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement, Render, Styled, Window, div, px,
};
use gpui_kit::{InteractiveElement as _, StatefulInteractiveElement as _};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Duration,
};

struct Installation {
    managed: Option<install::ManagedInstallation>,
    executable: Option<PathBuf>,
    custom: bool,
    missing_runtime: Option<&'static str>,
}

impl Installation {
    fn detect(data: Option<&Path>, server: ServerId, pref: &Preference) -> Self {
        let managed =
            data.and_then(|root| install::managed_installation(root, server).ok().flatten());
        let executable = pref
            .executable
            .clone()
            .or_else(|| managed.as_ref().map(|managed| managed.executable.clone()))
            .or_else(|| crate::editor::find_executable(server.executable()));
        let required_runtime = match server {
            ServerId::Rust => None,
            ServerId::Go => Some(("go", "Go")),
            ServerId::TypeScript => Some(("node", "Node.js")),
            ServerId::Python
                if (pref.executable.is_none() && managed.is_some()) || pref.runtime.is_some() =>
            {
                Some(("node", "Node.js"))
            }
            ServerId::Python => None,
        };
        let missing_runtime = required_runtime.and_then(|(command, label)| {
            let runtime = pref
                .runtime
                .clone()
                .or_else(|| crate::editor::find_executable(command));
            (!runtime.as_ref().is_some_and(|path| path.is_file())).then_some(label)
        });
        Self {
            managed,
            executable,
            custom: pref.executable.is_some(),
            missing_runtime,
        }
    }

    fn available(&self) -> bool {
        self.installed() && self.missing_runtime.is_none()
    }

    fn installed(&self) -> bool {
        self.executable.as_ref().is_some_and(|path| path.is_file())
    }

    fn description(&self) -> String {
        let source: String = if self.custom {
            if self.installed() {
                "Custom installation"
            } else {
                "Custom executable not found"
            }
            .into()
        } else if let Some(managed) = &self.managed {
            if self.installed() {
                format!("Installed by Devcroft · v{}", managed.version)
            } else {
                "Installation incomplete".into()
            }
        } else if self.installed() {
            "System installation".into()
        } else {
            "Not installed".into()
        };
        if self.installed()
            && let Some(runtime) = self.missing_runtime
        {
            format!("{source} · {runtime} required")
        } else {
            source
        }
    }
}

struct Row {
    executable: Entity<InputState>,
    runtime: Entity<InputState>,
    root: Entity<InputState>,
    settings: Entity<InputState>,
    error: Option<String>,
}
pub(crate) struct LanguageServers {
    data: Option<PathBuf>,
    checkout: Option<PathBuf>,
    rows: HashMap<ServerId, Row>,
    notice: Option<String>,
}
impl LanguageServers {
    pub(crate) fn new(
        data: Option<PathBuf>,
        checkout: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let rows = ServerId::ALL
            .into_iter()
            .map(|s| {
                let pref = install::preferences(data.as_deref(), s);
                let mut input = |value: String, placeholder: &str| {
                    cx.new(|cx| {
                        let mut state = InputState::new(window, cx).placeholder(placeholder);
                        state.set_value(value, window, cx);
                        state
                    })
                };
                let executable = input(
                    pref.executable
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    "Automatic discovery, or absolute executable path",
                );
                let runtime = input(
                    pref.runtime
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    "Automatic runtime, or absolute Node / Go executable",
                );
                let root = input(
                    checkout
                        .as_ref()
                        .and_then(|p| pref.roots.get(p))
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    "Automatic project root, or an absolute directory in this checkout",
                );
                let settings = input(
                    if pref.settings.is_null() {
                        String::new()
                    } else {
                        pref.settings.to_string()
                    },
                    "Optional server settings as a JSON object",
                );
                (
                    s,
                    Row {
                        executable,
                        runtime,
                        root,
                        settings,
                        error: None,
                    },
                )
            })
            .collect();
        cx.spawn(async move |view, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if view.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        })
        .detach();
        Self {
            data,
            checkout,
            rows,
            notice: None,
        }
    }
    fn save(&mut self, server: ServerId, cx: &mut Context<Self>) -> bool {
        let result = (|| -> anyhow::Result<()> {
            let data = self
                .data
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("Device data is unavailable"))?;
            let row = &self.rows[&server];
            let path = |input: &Entity<InputState>| -> anyhow::Result<Option<PathBuf>> {
                let value = input.read(cx).value().trim().to_owned();
                if value.is_empty() {
                    return Ok(None);
                }
                let path = PathBuf::from(value);
                anyhow::ensure!(path.is_absolute(), "Use an absolute path");
                Ok(Some(path))
            };
            let mut pref = install::preferences(Some(data), server);
            pref.executable = path(&row.executable)?;
            pref.runtime = path(&row.runtime)?;
            if let Some(checkout) = &self.checkout {
                if let Some(root) = path(&row.root)? {
                    let root = root.canonicalize()?;
                    anyhow::ensure!(
                        root.is_dir() && root.starts_with(checkout),
                        "Root must be a directory inside this checkout"
                    );
                    pref.roots.insert(checkout.clone(), root);
                } else {
                    pref.roots.remove(checkout);
                }
            }
            let json = row.settings.read(cx).value();
            pref.settings = if json.trim().is_empty() {
                serde_json::Value::Null
            } else {
                let value: serde_json::Value = serde_json::from_str(&json)?;
                anyhow::ensure!(value.is_object(), "Settings must be a JSON object");
                value
            };
            install::save_preference(data, server, pref)
        })();
        let saved = match result {
            Ok(()) => {
                self.rows.get_mut(&server).unwrap().error = None;
                self.notice =
                    Some("Saved. Open workspaces apply changes on their next check.".into());
                true
            }
            Err(error) => {
                self.rows.get_mut(&server).unwrap().error = Some(format!("{error:#}"));
                false
            }
        };
        cx.notify();
        saved
    }

    fn open_configuration(
        &mut self,
        server: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Each opening starts from persisted values, discarding any canceled draft.
        self.notice = None;
        let pref = install::preferences(self.data.as_deref(), server);
        let row = self.rows.get_mut(&server).unwrap();
        row.error = None;
        let mut reset = |input: &Entity<InputState>, value: String| {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        };
        reset(
            &row.executable,
            pref.executable
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        );
        reset(
            &row.runtime,
            pref.runtime
                .as_ref()
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        );
        reset(
            &row.root,
            self.checkout
                .as_ref()
                .and_then(|checkout| pref.roots.get(checkout))
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
        );
        reset(
            &row.settings,
            if pref.settings.is_null() {
                String::new()
            } else {
                pref.settings.to_string()
            },
        );
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let content = view.update(cx, |this, cx| this.render_configuration(server, cx));
            let save_view = view.clone();
            let unavailable = view.read(cx).data.is_none();
            dialog
                .title(format!("{} advanced settings", server.label()))
                .w(px(600.))
                .child(content)
                .footer(
                    DialogFooter::new()
                        .child(
                            Button::new("lsp-configuration-cancel")
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("lsp-configuration-save")
                                .primary()
                                .label("Save")
                                .disabled(unavailable)
                                .on_click(|_, window, cx| {
                                    window.dispatch_action(
                                        Box::new(Confirm { secondary: false }),
                                        cx,
                                    );
                                }),
                        ),
                )
                .on_ok(move |_, _, cx| save_view.update(cx, |this, cx| this.save(server, cx)))
        });
    }

    fn render_configuration(
        &self,
        server: ServerId,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row = &self.rows[&server];
        let pref = install::preferences(self.data.as_deref(), server);
        let installation = Installation::detect(self.data.as_deref(), server, &pref);
        let job = self
            .data
            .as_deref()
            .and_then(|root| install::job(root, server));
        let busy = job.as_ref().is_some_and(|job| job.busy);

        v_flex()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Optional overrides for {}. Leave fields empty to use automatic defaults.",
                        server.id()
                    )),
            )
            .when_some(installation.executable.clone(), |fields, path| {
                fields.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Executable: {}", path.display())),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("Server executable"))
                    .child(Input::new(&row.executable)),
            )
            .when(server != ServerId::Rust, |fields| fields.child(
                v_flex().gap_1()
                    .child(div().text_xs().child(if server == ServerId::Go { "Go runtime" } else { "Node.js runtime" }))
                    .child(Input::new(&row.runtime))))
            .when(self.checkout.is_some(), |fields| {
                fields.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_xs().child("Project root"))
                        .child(Input::new(&row.root)),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().child("Server settings (JSON)"))
                    .child(Input::new(&row.settings)),
            )
            .when_some(row.error.clone(), |fields, error| {
                fields.child(div().text_sm().text_color(cx.theme().danger).child(error))
            })
            .when_some(installation.managed.as_ref(), |fields, managed| {
                fields.child(v_flex().gap_2().pt_3().border_t_1().border_color(cx.theme().border)
                    .child(div().text_sm().font_semibold().child("Devcroft installation"))
                    .child(div().text_xs().text_color(cx.theme().muted_foreground)
                        .child(format!("Version {}. Uninstall removes only the copy installed by Devcroft.", managed.version)))
                    .child(h_flex().gap_2().flex_wrap()
                        .child(Button::new(format!("lsp-update-{}",server.id())).small().label("Check for updates")
                            .disabled(busy || self.data.is_none()).on_click(cx.listener(move |this,_,_,cx| this.action(server,Action::Install,cx))))
                        .when_some(managed.previous_version.clone(), |actions, version| actions.child(
                            Button::new(format!("lsp-rollback-{}",server.id())).small().label(format!("Restore v{version}"))
                                .disabled(busy || self.data.is_none()).on_click(cx.listener(move |this,_,_,cx| this.action(server,Action::Rollback,cx)))))
                        .child(Button::new(format!("lsp-remove-{}",server.id())).small().ghost().label("Uninstall")
                            .disabled(busy || self.data.is_none()).on_click(cx.listener(move |this,_,_,cx| this.action(server,Action::Remove,cx))))))
            })
            .when_some(job.as_ref(), |fields, job| fields.child(div().text_xs().child(job.status.clone())))
            .when(busy && job.as_ref().is_some_and(|job| job.cancellable), |fields| fields.child(
                Button::new(format!("lsp-dialog-cancel-{}", server.id())).small().label("Cancel operation")
                    .on_click(cx.listener(move |this,_,_,cx| this.cancel_operation(server,cx)))))
            .when_some(
                job.as_ref()
                    .map(|job| super::process::log_text(&job.log))
                    .filter(|log| !log.is_empty()),
                |fields, log| {
                    fields.child(
                        v_flex()
                            .gap_1()
                            .child(div().text_xs().child("Operation log"))
                            .child(
                                div()
                                    .id(format!("lsp-log-{}", server.id()))
                                    .max_h(px(140.))
                                    .overflow_y_scroll()
                                    .text_xs()
                                    .child(log),
                            ),
                    )
                },
            )
    }
    fn action(&mut self, server: ServerId, action: Action, cx: &mut Context<Self>) {
        if let Some(data) = &self.data {
            install::begin(data.clone(), server, action);
        }
        cx.notify();
    }
    fn cancel_operation(&self, server: ServerId, cx: &mut Context<Self>) {
        if let Some(job) = self
            .data
            .as_deref()
            .and_then(|root| install::job(root, server))
        {
            job.cancel.store(true, Ordering::SeqCst);
        }
        cx.notify();
    }
    fn set_enabled(&mut self, server: ServerId, enabled: bool, cx: &mut Context<Self>) {
        if let Some(data) = &self.data {
            let mut pref = install::preferences(Some(data), server);
            if enabled && !Installation::detect(Some(data), server, &pref).available() {
                return;
            }
            pref.disabled = !enabled;
            if let Err(e) = install::save_preference(data, server, pref) {
                self.notice = Some(format!("{e:#}"));
            }
        }
        cx.notify();
    }

    fn render_server(&self, server: ServerId, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let pref = install::preferences(self.data.as_deref(), server);
        let installation = Installation::detect(self.data.as_deref(), server, &pref);
        let available = installation.available();
        let installed = installation.installed();
        let job = self
            .data
            .as_deref()
            .and_then(|root| install::job(root, server));
        let busy = job.as_ref().is_some_and(|job| job.busy);
        let source = installation.description();
        let view = cx.entity().downgrade();

        v_flex()
            .gap_3()
            .p_3()
            .min_w_0()
            .rounded_lg()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .justify_between()
                    .child(
                        v_flex()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if available && pref.disabled {
                                        cx.theme().muted_foreground
                                    } else {
                                        cx.theme().foreground
                                    })
                                    .child(server.label()),
                            )
                            .child(
                                h_flex()
                                    .gap_2()
                                    .flex_wrap()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(div().child(server.id()))
                                    .child(div().child("·"))
                                    .child(div().child(source)),
                            ),
                    )
                    .when(available, |header| {
                        header.child(
                            Switch::new(format!("lsp-enable-{}", server.id()))
                                .flex_none()
                                .checked(!pref.disabled)
                                .accessibility_label(format!("{} language support", server.label()))
                                .tooltip(format!(
                                    "{} {} language support",
                                    if pref.disabled { "Enable" } else { "Disable" },
                                    server.label()
                                ))
                                .disabled(busy || self.data.is_none())
                                .on_change(move |enabled, _, cx| {
                                    let _ = view.update(cx, |this, cx| {
                                        this.set_enabled(server, *enabled, cx);
                                    });
                                }),
                        )
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .flex_wrap()
                    .when(!installed && !installation.custom, |actions| {
                        actions.child(
                            Button::new(format!("lsp-install-{}", server.id()))
                                .small()
                                .label(if installation.managed.is_some() {
                                    "Reinstall"
                                } else {
                                    "Install"
                                })
                                .disabled(busy || self.data.is_none())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.action(server, Action::Install, cx)
                                })),
                        )
                    })
                    .child(
                        Button::new(format!("lsp-existing-{}", server.id()))
                            .small()
                            .ghost()
                            .label(if !installed && installation.custom {
                                "Fix executable path".into()
                            } else if installed && let Some(runtime) = installation.missing_runtime
                            {
                                format!("Set up {runtime}")
                            } else {
                                "Advanced settings".into()
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_configuration(server, window, cx);
                            })),
                    )
                    .when(
                        busy && job.as_ref().is_some_and(|job| job.cancellable),
                        |actions| {
                            actions.child(
                                Button::new(format!("lsp-cancel-{}", server.id()))
                                    .small()
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cancel_operation(server, cx)
                                    })),
                            )
                        },
                    ),
            )
            .when_some(job.as_ref(), |card, job| {
                card.child(div().text_xs().child(job.status.clone()))
            })
    }
}
impl Render for LanguageServers {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("language-server-settings")
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_sm().font_semibold().child("Language servers"))
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Install a server to add language support, then switch it on or off. Advanced settings includes installation management and custom paths."),
                    ),
            )
            .when_some(self.notice.clone(), |panel, notice| {
                panel.child(div().text_xs().child(notice))
            })
            .children(
                ServerId::ALL
                    .into_iter()
                    .map(|server| self.render_server(server, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::test::TestWindowExt as _;
    use std::{cell::RefCell, rc::Rc};

    struct TestHost;
    impl Render for TestHost {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    #[test]
    fn installed_server_requires_its_runtime_before_it_can_be_enabled() {
        let data = tempfile::tempdir().unwrap();
        let executable = data.path().join("server");
        let runtime = data.path().join("node");
        std::fs::write(&executable, "fixture").unwrap();
        let pref = Preference {
            executable: Some(executable),
            runtime: Some(runtime.clone()),
            ..Preference::default()
        };
        let missing_runtime = Installation::detect(None, ServerId::TypeScript, &pref);
        assert!(missing_runtime.installed());
        assert!(!missing_runtime.available());
        assert_eq!(missing_runtime.missing_runtime, Some("Node.js"));
        assert!(missing_runtime.description().contains("Node.js required"));
        std::fs::write(&runtime, "fixture").unwrap();
        assert!(Installation::detect(None, ServerId::TypeScript, &pref).available());
    }

    #[gpui_kit::test]
    fn missing_custom_server_cannot_be_enabled_until_its_file_exists(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let data = tempfile::tempdir().unwrap();
        let executable = data.path().join("custom-server");
        install::save_preference(
            data.path(),
            ServerId::Rust,
            Preference {
                disabled: true,
                executable: Some(executable.clone()),
                ..Preference::default()
            },
        )
        .unwrap();
        let data_path = data.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let panel = cx.new(|cx| LanguageServers::new(Some(data_path), None, window, cx));
            *slot.borrow_mut() = Some(panel.clone());
            gpui_kit::component::Root::new(panel, window, cx)
        });
        let panel = holder.borrow().clone().unwrap();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("lsp-enable-rust-analyzer").is_none());
            assert!(window.try_find("lsp-install-rust-analyzer").is_none());
            panel.update(cx, |panel, cx| panel.set_enabled(ServerId::Rust, true, cx));
            assert!(install::preferences(Some(data.path()), ServerId::Rust).disabled);

            std::fs::write(&executable, "fixture").unwrap();
            window.render_frame(cx);
            assert!(window.find("lsp-enable-rust-analyzer").visible());
            assert!(window.try_find("lsp-install-rust-analyzer").is_none());
            window.click("lsp-enable-rust-analyzer", cx);
            assert!(!install::preferences(Some(data.path()), ServerId::Rust).disabled);

            std::fs::remove_file(&executable).unwrap();
            window.render_frame(cx);
            assert!(window.try_find("lsp-enable-rust-analyzer").is_none());
        });
    }

    #[gpui_kit::test]
    fn advanced_dialog_validates_discards_drafts_and_preserves_parent(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            cx.set_reduce_motion(true);
        });
        let data = tempfile::tempdir().unwrap();
        let data_path = data.path().to_owned();
        let holder = Rc::new(RefCell::new(None));
        let slot = holder.clone();
        let (_, test_cx) = cx.add_window_view(move |window, cx| {
            let panel = cx.new(|cx| LanguageServers::new(Some(data_path), None, window, cx));
            *slot.borrow_mut() = Some(panel);
            let host = cx.new(|_| TestHost);
            gpui_kit::component::Root::new(host, window, cx)
        });
        let panel = holder.borrow().clone().unwrap();

        test_cx.update(|window, cx| {
            let parent_panel = panel.clone();
            window.open_dialog(cx, move |dialog, _, _| {
                dialog.title("Settings").child(parent_panel.clone())
            });
            panel.update(cx, |panel, cx| {
                panel.open_configuration(ServerId::Rust, window, cx)
            });
            window.render_frame(cx);
            assert!(window.find("lsp-configuration-save").visible());

            let executable = panel.read(cx).rows[&ServerId::Rust].executable.clone();
            executable.update(cx, |input, cx| {
                input.set_value("relative-server-path", window, cx)
            });
            window.click("lsp-configuration-save", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.find("lsp-configuration-save").visible());
            assert_eq!(
                panel.read(cx).rows[&ServerId::Rust].error.as_deref(),
                Some("Use an absolute path")
            );
            assert!(
                install::preferences(Some(data.path()), ServerId::Rust)
                    .executable
                    .is_none()
            );

            window.click("lsp-configuration-cancel", cx);
            assert!(window.try_find("lsp-configuration-save").is_none());
            assert!(window.find("lsp-existing-rust-analyzer").visible());
            panel.update(cx, |panel, cx| {
                panel.open_configuration(ServerId::Rust, window, cx)
            });
            window.render_frame(cx);
            let executable = panel.read(cx).rows[&ServerId::Rust].executable.clone();
            assert!(executable.read(cx).value().is_empty());
            assert!(panel.read(cx).rows[&ServerId::Rust].error.is_none());

            let custom_path = data.path().join("custom-server");
            executable.update(cx, |input, cx| {
                input.set_value(custom_path.display().to_string(), window, cx)
            });
            let settings = panel.read(cx).rows[&ServerId::Rust].settings.clone();
            settings.update(cx, |input, cx| input.set_value("[]", window, cx));
            window.click("lsp-configuration-save", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert_eq!(
                panel.read(cx).rows[&ServerId::Rust].error.as_deref(),
                Some("Settings must be a JSON object")
            );
            assert!(
                install::preferences(Some(data.path()), ServerId::Rust)
                    .executable
                    .is_none()
            );

            let settings = panel.read(cx).rows[&ServerId::Rust].settings.clone();
            settings.update(cx, |input, cx| input.set_value("{}", window, cx));
            window.click("lsp-configuration-save", cx);
        });
        test_cx.run_until_parked();
        test_cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find("lsp-configuration-save").is_none());
            assert!(window.find("lsp-existing-rust-analyzer").visible());
            assert_eq!(
                install::preferences(Some(data.path()), ServerId::Rust).executable,
                Some(data.path().join("custom-server"))
            );
        });
    }
}
