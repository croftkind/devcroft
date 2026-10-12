//! Checkout-aware content pairs for the shared diff viewer.

use std::{
    ffi::OsString,
    fs,
    io::Read as _,
    path::{Component, Path},
    time::Duration,
};

use crate::diff::{
    kit_viewer::PreparedKitDiff,
    model::{ChangedFile, FileContent, FileStatus, UnavailableReason, count_changes, diff_text},
    viewer::PreparedFileDiff,
};

use super::{
    model::{Change, ChangeKind},
    repository::{LoadError, run_git_args},
};

const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChangeArea {
    Conflict,
    Staged,
    Unstaged,
    Untracked,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DiffTarget {
    pub(super) area: ChangeArea,
    pub(super) change: Change,
}

impl DiffTarget {
    pub(super) fn label(&self) -> String {
        self.change.path.to_string_lossy().into_owned()
    }
}

pub(super) fn load_change_diff(
    root: &Path,
    target: &DiffTarget,
) -> Result<PreparedFileDiff, String> {
    if target.area == ChangeArea::Conflict {
        return Ok(PreparedFileDiff::new(unavailable_file(
            &target.change,
            UnavailableReason::Unsupported,
        )));
    }
    let path = &target.change.path;
    let original = target.change.original_path.as_ref().unwrap_or(path);
    let (old, new) = match target.area {
        ChangeArea::Staged => match target.change.kind {
            ChangeKind::Added | ChangeKind::Copied => (Content::empty(), read_index(root, path)?),
            ChangeKind::Deleted => (read_head(root, path)?, Content::empty()),
            ChangeKind::Renamed => (read_head(root, original)?, read_index(root, path)?),
            _ => (read_head(root, path)?, read_index(root, path)?),
        },
        ChangeArea::Unstaged => match target.change.kind {
            ChangeKind::Deleted => (read_index(root, path)?, Content::empty()),
            ChangeKind::Renamed => (read_index(root, original)?, read_worktree(root, path)),
            _ => (read_index(root, path)?, read_worktree(root, path)),
        },
        ChangeArea::Untracked => (Content::empty(), read_worktree(root, path)),
        ChangeArea::Conflict => unreachable!(),
    };
    Ok(PreparedFileDiff::new(classify(target, old, new)))
}

/// Raw tree entries pin both sides to immutable objects, independent of HEAD/index.
pub(super) fn load_history_diff(
    root: &Path,
    file: &super::history::HistoryFile,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<PreparedKitDiff, String> {
    let (old, new) = load_history_pair(root, file, cancel)?;
    let target = DiffTarget {
        area: ChangeArea::Staged,
        change: file.change.clone(),
    };
    let changed = classify_content(&target, &old, &new);
    PreparedKitDiff::new(
        changed,
        old.text(),
        new.text(),
        [&file.old_mode, &file.new_mode],
    )
}

fn load_history_pair(
    root: &Path,
    file: &super::history::HistoryFile,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(Content, Content), String> {
    let old = if file.old_mode == "000000" {
        Content::empty()
    } else {
        read_object_with_cancel(
            root,
            file.old_mode.as_bytes(),
            file.old_oid.as_bytes(),
            Some(cancel),
        )?
    };
    let new = if file.new_mode == "000000" {
        Content::empty()
    } else {
        read_object_with_cancel(
            root,
            file.new_mode.as_bytes(),
            file.new_oid.as_bytes(),
            Some(cancel),
        )?
    };
    Ok((old, new))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContentKind {
    Empty,
    File,
    Symlink,
}

enum Content {
    Bytes { kind: ContentKind, bytes: Vec<u8> },
    TooLarge,
    Missing,
    Submodule,
}

impl Content {
    fn text(&self) -> &str {
        match self {
            Self::Bytes { bytes, .. } => std::str::from_utf8(bytes).unwrap_or(""),
            _ => "",
        }
    }

    fn empty() -> Self {
        Self::Bytes {
            kind: ContentKind::Empty,
            bytes: Vec::new(),
        }
    }
}

fn classify(target: &DiffTarget, old: Content, new: Content) -> ChangedFile {
    classify_content(target, &old, &new)
}

fn classify_content(target: &DiffTarget, old: &Content, new: &Content) -> ChangedFile {
    let unavailable = match (old, new) {
        (Content::Submodule, _) | (_, Content::Submodule) => Some(UnavailableReason::Submodule),
        (Content::TooLarge, _) | (_, Content::TooLarge) => Some(UnavailableReason::TooLarge),
        (Content::Missing, _) | (_, Content::Missing) => Some(UnavailableReason::Missing),
        (Content::Bytes { kind: old_kind, .. }, Content::Bytes { kind: new_kind, .. })
            if *old_kind != ContentKind::Empty
                && *new_kind != ContentKind::Empty
                && old_kind != new_kind =>
        {
            Some(UnavailableReason::Unsupported)
        }
        _ => None,
    };
    if let Some(reason) = unavailable {
        return unavailable_file(&target.change, reason);
    }
    let (Content::Bytes { bytes: old, .. }, Content::Bytes { bytes: new, .. }) = (old, new) else {
        unreachable!("unavailable content returned above")
    };
    if is_binary(old) || is_binary(new) {
        return unavailable_file(&target.change, UnavailableReason::Binary);
    }
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(new)) else {
        return unavailable_file(&target.change, UnavailableReason::InvalidUtf8);
    };
    let (additions, deletions) = count_changes(old, new);
    let (hunks, truncated) = diff_text(old, new);
    ChangedFile {
        path: target.change.path.to_string_lossy().into_owned(),
        old_path: target
            .change
            .original_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        status: file_status(target.change.kind),
        additions,
        deletions,
        content: FileContent::Text { hunks, truncated },
    }
}

fn unavailable_file(change: &Change, reason: UnavailableReason) -> ChangedFile {
    ChangedFile {
        path: change.path.to_string_lossy().into_owned(),
        old_path: change
            .original_path
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        status: file_status(change.kind),
        additions: 0,
        deletions: 0,
        content: FileContent::Unavailable(reason),
    }
}

fn file_status(kind: ChangeKind) -> FileStatus {
    match kind {
        ChangeKind::Added | ChangeKind::Untracked | ChangeKind::Copied => FileStatus::Added,
        ChangeKind::Deleted => FileStatus::Deleted,
        ChangeKind::Renamed => FileStatus::Renamed,
        ChangeKind::TypeChanged | ChangeKind::Unmerged => FileStatus::TypeChanged,
        ChangeKind::Modified | ChangeKind::Unknown => FileStatus::Modified,
    }
}

fn read_head(root: &Path, path: &Path) -> Result<Content, String> {
    let mut args = vec![
        OsString::from("--literal-pathspecs"),
        OsString::from("ls-tree"),
        OsString::from("-z"),
        OsString::from("HEAD"),
        OsString::from("--"),
    ];
    args.push(path.as_os_str().to_owned());
    let output = run(root, args, MAX_METADATA_BYTES)?;
    if !output.status.success() || output.stdout.is_empty() {
        return Ok(Content::Missing);
    }
    let Some(tab) = output.stdout.iter().position(|byte| *byte == b'\t') else {
        return Ok(Content::Missing);
    };
    let metadata = &output.stdout[..tab];
    let record_path = output.stdout[tab + 1..]
        .strip_suffix(&[0])
        .unwrap_or(&output.stdout[tab + 1..]);
    if record_path != path_bytes(path) {
        return Ok(Content::Missing);
    }
    let mut fields = metadata.split(|byte| *byte == b' ');
    let mode = fields.next().unwrap_or_default();
    let _kind = fields.next();
    let oid = fields.next().unwrap_or_default();
    read_object(root, mode, oid)
}

fn read_index(root: &Path, path: &Path) -> Result<Content, String> {
    let mut args = vec![
        OsString::from("--literal-pathspecs"),
        OsString::from("ls-files"),
        OsString::from("--stage"),
        OsString::from("-z"),
        OsString::from("--"),
    ];
    args.push(path.as_os_str().to_owned());
    let output = run(root, args, MAX_METADATA_BYTES)?;
    if !output.status.success() {
        return Ok(Content::Missing);
    }
    for record in output.stdout.split(|byte| *byte == 0) {
        let Some(tab) = record.iter().position(|byte| *byte == b'\t') else {
            continue;
        };
        if &record[tab + 1..] != path_bytes(path) {
            continue;
        }
        let mut fields = record[..tab].split(|byte| *byte == b' ');
        let mode = fields.next().unwrap_or_default();
        let oid = fields.next().unwrap_or_default();
        let stage = fields.next().unwrap_or_default();
        if stage == b"0" {
            return read_object(root, mode, oid);
        }
    }
    Ok(Content::Missing)
}

fn read_object(root: &Path, mode: &[u8], oid: &[u8]) -> Result<Content, String> {
    read_object_with_cancel(root, mode, oid, None)
}

fn read_object_with_cancel(
    root: &Path,
    mode: &[u8],
    oid: &[u8],
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Content, String> {
    let run = |root: &Path, args, limit| {
        if let Some(cancel) = cancel {
            super::repository::run_git_cancellable(root, args, limit, cancel).map_err(|error| {
                match error {
                    LoadError::Failed(message) => message,
                    _ => "Could not read Git object".into(),
                }
            })
        } else {
            run(root, args, limit)
        }
    };
    if mode == b"160000" {
        return Ok(Content::Submodule);
    }
    if oid.is_empty() {
        return Ok(Content::Missing);
    }
    let size = run(
        root,
        vec![
            OsString::from("cat-file"),
            OsString::from("-s"),
            os_string(oid),
        ],
        128,
    )?;
    if !size.status.success() {
        return Ok(Content::Missing);
    }
    let Ok(size) = String::from_utf8_lossy(&size.stdout)
        .trim()
        .parse::<usize>()
    else {
        return Ok(Content::Missing);
    };
    if size > MAX_FILE_BYTES {
        return Ok(Content::TooLarge);
    }
    let output = run(
        root,
        vec![
            OsString::from("cat-file"),
            OsString::from("blob"),
            os_string(oid),
        ],
        MAX_FILE_BYTES + 1,
    )?;
    if !output.status.success() {
        return Ok(Content::Missing);
    }
    if output.stdout_truncated || output.stdout.len() > MAX_FILE_BYTES {
        return Ok(Content::TooLarge);
    }
    Ok(Content::Bytes {
        kind: if mode == b"120000" {
            ContentKind::Symlink
        } else {
            ContentKind::File
        },
        bytes: output.stdout,
    })
}

fn read_worktree(root: &Path, path: &Path) -> Content {
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Content::Missing;
    }
    let mut candidate = root.to_owned();
    let components = path.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        candidate.push(component.as_os_str());
        if index + 1 < components.len()
            && fs::symlink_metadata(&candidate).is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Content::Missing;
        }
    }
    let Ok(metadata) = fs::symlink_metadata(&candidate) else {
        return Content::Missing;
    };
    if metadata.file_type().is_symlink() {
        return fs::read_link(&candidate).map_or(Content::Missing, |target| Content::Bytes {
            kind: ContentKind::Symlink,
            bytes: path_bytes(&target).to_vec(),
        });
    }
    if !metadata.is_file() {
        return Content::Missing;
    }
    if metadata.len() > MAX_FILE_BYTES as u64 {
        return Content::TooLarge;
    }
    let Ok(mut file) = fs::File::open(candidate) else {
        return Content::Missing;
    };
    let mut bytes = Vec::new();
    if file
        .by_ref()
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Content::Missing;
    }
    if bytes.len() > MAX_FILE_BYTES {
        Content::TooLarge
    } else {
        Content::Bytes {
            kind: ContentKind::File,
            bytes,
        }
    }
}

fn run(
    root: &Path,
    args: Vec<OsString>,
    stdout_limit: usize,
) -> Result<super::repository::CommandOutput, String> {
    run_git_args(root, args, None, READ_TIMEOUT, stdout_limit, 2 * 1024).map_err(
        |error| match error {
            LoadError::GitUnavailable => "Git is unavailable".to_owned(),
            LoadError::NotRepository => "This folder is not a Git repository".to_owned(),
            LoadError::Failed(message) => message,
        },
    )
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8 * 1024).any(|byte| *byte == 0)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> &[u8] {
    use std::os::unix::ffi::OsStrExt as _;
    path.as_os_str().as_bytes()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_encoded_bytes()
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    OsString::from_vec(bytes.to_vec())
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, process::Command};

    fn git(root: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .expect("git runs");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn setup() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-b", "main"]);
        git(temp.path(), &["config", "user.name", "Test"]);
        git(temp.path(), &["config", "user.email", "test@example.com"]);
        git(temp.path(), &["config", "commit.gpgsign", "false"]);
        fs::write(temp.path().join("tracked.txt"), "before\n").unwrap();
        git(temp.path(), &["add", "tracked.txt"]);
        git(temp.path(), &["commit", "-m", "initial"]);
        temp
    }

    #[test]
    fn loads_staged_unstaged_and_untracked_content_pairs() {
        let temp = setup();
        fs::write(temp.path().join("tracked.txt"), "staged\n").unwrap();
        git(temp.path(), &["add", "tracked.txt"]);
        fs::write(temp.path().join("tracked.txt"), "worktree\n").unwrap();
        fs::write(temp.path().join("new.txt"), "new\n").unwrap();

        for (area, change, additions, deletions) in [
            (
                ChangeArea::Staged,
                Change {
                    path: "tracked.txt".into(),
                    original_path: None,
                    kind: ChangeKind::Modified,
                    index_mode: None,
                    index_oid: None,
                },
                1,
                1,
            ),
            (
                ChangeArea::Unstaged,
                Change {
                    path: "tracked.txt".into(),
                    original_path: None,
                    kind: ChangeKind::Modified,
                    index_mode: None,
                    index_oid: None,
                },
                1,
                1,
            ),
            (
                ChangeArea::Untracked,
                Change {
                    path: "new.txt".into(),
                    original_path: None,
                    kind: ChangeKind::Untracked,
                    index_mode: None,
                    index_oid: None,
                },
                1,
                0,
            ),
        ] {
            let target = DiffTarget { area, change };
            let (old, new) = match area {
                ChangeArea::Staged => (
                    read_head(temp.path(), &target.change.path).unwrap(),
                    read_index(temp.path(), &target.change.path).unwrap(),
                ),
                ChangeArea::Unstaged => (
                    read_index(temp.path(), &target.change.path).unwrap(),
                    read_worktree(temp.path(), &target.change.path),
                ),
                ChangeArea::Untracked => (
                    Content::empty(),
                    read_worktree(temp.path(), &target.change.path),
                ),
                ChangeArea::Conflict => unreachable!(),
            };
            let file = classify(&target, old, new);
            assert_eq!((file.additions, file.deletions), (additions, deletions));
            assert!(matches!(file.content, FileContent::Text { .. }));
        }
    }

    #[test]
    fn classifies_binary_invalid_utf8_and_oversized_worktree_files() {
        let temp = setup();
        for (name, bytes, reason) in [
            ("binary.bin", vec![0, 1], UnavailableReason::Binary),
            ("invalid.txt", vec![0xff], UnavailableReason::InvalidUtf8),
            (
                "large.txt",
                vec![b'x'; MAX_FILE_BYTES + 1],
                UnavailableReason::TooLarge,
            ),
        ] {
            fs::write(temp.path().join(name), bytes).unwrap();
            let target = DiffTarget {
                area: ChangeArea::Untracked,
                change: Change {
                    path: name.into(),
                    original_path: None,
                    kind: ChangeKind::Untracked,
                    index_mode: None,
                    index_oid: None,
                },
            };
            let file = classify(
                &target,
                Content::empty(),
                read_worktree(temp.path(), &target.change.path),
            );
            assert_eq!(file.content, FileContent::Unavailable(reason));
        }
    }

    #[test]
    fn missing_and_submodule_content_have_explicit_states() {
        let temp = setup();
        let missing = DiffTarget {
            area: ChangeArea::Untracked,
            change: Change {
                path: "gone.txt".into(),
                original_path: None,
                kind: ChangeKind::Untracked,
                index_mode: None,
                index_oid: None,
            },
        };
        assert_eq!(
            classify(
                &missing,
                Content::empty(),
                read_worktree(temp.path(), &missing.change.path)
            )
            .content,
            FileContent::Unavailable(UnavailableReason::Missing)
        );

        let oid = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(temp.path())
            .output()
            .unwrap();
        let oid = String::from_utf8(oid.stdout).unwrap();
        git(
            temp.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000",
                oid.trim(),
                "nested",
            ],
        );
        let submodule = DiffTarget {
            area: ChangeArea::Staged,
            change: Change {
                path: "nested".into(),
                original_path: None,
                kind: ChangeKind::Added,
                index_mode: None,
                index_oid: None,
            },
        };
        assert_eq!(
            classify(
                &submodule,
                Content::empty(),
                read_index(temp.path(), &submodule.change.path).unwrap()
            )
            .content,
            FileContent::Unavailable(UnavailableReason::Submodule)
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_type_changes_and_non_utf8_paths_remain_usable() {
        use std::os::unix::ffi::OsStringExt as _;
        use std::os::unix::fs::symlink;

        let temp = setup();
        fs::remove_file(temp.path().join("tracked.txt")).unwrap();
        symlink("target.txt", temp.path().join("tracked.txt")).unwrap();
        let changed_type = DiffTarget {
            area: ChangeArea::Unstaged,
            change: Change {
                path: "tracked.txt".into(),
                original_path: None,
                kind: ChangeKind::TypeChanged,
                index_mode: None,
                index_oid: None,
            },
        };
        assert_eq!(
            classify(
                &changed_type,
                read_index(temp.path(), &changed_type.change.path).unwrap(),
                read_worktree(temp.path(), &changed_type.change.path)
            )
            .content,
            FileContent::Unavailable(UnavailableReason::Unsupported)
        );

        let raw = PathBuf::from(OsString::from_vec(b"invalid-\xff.txt".to_vec()));
        if fs::write(temp.path().join(&raw), "text\n").is_err() {
            // Some Unix filesystems (notably the sandboxed macOS test
            // volume) reject this byte sequence. The porcelain parser still
            // has a platform-independent raw-path test.
            return;
        }
        let target = DiffTarget {
            area: ChangeArea::Untracked,
            change: Change {
                path: raw,
                original_path: None,
                kind: ChangeKind::Untracked,
                index_mode: None,
                index_oid: None,
            },
        };
        assert!(matches!(
            classify(
                &target,
                Content::empty(),
                read_worktree(temp.path(), &target.change.path)
            )
            .content,
            FileContent::Text { .. }
        ));
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn immutable_history_pairs_ignore_current_head_index_and_worktree() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let git = |args: &[&str], input: Option<&[u8]>| {
            let out = run_git_args(
                root,
                args.iter().map(Into::into).collect(),
                input.map(Vec::from),
                READ_TIMEOUT,
                1024,
                1024,
            )
            .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "-b", "main"], None);
        let old = git(&["hash-object", "-w", "--stdin"], Some(b"historical old\n"));
        let new = git(&["hash-object", "-w", "--stdin"], Some(b"historical new\n"));
        fs::write(root.join("file.txt"), "unrelated worktree\n").unwrap();
        git(&["add", "file.txt"], None);
        let mut file = super::super::history::HistoryFile {
            change: Change {
                path: "file.txt".into(),
                original_path: None,
                kind: ChangeKind::Modified,
                index_mode: None,
                index_oid: None,
            },
            old_mode: "100644".into(),
            new_mode: "100644".into(),
            old_oid: old,
            new_oid: new,
        };
        let cancel = AtomicBool::new(false);
        let diff = load_history_diff(root, &file, &cancel).unwrap().file;
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        let FileContent::Text { hunks, .. } = diff.content else {
            panic!("text diff expected")
        };
        let text = format!("{hunks:?}");
        assert!(text.contains("historical old"));
        assert!(text.contains("historical new"));
        assert!(!text.contains("unrelated"));
        file.old_mode = "000000".into();
        file.change.kind = ChangeKind::Added;
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .deletions,
            0
        );
        file.old_mode = "100644".into();
        file.new_mode = "000000".into();
        file.change.kind = ChangeKind::Deleted;
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .additions,
            0
        );
        file.new_mode = "120000".into();
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .content,
            FileContent::Unavailable(UnavailableReason::Unsupported)
        );
        file.new_mode = "160000".into();
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .content,
            FileContent::Unavailable(UnavailableReason::Submodule)
        );
        file.new_mode = "100644".into();
        file.new_oid = git(&["hash-object", "-w", "--stdin"], Some(&[0, 1]));
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .content,
            FileContent::Unavailable(UnavailableReason::Binary)
        );
        file.new_oid = git(&["hash-object", "-w", "--stdin"], Some(&[0xff]));
        assert_eq!(
            load_history_diff(root, &file, &cancel)
                .unwrap()
                .file
                .content,
            FileContent::Unavailable(UnavailableReason::InvalidUtf8)
        );
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(
            load_history_diff(root, &file, &cancel)
                .err()
                .unwrap()
                .contains("cancelled")
        );
    }
}
