//! Bounded conflict reads and guarded resolutions. Git owns the index and operation state.
use super::{
    model::OperationState,
    repository::{LoadError, detect_operation, run_git_args, stderr_message},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read as _, Write as _},
    path::{Component, Path, PathBuf},
    time::Duration,
};

pub(super) const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_INSPECT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct OperationDetails {
    pub kind: OperationState,
    pub head: String,
    pub target: String,
    pub step: String,
    pub current_label: String,
    pub incoming_label: String,
    pub explanation: String,
    // Includes todo and step files so a stale button cannot continue another step.
    identity: Vec<u8>,
}

pub(super) fn operation_details(root: &Path) -> Result<Option<OperationDetails>, String> {
    let Some(kind) = detect_operation(root) else {
        return Ok(None);
    };
    let repo = gix::discover(root).map_err(|e| e.to_string())?;
    let dir = repo.git_dir();
    let head = git(root, &["rev-parse", "HEAD"], 4096)?.trim().to_owned();
    let branch =
        git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"], 4096).unwrap_or_default();
    let files = [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "sequencer/head",
        "sequencer/todo",
        "rebase-merge/head-name",
        "rebase-merge/onto",
        "rebase-merge/stopped-sha",
        "rebase-merge/msgnum",
        "rebase-merge/end",
        "rebase-merge/git-rebase-todo",
        "rebase-apply/head-name",
        "rebase-apply/onto",
        "rebase-apply/original-commit",
        "rebase-apply/next",
        "rebase-apply/last",
    ];
    let mut identity = Vec::new();
    let mut metadata = std::collections::HashMap::new();
    for name in files {
        identity.extend_from_slice(name.as_bytes());
        identity.push(0);
        let bytes = match fs::File::open(dir.join(name)) {
            Ok(file) => {
                let mut bytes = Vec::new();
                file.take(256 * 1024 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                if bytes.len() > 256 * 1024 {
                    return Err("Git operation metadata is too large to inspect safely.".into());
                }
                bytes
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("Could not read Git operation state: {e}")),
        };
        identity.extend_from_slice(&bytes);
        identity.push(0);
        metadata.insert(name, bytes);
    }
    let read = |name: &str| {
        String::from_utf8_lossy(metadata.get(name).map(Vec::as_slice).unwrap_or_default())
            .trim()
            .to_owned()
    };
    let (target, step, current_label, incoming_label, explanation) = match kind {
        OperationState::Rebase => {
            let prefix = if dir.join("rebase-merge").exists() {
                "rebase-merge"
            } else {
                "rebase-apply"
            };
            let onto = read(&format!("{prefix}/onto"));
            let replay = if prefix == "rebase-merge" {
                read(&format!("{prefix}/stopped-sha"))
            } else {
                read(&format!("{prefix}/original-commit"))
            };
            let step = if prefix == "rebase-merge" {
                format!(
                    "Step {} of {}",
                    read("rebase-merge/msgnum"),
                    read("rebase-merge/end")
                )
            } else {
                format!(
                    "Step {} of {}",
                    read("rebase-apply/next"),
                    read("rebase-apply/last")
                )
            };
            (
                commit_label(root, &onto),
                step,
                format!("Current · onto {}", commit_label(root, &onto)),
                format!("Incoming · replaying {}", commit_label(root, &replay)),
                format!(
                    "Rebasing {}: Current (ours) is the new base plus replayed commits; Incoming (theirs) is the commit being replayed.",
                    read(&format!("{prefix}/head-name")).trim_start_matches("refs/heads/")
                ),
            )
        }
        _ => {
            let name = match kind {
                OperationState::Merge => "MERGE_HEAD",
                OperationState::CherryPick => "CHERRY_PICK_HEAD",
                OperationState::Revert => "REVERT_HEAD",
                _ => unreachable!(),
            };
            let target = read(name)
                .lines()
                .map(|id| commit_label(root, id))
                .collect::<Vec<_>>()
                .join(", ");
            let explanation = if kind == OperationState::Revert {
                "Incoming is the inverse change from the commit being reverted."
            } else {
                "Current (ours) is your checkout; Incoming (theirs) is the change being applied."
            };
            (
                target.clone(),
                "Resolve conflicts, then continue".into(),
                format!("Current · {} {}", branch.trim(), short(&head)),
                format!("Incoming · {target}"),
                explanation.into(),
            )
        }
    };
    Ok(Some(OperationDetails {
        kind,
        head,
        target,
        step,
        current_label,
        incoming_label,
        explanation,
        identity: Sha256::digest(&identity).to_vec(),
    }))
}
fn short(oid: &str) -> &str {
    &oid[..oid.len().min(10)]
}
fn valid_oid(oid: &str) -> bool {
    (oid.len() == 40 || oid.len() == 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}
fn commit_label(root: &Path, oid: &str) -> String {
    if !valid_oid(oid) {
        return "unknown commit".into();
    }
    git(root, &["show", "-s", "--format=%h %s", oid], 4096)
        .map(|s| s.trim().chars().take(180).collect())
        .unwrap_or_else(|_| short(oid).into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Continuation {
    Continue,
    Abort,
    Skip,
}
impl Continuation {
    pub fn label(self) -> &'static str {
        match self {
            Self::Continue => "Continue",
            Self::Abort => "Abort",
            Self::Skip => "Skip",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ContinuationSource {
    pub operation: OperationDetails,
    index: Vec<u8>,
    worktree: Vec<(PathBuf, Baseline)>,
}
pub(super) fn capture_continuation(
    root: &Path,
    expected: &OperationDetails,
) -> Result<ContinuationSource, String> {
    let snapshot = super::repository::load_snapshot(root).map_err(error)?;
    if snapshot.operation_details.as_ref() != Some(expected) {
        return Err("The Git operation changed. Refresh before continuing.".into());
    }
    let index = command(
        root,
        vec!["ls-files".into(), "--stage".into(), "-z".into()],
        None,
        8 * 1024 * 1024,
    )?;
    let mut paths = snapshot
        .conflicts
        .iter()
        .chain(&snapshot.staged)
        .chain(&snapshot.unstaged)
        .chain(&snapshot.untracked)
        .map(|c| c.path.clone())
        .collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    if paths.len() > 10_000 {
        return Err(
            "More than 10,000 changed paths must be reviewed. Continue this operation externally."
                .into(),
        );
    }
    let mut worktree = Vec::new();
    let mut total = 0u64;
    for path in paths {
        let state = baseline(root, &path)?;
        if let Baseline::File { len, .. } = &state {
            total += len;
            if total > MAX_INSPECT_BYTES {
                return Err("Changed working-tree content exceeds the 64 MiB continuation inspection limit. Continue this operation externally.".into());
            }
        }
        worktree.push((path, state));
    }
    Ok(ContinuationSource {
        operation: expected.clone(),
        index,
        worktree,
    })
}
pub(super) fn continue_operation(
    root: &Path,
    expected: &ContinuationSource,
    action: Continuation,
) -> Result<String, String> {
    if capture_continuation(root, &expected.operation)? != *expected {
        return Err("The index or working tree changed after this action opened. Refresh and review it again.".into());
    }
    if action == Continuation::Continue && !unmerged(root)?.is_empty() {
        return Err("Resolve every unmerged index entry before continuing.".into());
    }
    if action == Continuation::Skip && expected.operation.kind != OperationState::Rebase {
        return Err("Skip is only available during rebase.".into());
    }
    let command = match expected.operation.kind {
        OperationState::Merge => "merge",
        OperationState::Rebase => "rebase",
        OperationState::CherryPick => "cherry-pick",
        OperationState::Revert => "revert",
    };
    let flag = match action {
        Continuation::Continue => "--continue",
        Continuation::Abort => "--abort",
        Continuation::Skip => "--skip",
    };
    super::remote::run_command(root, vec![command.into(), flag.into()])?;
    Ok(format!("{} completed", action.label()))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Stage {
    pub mode: String,
    pub oid: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct IndexConflict {
    #[serde(with = "raw_path")]
    pub path: PathBuf,
    pub stages: [Option<Stage>; 3],
}

pub(super) fn unmerged(root: &Path) -> Result<Vec<IndexConflict>, String> {
    let out = command(
        root,
        vec!["ls-files".into(), "--unmerged".into(), "-z".into()],
        None,
        8 * 1024 * 1024,
    )?;
    parse_unmerged(&out)
}
fn parse_unmerged(bytes: &[u8]) -> Result<Vec<IndexConflict>, String> {
    let mut result: Vec<IndexConflict> = Vec::new();
    for record in bytes.split(|b| *b == 0).filter(|r| !r.is_empty()) {
        let tab = record
            .iter()
            .position(|b| *b == b'\t')
            .ok_or("Malformed unmerged index entry")?;
        let fields = record[..tab].split(|b| *b == b' ').collect::<Vec<_>>();
        if fields.len() != 3 {
            return Err("Malformed unmerged index entry".into());
        }
        let mode = std::str::from_utf8(fields[0])
            .map_err(|_| "Invalid index mode")?
            .to_owned();
        let oid = std::str::from_utf8(fields[1])
            .map_err(|_| "Invalid index object")?
            .to_owned();
        let stage = match fields[2] {
            b"1" => 0,
            b"2" => 1,
            b"3" => 2,
            _ => return Err("Invalid index stage".into()),
        };
        if !valid_oid(&oid) || !["100644", "100755", "120000", "160000"].contains(&mode.as_str()) {
            return Err("Invalid conflict index identity".into());
        }
        #[cfg(unix)]
        let path = {
            use std::os::unix::ffi::OsStringExt as _;
            PathBuf::from(OsString::from_vec(record[tab + 1..].to_vec()))
        };
        #[cfg(not(unix))]
        let path = PathBuf::from(
            std::str::from_utf8(&record[tab + 1..]).map_err(|_| "Invalid repository path")?,
        );
        validate_path(&path)?;
        if result.last().is_none_or(|c| c.path != path) {
            result.push(IndexConflict {
                path,
                stages: Default::default(),
            })
        }
        let item = result.last_mut().unwrap();
        if item.stages[stage].is_some() {
            return Err("Duplicate index stage".into());
        }
        item.stages[stage] = Some(Stage { mode, oid });
    }
    Ok(result)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) enum Baseline {
    Missing,
    File { hash: Vec<u8>, len: u64, mode: u32 },
    Symlink(#[serde(with = "raw_path")] PathBuf),
    Directory,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Source {
    pub index: IndexConflict,
    pub disk: Baseline,
    pub operation: Option<OperationDetails>,
}
#[derive(Clone, Debug)]
pub(super) struct ConflictFile {
    pub source: Source,
    pub text: Option<TextMerge>,
    pub reason: String,
}
#[derive(Clone, Debug)]
pub(super) struct TextMerge {
    pub base: String,
    pub current: String,
    pub incoming: String,
    pub result: String,
    pub generated: String,
}

pub(super) fn load(root: &Path, path: &Path) -> Result<ConflictFile, String> {
    let index = unmerged(root)?
        .into_iter()
        .find(|c| c.path == path)
        .ok_or("This path is no longer conflicted. Refresh Git status.")?;
    let disk = baseline(root, path)?;
    let operation = operation_details(root)?;
    let source = Source {
        index,
        disk,
        operation,
    };
    let structural=source.index.stages.iter().flatten().find_map(|s|match s.mode.as_str(){"120000"=>Some("Symbolic link conflict"),"160000"=>Some("Submodule conflict; choosing a side stages its commit without changing the submodule checkout"),_=>None});
    if let Some(reason) = structural {
        return Ok(ConflictFile {
            source,
            text: None,
            reason: reason.into(),
        });
    }
    if matches!(source.disk, Baseline::Symlink(_) | Baseline::Directory) {
        return Ok(ConflictFile{source,text:None,reason:"File/directory or working-tree type conflict; inspect it externally before resolving".into()});
    }
    if source.index.stages[1].is_none() || source.index.stages[2].is_none() {
        return Ok(ConflictFile{source,text:None,reason:"Modify/delete or rename/delete conflict; choose the surviving side or delete this path".into()});
    }
    let text = (|| {
        let base = stage_text(root, source.index.stages[0].as_ref())?;
        let current = stage_text(root, source.index.stages[1].as_ref())?;
        let incoming = stage_text(root, source.index.stages[2].as_ref())?;
        let generated = diff3(root, &base, &current, &incoming)?;
        let result = match &source.disk {
            Baseline::Missing => generated.clone(),
            Baseline::File { len, .. } if *len <= MAX_TEXT_BYTES as u64 => {
                text_bytes(fs::read(root.join(path)).map_err(|e| e.to_string())?)?
            }
            _ => return Err("Working-tree result is larger than 2 MiB".into()),
        };
        Ok(TextMerge {
            base,
            current,
            incoming,
            result,
            generated,
        })
    })();
    // Recheck after reads: never associate a buffer with a different disk/index baseline.
    if baseline(root, path)? != source.disk
        || unmerged(root)?.iter().find(|c| c.path == path) != Some(&source.index)
    {
        return Err("Conflict changed while loading. Select the file again.".into());
    }
    match text {
        Ok(text) => Ok(ConflictFile {
            source,
            text: Some(text),
            reason: String::new(),
        }),
        Err(reason) => Ok(ConflictFile {
            source,
            text: None,
            reason,
        }),
    }
}
fn stage_text(root: &Path, stage: Option<&Stage>) -> Result<String, String> {
    let Some(stage) = stage else {
        return Ok(String::new());
    };
    let size = git(root, &["cat-file", "-s", &stage.oid], 128)?
        .trim()
        .parse::<usize>()
        .map_err(|_| "Invalid blob size")?;
    if size > MAX_TEXT_BYTES {
        return Err(
            "Conflict input is larger than 2 MiB. Choose a whole file or open externally.".into(),
        );
    }
    text_bytes(command(
        root,
        vec!["cat-file".into(), "blob".into(), stage.oid.clone().into()],
        None,
        MAX_TEXT_BYTES,
    )?)
}
fn text_bytes(bytes: Vec<u8>) -> Result<String, String> {
    if bytes.contains(&0) {
        return Err("Binary conflict. Choose a whole file or open externally.".into());
    }
    String::from_utf8(bytes).map_err(|_| {
        "Conflict contains invalid UTF-8. Choose a whole file or open externally.".into()
    })
}
fn diff3(root: &Path, base: &str, current: &str, incoming: &str) -> Result<String, String> {
    let dir = tempfile::tempdir().map_err(|e| e.to_string())?;
    for (name, text) in [("base", base), ("current", current), ("incoming", incoming)] {
        fs::write(dir.path().join(name), text).map_err(|e| e.to_string())?
    }
    let mut args = vec![
        "merge-file".into(),
        "--diff3".into(),
        "--marker-size=7".into(),
        "-p".into(),
        "-L".into(),
        "Current".into(),
        "-L".into(),
        "Base".into(),
        "-L".into(),
        "Incoming".into(),
    ];
    for name in ["current", "base", "incoming"] {
        args.push(dir.path().join(name).into_os_string())
    }
    let output = run_git_args(
        root,
        args,
        None,
        Duration::from_secs(30),
        MAX_TEXT_BYTES,
        2048,
    )
    .map_err(error)?;
    if output.stdout_truncated {
        return Err("Generated merge result is larger than 2 MiB".into());
    }
    if output
        .status
        .code()
        .is_none_or(|code| !(0..=127).contains(&code))
    {
        return Err(stderr_message(&output));
    }
    text_bytes(output.stdout)
}

pub(super) fn validate_path(path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("Unsafe repository path".into());
    }
    Ok(())
}
fn checked_path(root: &Path, path: &Path) -> Result<PathBuf, String> {
    validate_path(path)?;
    let mut candidate = root.canonicalize().map_err(|e| e.to_string())?;
    let parts = path.components().collect::<Vec<_>>();
    for (i, part) in parts.iter().enumerate() {
        candidate.push(part.as_os_str());
        if i + 1 < parts.len() {
            match fs::symlink_metadata(&candidate) {
                Ok(m) if m.is_dir() && !m.file_type().is_symlink() => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => {
                    return Err(
                        "Conflict path crosses a symlink or non-directory; inspect it externally."
                            .into(),
                    );
                }
            }
        }
    }
    Ok(candidate)
}
pub(super) fn baseline(root: &Path, path: &Path) -> Result<Baseline, String> {
    let path = checked_path(root, path)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Baseline::Missing),
        Err(e) => return Err(e.to_string()),
    };
    if metadata.file_type().is_symlink() {
        return fs::read_link(path)
            .map(Baseline::Symlink)
            .map_err(|e| e.to_string());
    }
    if metadata.is_dir() {
        return Ok(Baseline::Directory);
    }
    if !metadata.is_file() {
        return Err("Special filesystem entry; resolve externally.".into());
    }
    if metadata.len() > MAX_INSPECT_BYTES {
        return Err("Conflict file is larger than 64 MiB; resolve it externally.".into());
    }
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0; 64 * 1024];
    let mut len = 0u64;
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        len += n as u64;
        if len > MAX_INSPECT_BYTES {
            return Err("Conflict file grew beyond the inspection limit.".into());
        }
        hash.update(&bytes[..n]);
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.permissions().mode()
    };
    #[cfg(not(unix))]
    let mode = u32::from(metadata.permissions().readonly());
    Ok(Baseline::File {
        hash: hash.finalize().to_vec(),
        len,
        mode,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Text { result: String, stage: bool },
    Current,
    Incoming,
    Delete,
}
impl Resolution {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Text { stage: true, .. } => "Mark Resolved",
            Self::Text { stage: false, .. } => "Save Result",
            Self::Current => "Keep Current",
            Self::Incoming => "Keep Incoming",
            Self::Delete => "Delete",
        }
    }
}
fn verify(root: &Path, source: &Source) -> Result<PathBuf, String> {
    if unmerged(root)?.iter().find(|c| c.path == source.index.path) != Some(&source.index)
        || operation_details(root)? != source.operation
    {
        return Err("Conflict index or Git operation changed. Reload and review it again.".into());
    }
    if baseline(root, &source.index.path)? != source.disk {
        return Err(
            "The file changed on disk. Your Result draft is still open; copy it before reloading."
                .into(),
        );
    }
    checked_path(root, &source.index.path)
}
pub(super) fn resolve(root: &Path, source: &Source, choice: &Resolution) -> Result<String, String> {
    let path = verify(root, source)?;
    match choice {
        Resolution::Text { result, stage } => {
            if result.len() > MAX_TEXT_BYTES || result.contains('\0') {
                return Err("Result must be valid text no larger than 2 MiB.".into());
            }
            if source
                .index
                .stages
                .iter()
                .flatten()
                .any(|s| s.mode != "100644" && s.mode != "100755")
            {
                return Err("This conflict needs a whole-file choice.".into());
            }
            let intended_mode = match &source.disk {
                Baseline::File { mode, .. } => *mode,
                _ => {
                    #[cfg(unix)]
                    {
                        if source.index.stages[1]
                            .as_ref()
                            .is_some_and(|s| s.mode == "100755")
                        {
                            0o100755
                        } else {
                            0o100644
                        }
                    }
                    #[cfg(not(unix))]
                    {
                        0
                    }
                }
            };
            let filemode = git(root, &["config", "--bool", "--get", "core.filemode"], 128)
                .unwrap_or_else(|_| {
                    if cfg!(unix) {
                        "true".into()
                    } else {
                        "false".into()
                    }
                });
            let intended_index_mode = if filemode.trim() == "false" {
                source.index.stages[1]
                    .as_ref()
                    .map(|s| s.mode.as_str())
                    .unwrap_or("100644")
            } else if intended_mode & 0o111 != 0 {
                "100755"
            } else {
                "100644"
            };
            match &source.disk {
                Baseline::File { .. } => {
                    let original = fs::read(&path).map_err(|e| e.to_string())?;
                    verify(root, source)?;
                    crate::editor::native::save_if_unchanged(&path, &original, result.as_bytes())
                        .map_err(|e| format!("{e:#}"))?;
                }
                Baseline::Missing => {
                    let parent = path.parent().ok_or("No parent directory")?;
                    if !parent.is_dir() {
                        return Err("Parent directory is missing; resolve the structural conflict externally.".into());
                    }
                    let mut temp =
                        tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
                    temp.write_all(result.as_bytes())
                        .map_err(|e| e.to_string())?;
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt as _;
                        let exec = source.index.stages[1]
                            .as_ref()
                            .is_some_and(|s| s.mode == "100755");
                        temp.as_file()
                            .set_permissions(fs::Permissions::from_mode(if exec {
                                0o755
                            } else {
                                0o644
                            }))
                            .map_err(|e| e.to_string())?;
                    }
                    temp.as_file().sync_all().map_err(|e| e.to_string())?;
                    verify(root, source)?;
                    temp.persist_noclobber(&path).map_err(|e| e.to_string())?;
                }
                _ => return Err("Result cannot replace a symlink or directory.".into()),
            }
            if *stage {
                let expected_oid = filtered_oid(root, &source.index.path, result.as_bytes())?;
                verify_index(root, source)?;
                let expected_disk = Baseline::File {
                    hash: Sha256::digest(result.as_bytes()).to_vec(),
                    len: result.len() as u64,
                    mode: intended_mode,
                };
                if baseline(root, &source.index.path)? != expected_disk {
                    return Err(
                        "Result changed on disk before staging. Your draft remains available."
                            .into(),
                    );
                }
                stage_path(root, &source.index.path)?;
                verify_staged(
                    root,
                    source,
                    &expected_oid,
                    Some(intended_index_mode),
                    &expected_disk,
                )?;
            }
        }
        Resolution::Current | Resolution::Incoming => {
            let side = if matches!(choice, Resolution::Current) {
                1
            } else {
                2
            };
            let entry = source.index.stages[side]
                .as_ref()
                .ok_or("This side deleted the path. Use Delete to accept that deletion.")?;
            let expected_disk = if entry.mode == "160000" {
                source.disk.clone()
            } else {
                expected_checkout(root, &source.index.path, entry)?
            };
            verify(root, source)?;
            if entry.mode == "160000" {
                command(
                    root,
                    vec![
                        "--literal-pathspecs".into(),
                        "update-index".into(),
                        "--add".into(),
                        "--cacheinfo".into(),
                        entry.mode.clone().into(),
                        entry.oid.clone().into(),
                        source.index.path.as_os_str().to_owned(),
                    ],
                    None,
                    2048,
                )?;
            } else {
                if matches!(source.disk, Baseline::Directory) {
                    return Err("A directory occupies this path. Move it externally before choosing a file.".into());
                }
                command(
                    root,
                    vec![
                        "--literal-pathspecs".into(),
                        "checkout-index".into(),
                        "--force".into(),
                        format!("--stage={}", side + 1).into(),
                        "--".into(),
                        source.index.path.as_os_str().to_owned(),
                    ],
                    None,
                    2048,
                )?;
                verify_index(root, source)?;
                if !checkout_matches(&baseline(root, &source.index.path)?, &expected_disk) {
                    return Err(
                        "The file changed while choosing a side. Refresh and review it again."
                            .into(),
                    );
                }
                // Stage the immutable chosen side, without round-tripping it through a clean filter.
                command(
                    root,
                    vec![
                        "--literal-pathspecs".into(),
                        "update-index".into(),
                        "--add".into(),
                        "--cacheinfo".into(),
                        entry.mode.clone().into(),
                        entry.oid.clone().into(),
                        source.index.path.as_os_str().to_owned(),
                    ],
                    None,
                    2048,
                )?;
            }
            verify_staged(root, source, &entry.oid, Some(&entry.mode), &expected_disk)?;
        }
        Resolution::Delete => {
            if matches!(source.disk, Baseline::Directory)
                || source
                    .index
                    .stages
                    .iter()
                    .flatten()
                    .any(|s| s.mode == "160000")
            {
                return Err("Directory and submodule deletion must be handled externally.".into());
            }
            command(
                root,
                vec![
                    "--literal-pathspecs".into(),
                    "rm".into(),
                    "-f".into(),
                    "--".into(),
                    source.index.path.as_os_str().to_owned(),
                ],
                None,
                2048,
            )?;
        }
    }
    Ok(format!(
        "{} · {}",
        choice.label(),
        source.index.path.display()
    ))
}
fn verify_index(root: &Path, source: &Source) -> Result<(), String> {
    if unmerged(root)?.iter().find(|c| c.path == source.index.path) != Some(&source.index)
        || operation_details(root)? != source.operation
    {
        return Err(
            "Conflict index or operation changed while saving. Your draft remains available."
                .into(),
        );
    }
    Ok(())
}
fn filtered_oid(root: &Path, path: &Path, bytes: &[u8]) -> Result<String, String> {
    let mut option = OsString::from("--path=");
    option.push(path);
    String::from_utf8(command(
        root,
        vec!["hash-object".into(), option, "--stdin".into()],
        Some(bytes.to_vec()),
        128,
    )?)
    .map(|s| s.trim().to_owned())
    .map_err(|_| "Invalid staged object identity".into())
}
fn expected_checkout(root: &Path, path: &Path, stage: &Stage) -> Result<Baseline, String> {
    let mut option = OsString::from("--path=");
    option.push(path);
    let bytes = command(
        root,
        if stage.mode == "120000" {
            vec!["cat-file".into(), "blob".into(), stage.oid.clone().into()]
        } else {
            vec![
                "cat-file".into(),
                "--filters".into(),
                option,
                stage.oid.clone().into(),
            ]
        },
        None,
        MAX_INSPECT_BYTES as usize,
    )?;
    if stage.mode == "120000" {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            return Ok(Baseline::Symlink(PathBuf::from(OsString::from_vec(bytes))));
        }
        #[cfg(not(unix))]
        {
            return Err(
                "Symlink side selection is unsupported on this platform. Resolve externally."
                    .into(),
            );
        }
    }
    Ok(Baseline::File {
        hash: Sha256::digest(&bytes).to_vec(),
        len: bytes.len() as u64,
        mode: 0,
    })
}
fn checkout_matches(actual: &Baseline, expected: &Baseline) -> bool {
    match (actual, expected) {
        (
            Baseline::File {
                hash: a,
                len: al,
                mode: am,
            },
            Baseline::File {
                hash: b,
                len: bl,
                mode: bm,
            },
        ) => a == b && al == bl && (*bm == 0 || am == bm),
        _ => actual == expected,
    }
}
fn verify_staged(
    root: &Path,
    source: &Source,
    oid: &str,
    mode: Option<&str>,
    disk: &Baseline,
) -> Result<(), String> {
    let entries = command(
        root,
        vec![
            "--literal-pathspecs".into(),
            "ls-files".into(),
            "--stage".into(),
            "-z".into(),
            "--".into(),
            source.index.path.as_os_str().to_owned(),
        ],
        None,
        16 * 1024,
    )?;
    let records = entries
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .collect::<Vec<_>>();
    let matches = records.len() == 1
        && records[0]
            .split(|b| *b == b'\t')
            .next()
            .is_some_and(|head| {
                let fields = head.split(|b| *b == b' ').collect::<Vec<_>>();
                fields.len() == 3
                    && fields[1] == oid.as_bytes()
                    && fields[2] == b"0"
                    && mode.is_none_or(|mode| fields[0] == mode.as_bytes())
            });
    let actual_disk = baseline(root, &source.index.path)?;
    #[cfg(unix)]
    let mode_matches = match (disk, &actual_disk, mode) {
        (Baseline::File { mode: 0, .. }, Baseline::File { mode: actual, .. }, Some(expected)) => {
            ((*actual & 0o111) != 0) == (expected == "100755")
        }
        _ => true,
    };
    #[cfg(not(unix))]
    let mode_matches = true;
    if !matches
        || !mode_matches
        || !checkout_matches(&actual_disk, disk)
        || operation_details(root)? != source.operation
    {
        return Err("The file or index changed while staging. Your Result draft is preserved; refresh and review the staged content.".into());
    }
    Ok(())
}
fn stage_path(root: &Path, path: &Path) -> Result<(), String> {
    command(
        root,
        vec![
            "--literal-pathspecs".into(),
            "add".into(),
            "--".into(),
            path.as_os_str().to_owned(),
        ],
        None,
        2048,
    )
    .map(|_| ())
}
fn error(e: LoadError) -> String {
    match e {
        LoadError::GitUnavailable => "Git is unavailable".into(),
        LoadError::NotRepository => "Not a Git repository".into(),
        LoadError::Failed(s) => s,
    }
}
fn command(
    root: &Path,
    args: Vec<OsString>,
    stdin: Option<Vec<u8>>,
    limit: usize,
) -> Result<Vec<u8>, String> {
    let output =
        run_git_args(root, args, stdin, Duration::from_secs(30), limit, 4096).map_err(error)?;
    if !output.status.success() {
        return Err(stderr_message(&output));
    }
    if output.stdout_truncated {
        return Err("Git output exceeded the inspection limit".into());
    }
    Ok(output.stdout)
}
fn git(root: &Path, args: &[&str], limit: usize) -> Result<String, String> {
    String::from_utf8(command(
        root,
        args.iter().map(OsString::from).collect(),
        None,
        limit,
    )?)
    .map_err(|_| "Invalid Git metadata".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ConflictBlock {
    pub start: usize,
    pub end: usize,
    pub current: String,
    pub incoming: String,
    pub base: Option<String>,
}
/// Only complete, consistently sized marker blocks get buttons. Marker-like text is still a warning.
pub(super) fn blocks(text: &str) -> Vec<ConflictBlock> {
    let mut out = Vec::new();
    let lines = text.split_inclusive('\n').collect::<Vec<_>>();
    let mut offsets = Vec::new();
    let mut offset = 0;
    for line in &lines {
        offsets.push(offset);
        offset += line.len()
    }
    offsets.push(offset);
    let marker = |line: &str, ch: char| {
        let n = line.chars().take_while(|c| *c == ch).count();
        (n >= 7 && (line[n..].is_empty() || line[n..].starts_with([' ', '\r', '\n']))).then_some(n)
    };
    let mut i = 0;
    while i < lines.len() {
        let Some(size) = marker(lines[i], '<') else {
            i += 1;
            continue;
        };
        let start = i;
        let mut base = None;
        let mut middle = None;
        let mut end = None;
        i += 1;
        while i < lines.len() {
            if marker(lines[i], '<').is_some() {
                break;
            }
            if marker(lines[i], '|') == Some(size) && base.is_none() && middle.is_none() {
                base = Some(i)
            } else if lines[i].trim_end_matches(['\r', '\n']) == "=".repeat(size)
                && middle.is_none()
            {
                middle = Some(i)
            } else if marker(lines[i], '>') == Some(size) && middle.is_some() {
                end = Some(i);
                break;
            }
            i += 1;
        }
        if let (Some(mid), Some(end)) = (middle, end) {
            let current = lines[start + 1..base.unwrap_or(mid)].concat();
            let base = base.map(|b| lines[b + 1..mid].concat());
            let incoming = lines[mid + 1..end].concat();
            out.push(ConflictBlock {
                start: offsets[start],
                end: offsets[end + 1],
                current,
                incoming,
                base,
            });
            i = end + 1
        } else if i == start {
            i += 1
        }
    }
    out
}
pub(super) fn marker_warning(text: &str) -> bool {
    text.lines().any(|line| {
        line.starts_with("<<<<<<<")
            || line.starts_with("=======")
            || line.starts_with(">>>>>>>")
            || line.starts_with("|||||||")
    })
}

/// JSON journals preserve Unix paths that cannot be represented as Unicode.
mod raw_path {
    use super::*;
    pub fn serialize<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
        path.as_os_str().as_encoded_bytes().serialize(serializer)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> Result<PathBuf, D::Error> {
        let bytes = Vec::<u8>::deserialize(deserializer)?;
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt as _;
            Ok(PathBuf::from(OsString::from_vec(bytes)))
        }
        #[cfg(not(unix))]
        {
            String::from_utf8(bytes)
                .map(PathBuf::from)
                .map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::process::Command;
    pub fn git_ok(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "Git fixture command failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }
    fn run_continuation(
        root: &Path,
        source: &OperationDetails,
        action: Continuation,
    ) -> Result<String, String> {
        continue_operation(root, &capture_continuation(root, source)?, action)
    }
    fn git_fails(root: &Path, args: &[&str]) {
        assert!(
            !Command::new("git")
                .current_dir(root)
                .args(args)
                .output()
                .unwrap()
                .status
                .success()
        )
    }
    pub fn setup() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git_ok(root, &["init", "-b", "main"]);
        git_ok(root, &["config", "user.name", "Conflict Test"]);
        git_ok(root, &["config", "user.email", "conflict@example.com"]);
        git_ok(root, &["config", "commit.gpgSign", "false"]);
        git_ok(root, &["config", "tag.gpgSign", "false"]);
        git_ok(
            root,
            &[
                "config",
                "core.hooksPath",
                root.join(".git/hooks").to_str().unwrap(),
            ],
        );
        fs::write(root.join("file.txt"), "base\n").unwrap();
        git_ok(root, &["add", "-A"]);
        git_ok(root, &["commit", "-m", "base"]);
        dir
    }
    pub fn inject(root: &Path, path: &Path, sides: [Option<(&str, &[u8])>; 3]) {
        let mut input = Vec::new();
        input.extend_from_slice(format!("0 {}\t", "0".repeat(40)).as_bytes());
        input.extend_from_slice(path.as_os_str().as_encoded_bytes());
        input.push(0);
        for (i, side) in sides.into_iter().enumerate() {
            if let Some((mode, bytes)) = side {
                let oid = if mode == "160000" {
                    git_ok(root, &["rev-parse", "HEAD"])
                } else {
                    String::from_utf8(
                        command(
                            root,
                            vec!["hash-object".into(), "-w".into(), "--stdin".into()],
                            Some(bytes.to_vec()),
                            128,
                        )
                        .unwrap(),
                    )
                    .unwrap()
                    .trim()
                    .to_owned()
                };
                input.extend_from_slice(format!("{mode} {oid} {}\t", i + 1).as_bytes());
                input.extend_from_slice(path.as_os_str().as_encoded_bytes());
                input.push(0);
            }
        }
        command(
            root,
            vec!["update-index".into(), "-z".into(), "--index-info".into()],
            Some(input),
            2048,
        )
        .unwrap();
    }
    pub fn text_conflict(root: &Path, path: &Path) {
        inject(
            root,
            path,
            [
                Some(("100644", b"base\n")),
                Some(("100644", b"current\n")),
                Some(("100644", b"incoming\n")),
            ],
        );
        fs::write(
            root.join(path),
            "<<<<<<< HEAD\ncurrent\n=======\nincoming\n>>>>>>> other\n",
        )
        .unwrap();
    }
    fn real_conflict() -> tempfile::TempDir {
        let dir = setup();
        let root = dir.path();
        git_ok(root, &["switch", "-c", "topic"]);
        fs::write(root.join("file.txt"), "incoming\n").unwrap();
        git_ok(root, &["commit", "-am", "incoming"]);
        git_ok(root, &["switch", "main"]);
        fs::write(root.join("file.txt"), "current\n").unwrap();
        git_ok(root, &["commit", "-am", "current"]);
        dir
    }
    #[test]
    fn manual_result_and_deterministic_diff3_are_independent() {
        let dir = setup();
        text_conflict(dir.path(), Path::new("file.txt"));
        fs::write(dir.path().join("file.txt"), "manually merged\r\n").unwrap();
        let file = load(dir.path(), Path::new("file.txt")).unwrap();
        let text = file.text.unwrap();
        assert_eq!(text.result, "manually merged\r\n");
        let parsed = blocks(&text.generated);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].base.as_deref(), Some("base\n"));
        assert_eq!(parsed[0].current, "current\n");
        assert_eq!(parsed[0].incoming, "incoming\n");
        assert_eq!(
            diff3(dir.path(), &text.base, &text.current, &text.incoming).unwrap(),
            text.generated
        );
    }
    #[test]
    fn mark_resolved_stages_one_literal_path_and_preserves_other_conflicts() {
        let dir = setup();
        let path = Path::new(":(glob)*.txt");
        text_conflict(dir.path(), path);
        text_conflict(dir.path(), Path::new("file.txt"));
        let source = load(dir.path(), path).unwrap().source;
        resolve(
            dir.path(),
            &source,
            &Resolution::Text {
                result: "resolved\n".into(),
                stage: true,
            },
        )
        .unwrap();
        let remaining = unmerged(dir.path()).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, Path::new("file.txt"));
        assert_eq!(
            fs::read_to_string(dir.path().join(path)).unwrap(),
            "resolved\n"
        );
    }
    #[test]
    fn external_content_and_index_edits_refuse_overwrite() {
        let dir = setup();
        text_conflict(dir.path(), Path::new("file.txt"));
        let source = load(dir.path(), Path::new("file.txt")).unwrap().source;
        fs::write(dir.path().join("file.txt"), "external\n").unwrap();
        assert!(
            resolve(dir.path(), &source, &Resolution::Current)
                .unwrap_err()
                .contains("changed on disk")
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("file.txt")).unwrap(),
            "external\n"
        );
        let source = load(dir.path(), Path::new("file.txt")).unwrap().source;
        inject(
            dir.path(),
            Path::new("file.txt"),
            [
                None,
                Some(("100644", b"different\n")),
                Some(("100644", b"incoming\n")),
            ],
        );
        assert!(
            resolve(dir.path(), &source, &Resolution::Delete)
                .unwrap_err()
                .contains("index")
        );
    }
    #[cfg(unix)]
    #[test]
    fn crlf_and_executable_mode_survive_text_resolution() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = setup();
        text_conflict(dir.path(), Path::new("file.txt"));
        fs::write(dir.path().join("file.txt"), "old\r\n").unwrap();
        fs::set_permissions(
            dir.path().join("file.txt"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let source = load(dir.path(), Path::new("file.txt")).unwrap().source;
        let result =
            crate::editor::native::file_bytes_for_buffer("old\r\n", "merged\n").into_owned();
        resolve(
            dir.path(),
            &source,
            &Resolution::Text {
                result,
                stage: true,
            },
        )
        .unwrap();
        assert_eq!(
            fs::read(dir.path().join("file.txt")).unwrap(),
            b"merged\r\n"
        );
        assert_ne!(
            fs::metadata(dir.path().join("file.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
    }
    #[test]
    fn add_add_and_missing_result_use_three_way_merge() {
        let dir = setup();
        let path = Path::new("added.txt");
        inject(
            dir.path(),
            path,
            [
                None,
                Some(("100644", b"ours\n")),
                Some(("100644", b"theirs\n")),
            ],
        );
        let file = load(dir.path(), path).unwrap();
        assert!(file.text.as_ref().unwrap().base.is_empty());
        assert_eq!(blocks(&file.text.as_ref().unwrap().result).len(), 1);
        resolve(
            dir.path(),
            &file.source,
            &Resolution::Text {
                result: "both\n".into(),
                stage: true,
            },
        )
        .unwrap();
        assert_eq!(fs::read_to_string(dir.path().join(path)).unwrap(), "both\n");
    }
    #[test]
    fn deletion_binary_invalid_and_oversized_conflicts_have_whole_file_choices() {
        let dir = setup();
        let root = dir.path();
        let path = Path::new("file.txt");
        inject(
            root,
            path,
            [
                Some(("100644", b"base")),
                None,
                Some(("100644", b"survivor")),
            ],
        );
        let file = load(root, path).unwrap();
        assert!(file.text.is_none());
        assert!(file.reason.contains("delete"));
        resolve(root, &file.source, &Resolution::Incoming).unwrap();
        assert_eq!(fs::read(root.join(path)).unwrap(), b"survivor");
        for (bytes, label) in [
            (vec![0, 1, 2], "Binary"),
            (vec![255, 254], "UTF-8"),
            (vec![b'x'; MAX_TEXT_BYTES + 1], "2 MiB"),
        ] {
            inject(
                root,
                path,
                [None, Some(("100644", &bytes)), Some(("100644", b"text"))],
            );
            let file = load(root, path).unwrap();
            assert!(file.text.is_none());
            assert!(file.reason.contains(label), "{}", file.reason);
            resolve(root, &file.source, &Resolution::Current).unwrap();
            assert_eq!(fs::read(root.join(path)).unwrap(), bytes);
        }
        inject(
            root,
            path,
            [
                Some(("100644", b"base")),
                Some(("100644", b"survivor")),
                None,
            ],
        );
        let file = load(root, path).unwrap();
        resolve(root, &file.source, &Resolution::Delete).unwrap();
        assert!(!root.join(path).exists());
        assert!(unmerged(root).unwrap().is_empty());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_and_submodule_choices_do_not_follow_targets() {
        use std::os::unix::fs::symlink;
        let dir = setup();
        let root = dir.path();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("target");
        fs::write(&target, "safe").unwrap();
        let path = Path::new("link");
        symlink(&target, root.join(path)).unwrap();
        inject(
            root,
            path,
            [
                None,
                Some(("120000", b"new-target")),
                Some(("120000", b"other-target")),
            ],
        );
        let file = load(root, path).unwrap();
        assert!(file.reason.contains("Symbolic"));
        resolve(root, &file.source, &Resolution::Current).unwrap();
        assert_eq!(
            fs::read_link(root.join(path)).unwrap(),
            Path::new("new-target")
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "safe");
        let path = Path::new("submodule");
        fs::create_dir(root.join(path)).unwrap();
        fs::write(root.join(path).join("draft"), "keep").unwrap();
        inject(
            root,
            path,
            [None, Some(("160000", b"")), Some(("160000", b""))],
        );
        let file = load(root, path).unwrap();
        assert!(file.reason.contains("Submodule"));
        resolve(root, &file.source, &Resolution::Incoming).unwrap();
        assert_eq!(
            fs::read_to_string(root.join(path).join("draft")).unwrap(),
            "keep"
        );
        assert!(unmerged(root).unwrap().is_empty());
    }
    #[test]
    fn directories_are_never_replaced_or_recursively_deleted() {
        let dir = setup();
        let root = dir.path();
        let path = Path::new("directory");
        fs::create_dir(root.join(path)).unwrap();
        fs::write(root.join(path).join("draft"), "keep").unwrap();
        inject(
            root,
            path,
            [None, Some(("100644", b"file")), Some(("100644", b"other"))],
        );
        let file = load(root, path).unwrap();
        assert!(file.reason.contains("directory"));
        assert!(resolve(root, &file.source, &Resolution::Current).is_err());
        assert!(resolve(root, &file.source, &Resolution::Delete).is_err());
        assert_eq!(
            fs::read_to_string(root.join(path).join("draft")).unwrap(),
            "keep"
        );
    }
    #[test]
    fn failed_staging_keeps_saved_result_and_unmerged_index() {
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        fs::write(root.join(".gitattributes"), "file.txt filter=broken\n").unwrap();
        git_ok(root, &["config", "filter.broken.clean", "false"]);
        git_ok(root, &["config", "filter.broken.required", "true"]);
        let source = load(root, Path::new("file.txt")).unwrap().source;
        assert!(
            resolve(
                root,
                &source,
                &Resolution::Text {
                    result: "saved draft\n".into(),
                    stage: true
                }
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "saved draft\n"
        );
        assert_eq!(unmerged(root).unwrap().len(), 1);
    }
    #[test]
    fn merge_continue_rejects_unresolved_and_stale_state() {
        let dir = real_conflict();
        let root = dir.path();
        git_fails(root, &["merge", "topic"]);
        let source = operation_details(root).unwrap().unwrap();
        assert_eq!(source.kind, OperationState::Merge);
        assert!(source.incoming_label.contains("incoming"));
        assert!(
            run_continuation(root, &source, Continuation::Continue)
                .unwrap_err()
                .contains("unmerged")
        );
        run_continuation(root, &source, Continuation::Abort).unwrap();
        assert!(operation_details(root).unwrap().is_none());
        assert!(
            run_continuation(root, &source, Continuation::Continue)
                .unwrap_err()
                .contains("changed")
        );
    }
    #[cfg(unix)]
    #[test]
    fn failed_merge_hook_output_is_captured_and_continuation_remains_available() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = real_conflict();
        let root = dir.path();
        git_fails(root, &["merge", "topic"]);
        let file = load(root, Path::new("file.txt")).unwrap();
        resolve(
            root,
            &file.source,
            &Resolution::Text {
                result: "merged\n".into(),
                stage: true,
            },
        )
        .unwrap();
        let hook = root.join(".git/hooks/pre-commit");
        fs::write(
            &hook,
            "#!/bin/sh\nprintf 'hook says please fix it\\n'\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        let source = operation_details(root).unwrap().unwrap();
        assert!(
            run_continuation(root, &source, Continuation::Continue)
                .unwrap_err()
                .contains("hook says please fix it")
        );
        assert!(operation_details(root).unwrap().is_some());
        fs::remove_file(hook).unwrap();
        run_continuation(root, &source, Continuation::Continue).unwrap();
        assert!(operation_details(root).unwrap().is_none());
    }
    #[test]
    fn rebase_continue_skip_and_abort_use_normal_git_state() {
        for action in [
            Continuation::Continue,
            Continuation::Skip,
            Continuation::Abort,
        ] {
            let dir = real_conflict();
            let root = dir.path();
            git_ok(root, &["switch", "topic"]);
            git_fails(root, &["rebase", "main"]);
            let details = operation_details(root).unwrap().unwrap();
            assert_eq!(details.kind, OperationState::Rebase);
            assert!(details.explanation.contains("new base"));
            assert!(details.incoming_label.contains("incoming"));
            if action == Continuation::Continue {
                let file = load(root, Path::new("file.txt")).unwrap();
                assert_eq!(file.text.as_ref().unwrap().current, "current\n");
                assert_eq!(file.text.as_ref().unwrap().incoming, "incoming\n");
                resolve(
                    root,
                    &file.source,
                    &Resolution::Text {
                        result: "replayed\n".into(),
                        stage: true,
                    },
                )
                .unwrap();
            }
            run_continuation(root, &details, action).unwrap();
            assert!(operation_details(root).unwrap().is_none());
        }
    }
    #[test]
    fn cherry_pick_and_revert_continue_and_abort() {
        for kind in [OperationState::CherryPick, OperationState::Revert] {
            for abort in [false, true] {
                let dir = real_conflict();
                let root = dir.path();
                let target = if kind == OperationState::CherryPick {
                    git_ok(root, &["rev-parse", "topic"])
                } else {
                    git_ok(root, &["rev-parse", "HEAD"])
                };
                if kind == OperationState::Revert {
                    fs::write(root.join("file.txt"), "later\n").unwrap();
                    git_ok(root, &["commit", "-am", "later"]);
                }
                git_fails(
                    root,
                    &[
                        if kind == OperationState::CherryPick {
                            "cherry-pick"
                        } else {
                            "revert"
                        },
                        &target,
                    ],
                );
                let source = operation_details(root).unwrap().unwrap();
                assert_eq!(source.kind, kind);
                if !abort {
                    let file = load(root, Path::new("file.txt")).unwrap();
                    resolve(
                        root,
                        &file.source,
                        &Resolution::Text {
                            result: "resolved\n".into(),
                            stage: true,
                        },
                    )
                    .unwrap();
                }
                run_continuation(
                    root,
                    &source,
                    if abort {
                        Continuation::Abort
                    } else {
                        Continuation::Continue
                    },
                )
                .unwrap();
                assert!(operation_details(root).unwrap().is_none());
            }
        }
    }
    #[test]
    fn linked_worktree_operation_state_is_checkout_scoped() {
        let dir = real_conflict();
        let other = tempfile::tempdir().unwrap();
        let worktree = other.path().join("linked");
        git_ok(
            dir.path(),
            &[
                "worktree",
                "add",
                "-b",
                "linked",
                worktree.to_str().unwrap(),
                "main",
            ],
        );
        git_fails(&worktree, &["merge", "topic"]);
        let snapshot = super::super::repository::load_snapshot(&worktree).unwrap();
        assert_eq!(snapshot.operation, Some(OperationState::Merge));
        assert_eq!(snapshot.conflict_entries.len(), 1);
        assert!(operation_details(dir.path()).unwrap().is_none());
        run_continuation(
            &worktree,
            snapshot.operation_details.as_ref().unwrap(),
            Continuation::Abort,
        )
        .unwrap();
    }
    #[test]
    fn marker_parser_handles_diff3_crlf_unicode_and_incomplete_markers() {
        let text = "prefix\r\n<<<<<<< Current\r\nবাংলা\r\n||||||| Base\r\nbase\r\n=======\r\nincoming\r\n>>>>>>> Incoming\r\nsuffix\r\n";
        let parsed = blocks(text);
        assert_eq!(parsed.len(), 1);
        let block = &parsed[0];
        assert_eq!(&text[..block.start], "prefix\r\n");
        assert_eq!(&text[block.end..], "suffix\r\n");
        assert_eq!(block.current, "বাংলা\r\n");
        assert_eq!(block.base.as_deref(), Some("base\r\n"));
        assert!(blocks("<<<<<<< Current\nunfinished").is_empty());
        assert!(marker_warning("<<<<<<< this might be sample text"));
    }
    #[cfg(unix)]
    #[test]
    fn index_parser_and_journal_identity_preserve_raw_paths() {
        use std::os::unix::ffi::OsStringExt as _;
        let path = PathBuf::from(OsString::from_vec(b"raw-\xff\tfile".to_vec()));
        let oid = "a".repeat(40);
        let bytes = format!("100644 {oid} 2\t").into_bytes();
        let mut input = bytes;
        input.extend_from_slice(path.as_os_str().as_encoded_bytes());
        input.push(0);
        let parsed = parse_unmerged(&input).unwrap();
        assert_eq!(parsed[0].path, path);
        let source = Source {
            index: parsed[0].clone(),
            disk: Baseline::Missing,
            operation: None,
        };
        let roundtrip: Source =
            serde_json::from_slice(&serde_json::to_vec(&source).unwrap()).unwrap();
        assert_eq!(source, roundtrip);
    }
}

#[cfg(test)]
mod continuation_safety_tests {
    use super::tests::*;
    use super::*;
    use std::process::Command;
    #[test]
    fn stale_skip_and_abort_refuse_external_index_or_worktree_edits() {
        for action in [Continuation::Skip, Continuation::Abort] {
            let dir = setup();
            let root = dir.path();
            git_ok(root, &["switch", "-c", "topic"]);
            fs::write(root.join("file.txt"), "incoming\n").unwrap();
            git_ok(root, &["commit", "-am", "incoming"]);
            git_ok(root, &["switch", "main"]);
            fs::write(root.join("file.txt"), "current\n").unwrap();
            git_ok(root, &["commit", "-am", "current"]);
            git_ok(root, &["switch", "topic"]);
            assert!(
                !Command::new("git")
                    .current_dir(root)
                    .args(["rebase", "main"])
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            let operation = operation_details(root).unwrap().unwrap();
            let source = capture_continuation(root, &operation).unwrap();
            fs::write(root.join("file.txt"), "NEW_UNSAVED_WORK\n").unwrap();
            assert!(
                continue_operation(root, &source, action)
                    .unwrap_err()
                    .contains("working tree changed")
            );
            assert_eq!(
                fs::read_to_string(root.join("file.txt")).unwrap(),
                "NEW_UNSAVED_WORK\n"
            );
            assert!(operation_details(root).unwrap().is_some());
            let source = capture_continuation(root, &operation).unwrap();
            git_ok(root, &["add", "file.txt"]);
            assert!(
                continue_operation(root, &source, action)
                    .unwrap_err()
                    .contains("index")
            );
            assert!(operation_details(root).unwrap().is_some());
        }
    }
}

#[cfg(all(test, unix))]
mod stage_race_tests {
    use super::tests::*;
    use super::*;
    #[test]
    fn external_content_or_mode_change_during_add_is_not_reported_as_resolved() {
        for change_mode in [false, true] {
            let dir = setup();
            let root = dir.path();
            text_conflict(root, Path::new("file.txt"));
            let source = load(root, Path::new("file.txt")).unwrap().source;
            let script = root.join(".git/change-on-second-filter");
            fs::write(&script,if change_mode{"#!/bin/sh\ncount=0\n[ ! -f \"$1\" ] || read -r count < \"$1\"\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"$1\"\nif [ \"$count\" -ge 2 ]; then chmod +x \"$2\"; fi\ncat\n"}else{"#!/bin/sh\ncount=0\n[ ! -f \"$1\" ] || read -r count < \"$1\"\ncount=$((count + 1))\nprintf '%s\\n' \"$count\" > \"$1\"\nif [ \"$count\" -ge 2 ]; then printf 'external edit\\n' > \"$2\"; fi\ncat\n"}).unwrap();
            let quote = crate::agent_activity::shell_quote;
            let filter = format!(
                "sh {} {} {}",
                quote(script.to_str().unwrap()),
                quote(root.join(".git/filter-count").to_str().unwrap()),
                quote(root.join("file.txt").to_str().unwrap())
            );
            git_ok(root, &["config", "filter.race.clean", &filter]);
            git_ok(root, &["config", "filter.race.required", "true"]);
            fs::write(root.join(".gitattributes"), "file.txt filter=race\n").unwrap();
            let error = resolve(
                root,
                &source,
                &Resolution::Text {
                    result: "submitted result\n".into(),
                    stage: true,
                },
            )
            .unwrap_err();
            assert!(error.contains("while staging"), "{error}");
            if change_mode {
                use std::os::unix::fs::PermissionsExt as _;
                assert_ne!(
                    fs::metadata(root.join("file.txt"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o111,
                    0
                );
            } else {
                assert_eq!(
                    fs::read_to_string(root.join("file.txt")).unwrap(),
                    "external edit\n"
                );
            }
        }
    }
    #[test]
    fn core_filemode_false_preserves_worktree_permissions_and_index_policy() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = setup();
        let root = dir.path();
        text_conflict(root, Path::new("file.txt"));
        git_ok(root, &["config", "core.filemode", "false"]);
        fs::set_permissions(root.join("file.txt"), fs::Permissions::from_mode(0o755)).unwrap();
        let source = load(root, Path::new("file.txt")).unwrap().source;
        resolve(
            root,
            &source,
            &Resolution::Text {
                result: "merged\n".into(),
                stage: true,
            },
        )
        .unwrap();
        assert_ne!(
            fs::metadata(root.join("file.txt"))
                .unwrap()
                .permissions()
                .mode()
                & 0o111,
            0
        );
        assert!(git_ok(root, &["ls-files", "--stage", "file.txt"]).starts_with("100644"));
    }
}
