//! Repository-owned agent accounts. Portable records contain only an opaque
//! identity; homes, credentials and transcripts are always machine-local.
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};

use crate::{
    agent::AgentKind,
    agent_sessions::SessionSummary,
    data::{DataRoot, DeviceStore, Spaces, all_repositories, space_eq},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Account {
    directory: Option<PathBuf>,
}

impl Account {
    pub(crate) fn isolated(root: &DataRoot, id: &str) -> Result<Self> {
        ensure!(
            id.starts_with("account-")
                && id.len() <= 80
                && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "Invalid agent account identity"
        );
        Ok(Self {
            directory: Some(canonical(root.root()).join("agent-accounts").join(id)),
        })
    }

    pub(crate) fn store(&self, agent: AgentKind) -> Option<PathBuf> {
        let directory = self.directory.as_ref()?.join(agent.id());
        Some(if agent == AgentKind::Opencode {
            directory.join("data")
        } else {
            directory
        })
    }

    pub(crate) fn environment(&self, agent: AgentKind) -> Vec<(String, String)> {
        let Some(store) = self.store(agent) else {
            return Vec::new();
        };
        let mut values = vec![(
            match agent {
                AgentKind::Claude => "CLAUDE_CONFIG_DIR",
                AgentKind::Codex => "CODEX_HOME",
                AgentKind::Opencode => "XDG_DATA_HOME",
                AgentKind::Omp => "PI_CODING_AGENT_DIR",
            }
            .to_owned(),
            store.to_string_lossy().into_owned(),
        )];
        if agent == AgentKind::Claude {
            // Claude can also authenticate through the ant CLI's active
            // profile; keep that fallback in the same isolated account.
            values.push((
                "ANTHROPIC_CONFIG_DIR".into(),
                store.join("anthropic").to_string_lossy().into_owned(),
            ));
        }
        if agent == AgentKind::Omp {
            values.push((
                "OMP_PROFILE".into(),
                format!(
                    "devcroft-{}",
                    self.directory
                        .as_ref()
                        .unwrap()
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                ),
            ));
        }
        if agent == AgentKind::Opencode {
            let base = self.directory.as_ref().unwrap().join(agent.id());
            for (key, folder) in [
                ("XDG_CONFIG_HOME", "config"),
                ("XDG_STATE_HOME", "state"),
                ("XDG_CACHE_HOME", "cache"),
            ] {
                values.push((key.into(), base.join(folder).to_string_lossy().into_owned()));
            }
        }
        values
    }

    pub(crate) fn prepare(&self, agent: AgentKind) -> Result<()> {
        for (key, path) in self.environment(agent) {
            if key == "OMP_PROFILE" {
                continue;
            }
            let mut builder = fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder
                .create(&path)
                .with_context(|| format!("Creating agent account directory {path}"))?;
        }
        Ok(())
    }

    /// Runs *inside a subshell, after shell startup*. Shell rc files cannot
    /// overwrite the directory selection or reintroduce shared auth tokens.
    /// The surrounding terminal retains its normal environment on exit.
    pub(crate) fn wrap_command(&self, agent: AgentKind, command: &str) -> String {
        if self.directory.is_none() {
            return command.to_owned();
        }
        let exports = self
            .environment(agent)
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "export {key}={} &&",
                    crate::agent_activity::shell_quote(&value)
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        format!(
            "(unset {} && {exports} {command})",
            Self::credential_overrides(agent).join(" ")
        )
    }

    fn credential_overrides(agent: AgentKind) -> &'static [&'static str] {
        const CLAUDE: &[&str] = &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN_FILE_DESCRIPTOR",
            "CLAUDE_CODE_API_KEY_FILE_DESCRIPTOR",
            "CLAUDE_CODE_USE_BEDROCK",
            "CLAUDE_CODE_USE_VERTEX",
            "CLAUDE_CODE_USE_FOUNDRY",
            "ANTHROPIC_PROFILE",
            "ANTHROPIC_FEDERATION_RULE_ID",
            "ANTHROPIC_ORGANIZATION_ID",
            "ANTHROPIC_IDENTITY_TOKEN",
            "ANTHROPIC_IDENTITY_TOKEN_FILE",
        ];
        match agent {
            AgentKind::Claude => CLAUDE,
            AgentKind::Codex => &[
                "OPENAI_API_KEY",
                "CODEX_API_KEY",
                "CODEX_ACCESS_TOKEN",
                "OPENAI_ORGANIZATION",
                "OPENAI_PROJECT",
                "OPENAI_BASE_URL",
            ],
            AgentKind::Opencode | AgentKind::Omp => &[
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "ANTHROPIC_OAUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
                "OPENAI_API_KEY",
                "CODEX_API_KEY",
                "CODEX_ACCESS_TOKEN",
                "GEMINI_API_KEY",
                "GOOGLE_API_KEY",
                "COPILOT_GITHUB_TOKEN",
                "OPENROUTER_API_KEY",
                "AZURE_OPENAI_API_KEY",
                "GROQ_API_KEY",
                "MISTRAL_API_KEY",
                "XAI_API_KEY",
                "OPENCODE_API_KEY",
                "OPENCODE_DB",
                "OPENCODE_CONFIG",
                "OPENCODE_CONFIG_CONTENT",
                "OPENCODE_CONFIG_DIR",
                "PI_CODING_AGENT_PROFILE",
                "OMP_AUTH_BROKER_URL",
                "OMP_AUTH_BROKER_TOKEN",
                "OMP_AUTH_BROKER_ACCOUNT_POOL_FILE",
            ],
        }
    }
}

/// One consistent account map per scan, avoiding filesystem work for every row.
#[derive(Default)]
pub(crate) struct Accounts {
    checkouts: HashMap<PathBuf, Account>,
    managed_directory: Option<PathBuf>,
    pub(crate) isolated: Vec<Account>,
}

impl Accounts {
    pub(crate) fn load(root: Option<&DataRoot>) -> Result<Self> {
        let Some(root) = root else {
            return Ok(Self::default());
        };
        let spaces = Spaces::load(root)?;
        let mut by_space = Vec::new();
        let mut isolated = Vec::new();
        for space in &spaces.spaces {
            let account = if space.isolate_agents {
                let account = Account::isolated(
                    root,
                    space
                        .agent_account_id
                        .as_deref()
                        .context("Isolated space is missing its agent account identity")?,
                )?;
                ensure!(
                    !isolated.contains(&account),
                    "Spaces share an agent account identity"
                );
                isolated.push(account.clone());
                account
            } else {
                Account::default()
            };
            by_space.push((&space.name, account));
        }
        let managed_directory = Some(canonical(&root.root().join("agent-accounts")));
        if isolated.is_empty() {
            // Existing shared-only installations keep tolerant repository
            // discovery. Old managed histories are still excluded below.
            return Ok(Self {
                managed_directory,
                ..Self::default()
            });
        }
        // Don't let tolerant repository listing downgrade a corrupt device
        // binding store to shared authentication while isolation is enabled.
        DeviceStore::new(root).load()?;
        let (repositories, errors) = all_repositories(root)?;
        ensure!(
            errors.is_empty(),
            "Cannot determine repository agent accounts: {}",
            errors.join("; ")
        );
        let mut checkouts = HashMap::new();
        for repo in repositories {
            let Some(path) = repo.checkout_path else {
                continue;
            };
            let account = by_space
                .iter()
                .find(|(name, _)| space_eq(name, &repo.space))
                .map(|(_, account)| account.clone())
                .with_context(|| format!("Repository {} belongs to an unknown space", repo.key))?;
            let previous = checkouts.insert(canonical(&path), account.clone());
            ensure!(
                previous.is_none_or(|previous| previous == account),
                "Checkout is assigned to conflicting agent accounts"
            );
        }
        Ok(Self {
            checkouts,
            managed_directory,
            isolated,
        })
    }

    pub(crate) fn for_checkout(&self, checkout: &Path) -> Account {
        let checkout = canonical(checkout);
        self.checkouts
            .iter()
            .filter(|(path, _)| checkout.starts_with(path))
            .max_by_key(|(path, _)| path.components().count())
            .map(|(_, account)| account.clone())
            .unwrap_or_default()
    }

    pub(crate) fn permits(&self, session: &SessionSummary) -> bool {
        let Some(agent) = AgentKind::parse(&session.key.provider) else {
            return false;
        };
        let account = self.for_checkout(&session.cwd);
        match account.store(agent) {
            Some(store) => canonical(&store) == canonical(&session.key.store),
            None => !self
                .managed_directory
                .as_ref()
                .is_some_and(|directory| canonical(&session.key.store).starts_with(directory)),
        }
    }
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| match (path.parent(), path.file_name()) {
            (Some(parent), Some(name)) if parent != path => canonical(parent).join(name),
            _ => path.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        agent_activity::AgentActivityStore,
        agent_sessions::{Catalog, SessionKey},
        data::{self, DeviceRepositoryBinding},
    };
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, DataRoot, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::new(dir.path().join("devcroft's data"));
        data::ensure_dirs(root.root()).unwrap();
        data::ensure_spaces(&root).unwrap();
        let personal = dir.path().join("personal");
        let work = dir.path().join("work");
        for (key, checkout, space) in [("personal", &personal, "Personal"), ("work", &work, "Work")]
        {
            fs::create_dir_all(checkout).unwrap();
            let repo_dir = root.portable_dir().join("repositories").join(key);
            fs::create_dir_all(&repo_dir).unwrap();
            data::write_json_atomic(
                &repo_dir.join("repository.json"),
                &json!({"key": key, "space": space}),
            )
            .unwrap();
            DeviceStore::new(&root)
                .update(|state| {
                    state.repositories.get_or_insert_default().insert(
                        key.into(),
                        DeviceRepositoryBinding {
                            checkout_path: Some(checkout.to_string_lossy().into_owned()),
                            ..Default::default()
                        },
                    );
                })
                .unwrap();
        }
        (dir, root, personal, work)
    }

    fn isolate(root: &DataRoot, space: &str, enabled: bool) {
        let mut catalog = Spaces::load(root).unwrap();
        let expected = catalog.clone();
        catalog.set_agent_isolation(space, enabled).unwrap();
        catalog.save(root, &expected).unwrap();
    }

    fn session(cwd: &Path, store: &Path) -> SessionSummary {
        SessionSummary {
            key: SessionKey {
                provider: "claude".into(),
                store: canonical(store),
                id: "test-session".into(),
            },
            checkout: canonical(cwd),
            cwd: canonical(cwd),
            title: "Test session".into(),
            updated: 1,
            timestamp_source: "test".into(),
        }
    }

    #[test]
    fn repository_ownership_selects_accounts_independently_of_browsing_space() {
        let (_dir, root, personal, work) = fixture();
        assert_eq!(
            Accounts::load(Some(&root)).unwrap().for_checkout(&work),
            Account::default()
        );
        isolate(&root, "Work", true);
        isolate(&root, "Personal", true);
        let accounts = Accounts::load(Some(&root)).unwrap();
        let work_account = accounts.for_checkout(&work);
        let personal_account = accounts.for_checkout(&personal);
        assert_ne!(work_account, personal_account);
        assert_eq!(accounts.for_checkout(&work.join("src")), work_account);
        DeviceStore::new(&root)
            .update(|state| {
                state.active_space = Some("Personal".into());
                state.all_spaces = Some(true);
            })
            .unwrap();
        assert_eq!(
            Accounts::load(Some(&root)).unwrap().for_checkout(&work),
            work_account
        );
        for agent in AgentKind::ALL {
            assert_ne!(work_account.store(agent), personal_account.store(agent));
            assert!(
                !work_account
                    .store(agent)
                    .unwrap()
                    .starts_with(root.portable_dir())
            );
        }
        let metadata = fs::read_to_string(Spaces::path(&root)).unwrap();
        assert!(!metadata.contains(root.root().to_str().unwrap()));
        assert!(!metadata.contains("credentials"));
    }

    #[test]
    fn rename_and_reenable_preserve_login_but_delete_does_not_merge_accounts() {
        let (_dir, root, personal, work) = fixture();
        isolate(&root, "Work", true);
        let original = Accounts::load(Some(&root)).unwrap().for_checkout(&work);
        original.prepare(AgentKind::Claude).unwrap();
        let marker = original
            .store(AgentKind::Claude)
            .unwrap()
            .join("login-marker");
        fs::write(&marker, "fixture").unwrap();
        data::rename_space(&root, "Work", "Company").unwrap();
        assert_eq!(
            Accounts::load(Some(&root)).unwrap().for_checkout(&work),
            original
        );
        isolate(&root, "Company", false);
        let disabled = Accounts::load(Some(&root)).unwrap();
        assert_eq!(disabled.for_checkout(&work), Account::default());
        assert!(!disabled.permits(&session(&work, marker.parent().unwrap())));
        isolate(&root, "Company", true);
        assert_eq!(
            Accounts::load(Some(&root)).unwrap().for_checkout(&work),
            original
        );
        data::delete_space(&root, "Company", "Personal").unwrap();
        let accounts = Accounts::load(Some(&root)).unwrap();
        assert_eq!(
            accounts.for_checkout(&work),
            accounts.for_checkout(&personal)
        );
        assert!(!accounts.permits(&session(&work, marker.parent().unwrap())));
        assert!(marker.is_file());
    }

    #[test]
    fn wrong_account_resume_and_stale_cache_fail_closed_after_repository_move() {
        let (_dir, root, personal, work) = fixture();
        isolate(&root, "Work", true);
        isolate(&root, "Personal", true);
        let accounts = Accounts::load(Some(&root)).unwrap();
        let work_store = accounts
            .for_checkout(&work)
            .store(AgentKind::Claude)
            .unwrap();
        let personal_store = accounts
            .for_checkout(&personal)
            .store(AgentKind::Claude)
            .unwrap();
        let correct = session(&work, &work_store);
        let wrong = session(&work, &personal_store);
        assert!(accounts.permits(&correct));
        assert!(!accounts.permits(&wrong));
        let (activity, _) = AgentActivityStore::with_data_root(Some(root.clone()));
        let launch = activity.start_with_session(&work, AgentKind::Claude, Some(&wrong));
        assert!(launch.launch_error.is_some());

        let index = root.root().join("cache/agent-sessions");
        fs::create_dir_all(&index).unwrap();
        data::write_json_atomic(
            &index.join("index.json"),
            &json!({"version":1,"sessions":[correct],"transcripts":{}}),
        )
        .unwrap();
        let catalog = Catalog::new(Some(&root));
        catalog.load_cache();
        assert_eq!(catalog.snapshot().sessions.len(), 1);
        data::write_json_atomic(
            &root
                .portable_dir()
                .join("repositories/work/repository.json"),
            &json!({"key":"work","space":"Personal"}),
        )
        .unwrap();
        assert!(catalog.snapshot().sessions.is_empty());
        assert!(!Accounts::load(Some(&root)).unwrap().permits(&correct));
    }

    #[test]
    fn isolated_claude_session_is_discovered_and_resumed_from_its_store() {
        let (_dir, root, _personal, work) = fixture();
        isolate(&root, "Work", true);
        let account = Accounts::load(Some(&root)).unwrap().for_checkout(&work);
        let store = account.store(AgentKind::Claude).unwrap();
        let project = store.join("projects/test");
        fs::create_dir_all(&project).unwrap();
        let transcript = json!({"type":"user","sessionId":"test-session","cwd":work,"timestamp":"2026-01-01T12:00:00.123Z","message":{"content":"Work account session"}});
        fs::write(
            project.join("test-session.jsonl"),
            format!("{transcript}\n"),
        )
        .unwrap();
        let catalog = Catalog::new(Some(&root));
        let resumed = catalog.prepare_open(&session(&work, &store).key).unwrap();
        assert_eq!(resumed.title, "Work account session");
        let (activity, _) = AgentActivityStore::with_data_root(Some(root.clone()));
        let launch = activity.start_with_session(&work, AgentKind::Claude, Some(&resumed));
        assert!(launch.launch_error.is_none());
        assert_eq!(launch.account, account);
        assert!(launch.command_line().contains("--resume"));
        isolate(&root, "Work", false);
        assert!(catalog.prepare_open(&resumed.key).is_err());
    }

    #[test]
    fn malformed_configuration_does_not_fall_back_to_shared_credentials() {
        let (_dir, root, _, work) = fixture();
        isolate(&root, "Work", true);
        let mut spaces = Spaces::load(&root).unwrap();
        let expected = spaces.clone();
        spaces
            .spaces
            .iter_mut()
            .find(|space| space.name == "Work")
            .unwrap()
            .agent_account_id = Some("../../escape".into());
        spaces.save(&root, &expected).unwrap();
        let (activity, _) = AgentActivityStore::with_data_root(Some(root.clone()));
        assert!(
            activity
                .start(&work, AgentKind::Claude)
                .launch_error
                .is_some()
        );
        fs::write(root.device_path(), "{invalid").unwrap();
        assert!(Accounts::load(Some(&root)).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn provider_overrides_cannot_select_the_shared_database_or_subscription() {
        use std::process::Command;
        let (_dir, root, personal, work) = fixture();
        isolate(&root, "Work", true);
        isolate(&root, "Personal", true);
        let accounts = Accounts::load(Some(&root)).unwrap();
        let account = accounts.for_checkout(&work);
        let other = accounts.for_checkout(&personal);
        let commands = [
            (
                AgentKind::Opencode,
                "printf '%s\\n' \"${OPENCODE_DB-unset}\" \"$XDG_DATA_HOME\"",
            ),
            (
                AgentKind::Omp,
                "printf '%s\\n' \"${ANTHROPIC_OAUTH_TOKEN-unset}\" \"$OMP_PROFILE\"",
            ),
            (
                AgentKind::Codex,
                "printf '%s\\n' \"${CODEX_ACCESS_TOKEN-unset}\" \"$CODEX_HOME\"",
            ),
        ];
        for (agent, command) in commands {
            account.prepare(agent).unwrap();
            let output = Command::new("/bin/sh")
                .arg("-c")
                .arg(account.wrap_command(agent, command))
                .env("OPENCODE_DB", "/shared/database")
                .env("ANTHROPIC_OAUTH_TOKEN", "shared-token")
                .env("OMP_PROFILE", "shared-profile")
                .env("CODEX_ACCESS_TOKEN", "shared-token")
                .output()
                .unwrap();
            assert!(output.status.success());
            let output = String::from_utf8(output.stdout).unwrap();
            assert!(output.starts_with("unset\n"), "{agent:?}: {output}");
            assert!(!output.contains("shared"));
            assert_ne!(account.environment(agent), other.environment(agent));
            if agent == AgentKind::Omp {
                assert!(output.contains("devcroft-account-"));
            } else {
                assert!(output.contains(account.store(agent).unwrap().to_str().unwrap()));
            }
        }
    }

    #[test]
    #[cfg(unix)]
    fn shell_startup_auth_overrides_are_removed_without_changing_parent_environment() {
        use std::{os::unix::fs::PermissionsExt, process::Command};
        let (_dir, root, _, work) = fixture();
        isolate(&root, "Work", true);
        let account = Accounts::load(Some(&root)).unwrap().for_checkout(&work);
        account.prepare(AgentKind::Claude).unwrap();
        let script = account.wrap_command(AgentKind::Claude,
            "printf '%s\\n' \"$CLAUDE_CONFIG_DIR\" \"${ANTHROPIC_API_KEY-unset}\" \"${CLAUDE_CODE_OAUTH_TOKEN-unset}\" \"$ANTHROPIC_CONFIG_DIR\"");
        let output = Command::new("/bin/sh")
            .arg("-c")
            .arg(format!(
                "{script}; printf '%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$ANTHROPIC_API_KEY\""
            ))
            .env("CLAUDE_CONFIG_DIR", "/shared")
            .env("ANTHROPIC_API_KEY", "shared-key")
            .env("CLAUDE_CODE_OAUTH_TOKEN", "shared-token")
            .output()
            .unwrap();
        assert!(output.status.success());
        let store = account.store(AgentKind::Claude).unwrap();
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "{}\nunset\nunset\n{}\n/shared\nshared-key\n",
                store.display(),
                store.join("anthropic").display()
            )
        );
        assert_eq!(
            fs::metadata(&store).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            Account::default().wrap_command(AgentKind::Claude, "claude"),
            "claude"
        );
    }
}
