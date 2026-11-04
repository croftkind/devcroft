//! Durable machine-local ownership. Provider catalogs are disposable; this file
//! is not. Every mutation reloads under an OS lock and replaces one JSON file.
use super::{DataRoot, spaces::Space};
use crate::{
    agent::AgentKind,
    agent_accounts::Account,
    agent_sessions::{SessionKey, SessionSummary},
    relative_time::current_unix_secs,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::PathBuf,
};

fn id(prefix: &str) -> String {
    super::record::random_id().replacen("art-", prefix, 1)
}
pub(crate) fn normalized(value: &str) -> String {
    value.trim().to_lowercase()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Thread {
    pub id: String,
    pub space: String,
    pub agent: AgentKind,
    pub cwd: PathBuf,
    pub managed: bool,
    pub account: Account,
    /// Capture even shared stores at creation; changing the environment later
    /// must not redirect an existing conversation's resume.
    pub store: PathBuf,
    pub native: Option<SessionKey>,
    pub title: String,
    pub title_override: Option<String>,
    pub tags: Vec<String>,
    pub created: i64,
    pub updated: i64,
    pub archived: bool,
    pub launch_error: Option<String>,
    /// Persist the pre-launch snapshot. Manual linking excludes old history,
    /// and each identity can be owned by only one thread.
    pub baseline: Vec<SessionKey>,
}
impl Thread {
    pub fn label(&self) -> &str {
        self.title_override.as_deref().unwrap_or(&self.title)
    }
    pub fn context(&self) -> crate::agent_accounts::AgentLaunchContext {
        crate::agent_accounts::AgentLaunchContext {
            cwd: self.cwd.clone(),
            agent: self.agent,
            account: self.account.clone(),
            store: Some(self.store.clone()),
            session: self.native.as_ref().map(|key| SessionSummary {
                key: key.clone(),
                cwd: self.cwd.clone(),
                checkout: self.cwd.clone(),
                title: self.label().into(),
                updated: self.updated,
                timestamp_source: "chat".into(),
            }),
        }
    }
    pub fn available_space<'a>(&self, spaces: &'a super::Spaces) -> Option<&'a Space> {
        spaces.spaces.iter().find(|s| s.id == self.space)
    }
    pub fn account_matches(&self, root: &DataRoot, spaces: &super::Spaces) -> bool {
        self.available_space(spaces)
            .and_then(|s| Account::for_space(root, s).ok())
            .is_some_and(|a| a == self.account)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Tag {
    pub id: String,
    pub space: String,
    pub name: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct ViewState {
    pub recent: bool,
    pub collapsed: HashSet<String>,
    pub search: String,
    pub tags: HashSet<String>,
    pub archived: bool,
    pub harness: Option<AgentKind>,
    pub selected: Option<String>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Registry {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub views: BTreeMap<String, ViewState>,
    pub last_creation_space: Option<String>,
}
impl Registry {
    pub fn path(root: &DataRoot) -> PathBuf {
        root.root().join("local/chat.json")
    }
    pub fn load(root: &DataRoot) -> Result<Self> {
        let path = Self::path(root);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e).context("Reading Chat ownership"),
        };
        ensure!(
            bytes.len() <= 32 * 1024 * 1024,
            "Chat registry exceeds size limit"
        );
        let registry: Self = serde_json::from_slice(&bytes).context("Reading Chat ownership")?;
        registry.validate()?;
        Ok(registry)
    }
    pub(super) fn validate(&self) -> Result<()> {
        let mut ids = HashSet::new();
        let mut keys = HashSet::new();
        for t in &self.threads {
            ensure!(
                t.id.starts_with("chat-")
                    && t.id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
                    && ids.insert(&t.id),
                "Invalid or duplicate Chat identity"
            );
            ensure!(
                !t.space.is_empty() && t.cwd.is_absolute() && t.store.is_absolute(),
                "Invalid Chat context"
            );
            if let Some(key) = &t.native {
                ensure!(
                    key.provider == t.agent.id() && key.store == t.store && keys.insert(key),
                    "Conflicting Chat native identity"
                );
            }
            let mut assignments = HashSet::new();
            for tag in &t.tags {
                ensure!(
                    assignments.insert(tag)
                        && self.tags.iter().any(|x| &x.id == tag && x.space == t.space),
                    "Invalid Chat tag assignment"
                );
            }
        }
        let mut tag_ids = HashSet::new();
        let mut names = HashSet::new();
        for tag in &self.tags {
            ensure!(
                tag_ids.insert(&tag.id)
                    && !normalized(&tag.name).is_empty()
                    && normalized(&tag.name) != "general"
                    && names.insert((&tag.space, normalized(&tag.name))),
                "Invalid or duplicate Chat tag"
            );
        }
        Ok(())
    }
    pub(super) fn lock(root: &DataRoot) -> Result<fs::File> {
        let path = Self::path(root);
        let parent = path.parent().unwrap();
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(parent.join("chat.lock"))?;
        lock.lock()?;
        Ok(lock)
    }
    pub fn update<T>(root: &DataRoot, change: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        let _lock = Self::lock(root)?;
        let path = Self::path(root);
        let mut registry = Self::load(root)?;
        let result = change(&mut registry)?;
        registry.validate()?;
        super::write_json_atomic(&path, &registry)?;
        Ok(result)
    }
    pub fn create(
        &mut self,
        root: &DataRoot,
        space: &Space,
        agent: AgentKind,
        folder: Option<PathBuf>,
        baseline: Vec<SessionKey>,
    ) -> Result<String> {
        ensure!(
            !space.id.is_empty(),
            "Space identity migration is incomplete"
        );
        let account = Account::for_space(root, space)?;
        account.prepare(agent)?;
        let thread_id = id("chat-");
        let managed = folder.is_none();
        let cwd = folder.unwrap_or_else(|| root.root().join("session-workspaces").join(&thread_id));
        if managed {
            fs::create_dir_all(&cwd)?;
        }
        ensure!(cwd.is_dir(), "Working folder is unavailable");
        let cwd = cwd.canonicalize()?;
        let store = account.session_store(agent)?;
        let now = current_unix_secs();
        self.threads.push(Thread {
            id: thread_id.clone(),
            space: space.id.clone(),
            agent,
            cwd,
            managed,
            account,
            store,
            native: None,
            title: "New chat".into(),
            title_override: None,
            tags: Vec::new(),
            created: now,
            updated: now,
            archived: false,
            launch_error: None,
            baseline,
        });
        self.last_creation_space = Some(space.id.clone());
        Ok(thread_id)
    }
    pub fn thread_mut(&mut self, id: &str) -> Result<&mut Thread> {
        self.threads
            .iter_mut()
            .find(|t| t.id == id)
            .context("Chat was removed; reload")
    }
    pub fn tag(&mut self, space: &str, name: &str) -> Result<String> {
        let name = name.trim();
        let key = normalized(name);
        ensure!(
            !key.is_empty()
                && key != "general"
                && name.chars().count() <= 48
                && !name.chars().any(char::is_control),
            "Use a tag of 1–48 characters; General is reserved"
        );
        if let Some(tag) = self
            .tags
            .iter()
            .find(|t| t.space == space && normalized(&t.name) == key)
        {
            return Ok(tag.id.clone());
        }
        let tag_id = id("tag-");
        self.tags.push(Tag {
            id: tag_id.clone(),
            space: space.into(),
            name: name.into(),
        });
        Ok(tag_id)
    }
    pub fn rename_tag(&mut self, id: &str, name: &str) -> Result<()> {
        let tag = self
            .tags
            .iter()
            .find(|t| t.id == id)
            .context("Tag was removed")?;
        let key = normalized(name);
        ensure!(
            !key.is_empty()
                && key != "general"
                && name.trim().chars().count() <= 48
                && !name.chars().any(char::is_control),
            "Invalid tag name"
        );
        ensure!(
            !self
                .tags
                .iter()
                .any(|t| t.id != id && t.space == tag.space && normalized(&t.name) == key),
            "Tag already exists"
        );
        self.tags.iter_mut().find(|t| t.id == id).unwrap().name = name.trim().into();
        Ok(())
    }
    pub fn delete_tag(&mut self, id: &str) {
        self.tags.retain(|t| t.id != id);
        for t in &mut self.threads {
            t.tags.retain(|x| x != id);
        }
    }
    pub fn labels(&self, thread: &Thread) -> Vec<String> {
        if thread.tags.is_empty() {
            return vec!["General".into()];
        }
        self.tags
            .iter()
            .filter(|t| thread.tags.contains(&t.id))
            .map(|t| t.name.clone())
            .collect()
    }
    pub fn results<'a>(&'a self, space: Option<&str>, view: &ViewState) -> Vec<&'a Thread> {
        let search = normalized(&view.search);
        let mut rows: Vec<_> = self
            .threads
            .iter()
            .filter(|t| {
                let labels = self.labels(t);
                space.is_none_or(|s| s == t.space)
                    && (view.archived || !t.archived)
                    && view.harness.is_none_or(|a| a == t.agent)
                    && (view.tags.is_empty()
                        || labels.iter().any(|l| view.tags.contains(&normalized(l))))
                    && (normalized(t.label()).contains(&search)
                        || labels.iter().any(|l| normalized(l).contains(&search)))
            })
            .collect();
        rows.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        rows
    }
    pub fn groups<'a>(
        &'a self,
        rows: &[&'a Thread],
        view: &ViewState,
    ) -> Vec<(String, Vec<&'a Thread>)> {
        let mut groups: BTreeMap<String, Vec<_>> = BTreeMap::new();
        for t in rows {
            for label in self.labels(t) {
                let key = normalized(&label);
                if view.tags.is_empty() || view.tags.contains(&key) {
                    groups.entry(key).or_default().push(*t);
                }
            }
        }
        let general = groups.remove("general");
        let mut result: Vec<_> = groups.into_iter().collect();
        if let Some(rows) = general {
            result.push(("general".into(), rows));
        }
        result
    }
    pub fn candidates<'a>(
        &self,
        id: &str,
        sessions: &'a [SessionSummary],
    ) -> Vec<&'a SessionSummary> {
        let Some(t) = self.threads.iter().find(|t| t.id == id) else {
            return Vec::new();
        };
        sessions
            .iter()
            .filter(|s| {
                s.key.provider == t.agent.id()
                    && s.key.store == t.store
                    && s.cwd == t.cwd
                    && !t.baseline.contains(&s.key)
                    && !self
                        .threads
                        .iter()
                        .any(|other| other.native.as_ref() == Some(&s.key))
            })
            .collect()
    }
    pub fn bind(&mut self, id: &str, session: &SessionSummary) -> Result<()> {
        ensure!(
            self.candidates(id, std::slice::from_ref(session)).len() == 1,
            "History does not match this chat's provider, store and folder, or is already owned"
        );
        let t = self.thread_mut(id)?;
        t.native = Some(session.key.clone());
        t.title = session.title.clone();
        t.updated = t.updated.max(session.updated);
        Ok(())
    }
    pub fn move_space(&mut self, root: &DataRoot, source: &Space, target: &Space) -> Result<()> {
        let destination_account = Account::for_space(root, target)?;
        let threads: Vec<_> = self
            .threads
            .iter()
            .filter(|t| t.space == source.id)
            .map(|t| (t.id.clone(), self.labels(t)))
            .collect();
        ensure!(
            self.threads
                .iter()
                .filter(|t| t.space == source.id)
                .all(|t| t.account == destination_account
                    && destination_account
                        .session_store(t.agent)
                        .is_ok_and(|store| store == t.store)),
            "This space owns {} chats in a different account. Retain the space and its history; start new chats in the destination instead.",
            threads.len()
        );
        for (id, labels) in threads {
            let mut tags = Vec::new();
            for label in labels {
                if normalized(&label) != "general" {
                    tags.push(self.tag(&target.id, &label)?);
                }
            }
            let t = self.thread_mut(&id)?;
            t.space = target.id.clone();
            t.tags = tags;
        }
        self.tags.retain(|t| t.space != source.id);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, DataRoot, super::super::Spaces) {
        let dir = tempfile::tempdir().unwrap();
        let root = DataRoot::new(dir.path().into());
        let spaces = super::super::ensure_spaces(&root).unwrap();
        (dir, root, spaces)
    }
    #[test]
    fn durable_ownership_tags_filters_and_atomic_failure() {
        let (_dir, root, spaces) = setup();
        let space = &spaces.spaces[0];
        let id = Registry::update(&root, |r| {
            r.create(&root, space, AgentKind::Codex, None, vec![])
        })
        .unwrap();
        Registry::update(&root, |r| {
            let a = r.tag(&space.id, " Ideas ")?;
            assert_eq!(a, r.tag(&space.id, "ideas")?);
            let b = r.tag(&space.id, "design")?;
            r.thread_mut(&id)?.tags = vec![a, b];
            Ok(())
        })
        .unwrap();
        let r = Registry::load(&root).unwrap();
        let t = &r.threads[0];
        assert!(t.cwd.is_dir());
        assert!(!t.cwd.join(".git").exists());
        let mut v = ViewState::default();
        assert_eq!(r.groups(&r.results(None, &v), &v).len(), 2);
        assert_eq!(r.results(None, &v).len(), 1);
        v.tags.insert("ideas".into());
        v.tags.insert("unknown".into());
        assert_eq!(r.results(None, &v).len(), 1);
        assert_eq!(r.groups(&r.results(None, &v), &v).len(), 1);
        v.search = "absent".into();
        assert!(r.results(None, &v).is_empty());
        assert!(
            Registry::update(&root, |r| -> Result<()> {
                r.threads.clear();
                anyhow::bail!("interrupted")
            })
            .is_err()
        );
        assert_eq!(Registry::load(&root).unwrap().threads.len(), 1);
        Registry::update(&root, |r| {
            for tag in r.threads[0].tags.clone() {
                r.delete_tag(&tag);
            }
            Ok(())
        })
        .unwrap();
        let r = Registry::load(&root).unwrap();
        assert_eq!(r.labels(&r.threads[0]), vec!["General"]);
    }
    #[test]
    fn native_identity_never_guesses_or_crosses_accounts() {
        let (_dir, root, spaces) = setup();
        let mut r = Registry::default();
        let a = r
            .create(&root, &spaces.spaces[0], AgentKind::Codex, None, vec![])
            .unwrap();
        let b = r
            .create(
                &root,
                &spaces.spaces[1],
                AgentKind::Codex,
                Some(r.threads[0].cwd.clone()),
                vec![],
            )
            .unwrap();
        let mut session = r.threads[0]
            .context()
            .session
            .unwrap_or_else(|| SessionSummary {
                key: SessionKey {
                    provider: "codex".into(),
                    store: r.threads[0].store.clone(),
                    id: "native".into(),
                },
                cwd: r.threads[0].cwd.clone(),
                checkout: PathBuf::new(),
                title: "Native".into(),
                updated: 1,
                timestamp_source: "test".into(),
            });
        session.key.store = PathBuf::from("/different-account");
        assert!(r.bind(&a, &session).is_err());
        session.key.store = r.threads[0].store.clone();
        r.bind(&a, &session).unwrap();
        assert!(r.bind(&b, &session).is_err());
        let mut isolated = spaces.spaces[1].clone();
        isolated.isolate_agents = true;
        isolated.agent_account_id = Some("account-other".into());
        assert!(r.move_space(&root, &spaces.spaces[0], &isolated).is_err());
        let folder = r.threads[0].cwd.clone();
        r.thread_mut(&a).unwrap().archived = true;
        assert!(folder.is_dir());
    }
    #[test]
    fn stable_space_identity_survives_rename_and_conflicts_are_rejected() {
        let (_dir, root, spaces) = setup();
        let id = spaces.spaces[0].id.clone();
        super::super::rename_space(&root, "Personal", "Renamed").unwrap();
        assert_eq!(super::super::Spaces::load(&root).unwrap().spaces[0].id, id);
        let mut catalog = super::super::Spaces::load(&root).unwrap();
        catalog.spaces[1].id = id;
        super::super::write_json_atomic(&super::super::Spaces::path(&root), &catalog).unwrap();
        assert!(super::super::Spaces::load(&root).is_err());
    }
    #[test]
    fn moves_map_tags_atomically_and_reject_stale_shared_stores() {
        let (_dir, root, spaces) = setup();
        let mut r = Registry::default();
        let id = r
            .create(&root, &spaces.spaces[0], AgentKind::Codex, None, vec![])
            .unwrap();
        let a = r.tag(&spaces.spaces[0].id, "Ideas").unwrap();
        let b = r.tag(&spaces.spaces[1].id, "ideas").unwrap();
        r.thread_mut(&id).unwrap().tags = vec![a];
        r.move_space(&root, &spaces.spaces[0], &spaces.spaces[1])
            .unwrap();
        assert_eq!(r.thread_mut(&id).unwrap().tags, vec![b]);
        r.thread_mut(&id).unwrap().store = root.root().join("previous-shared-store");
        assert!(
            r.move_space(&root, &spaces.spaces[1], &spaces.spaces[0])
                .is_err()
        );
        assert_eq!(r.thread_mut(&id).unwrap().space, spaces.spaces[1].id);
    }
    #[test]
    fn concurrent_metadata_edits_preserve_all_records_and_corrupt_data_is_untouched() {
        let (_dir, root, spaces) = setup();
        let space = spaces.spaces[0].id.clone();
        std::thread::scope(|scope| {
            for i in 0..8 {
                let root = &root;
                let space = &space;
                scope.spawn(move || {
                    Registry::update(root, |r| r.tag(space, &format!("tag {i}"))).unwrap()
                });
            }
        });
        assert_eq!(Registry::load(&root).unwrap().tags.len(), 8);
        std::fs::write(Registry::path(&root), "broken ownership").unwrap();
        assert!(
            Registry::update(&root, |r| {
                r.threads.clear();
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(Registry::path(&root)).unwrap(),
            "broken ownership"
        );
    }
    #[test]
    fn referenced_shared_stores_remain_discoverable_and_space_id_changes_never_reassign() {
        let (_dir, root, spaces) = setup();
        Registry::update(&root, |r| {
            r.create(&root, &spaces.spaces[0], AgentKind::Codex, None, vec![])?;
            r.threads[0].store = root.root().join("old-store");
            Ok(())
        })
        .unwrap();
        let accounts = crate::agent_accounts::Accounts::load(Some(&root)).unwrap();
        assert!(
            accounts
                .chat_sources
                .contains(&(AgentKind::Codex, root.root().join("old-store")))
        );
        let registry = Registry::load(&root).unwrap();
        let mut changed = spaces.clone();
        changed.spaces[0].id = "space-upstream".into();
        assert!(registry.threads[0].available_space(&changed).is_none());
        assert!(!registry.threads[0].account_matches(&root, &changed));
    }
}
