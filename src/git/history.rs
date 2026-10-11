//! Paged history pinned to immutable commit ids, with raw paths and tree entries.
use std::{ffi::OsString, path::Path, sync::atomic::AtomicBool};

use super::{
    model::{Change, ChangeKind},
    repository::{LoadError, run_git_cancellable, stderr_message},
};

pub(super) const PAGE_SIZE: usize = 100;
const MAX_HISTORY_BYTES: usize = 8 * 1024 * 1024;
const FORMAT: &str =
    "--format=%H%x00%P%x00%s%x00%an%x00%ae%x00%aI%x00%cn%x00%ce%x00%cI%x00%D%x00%b";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
    pub subject: String,
    pub author: String,
    pub email: String,
    pub authored: String,
    pub committer: String,
    pub committer_email: String,
    pub committed: String,
    pub decorations: String,
    pub body: String,
    pub metadata_truncated: bool,
    pub graph: String,
}
impl Commit {
    pub fn matches(&self, query: &str) -> bool {
        [
            &self.oid,
            &self.subject,
            &self.author,
            &self.email,
            &self.body,
            &self.decorations,
            &self.authored,
            &self.committed,
            &self.committer,
            &self.committer_email,
        ]
        .iter()
        .any(|value| value.to_lowercase().contains(query))
    }
    pub fn time_label(&self) -> String {
        let Ok(time) = chrono::DateTime::parse_from_rfc3339(&self.committed) else {
            return self.committed.clone();
        };
        let seconds = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - time.timestamp())
        .max(0);
        let relative = if seconds < 60 {
            "just now".into()
        } else if seconds < 3600 {
            format!("{}m ago", seconds / 60)
        } else if seconds < 86400 {
            format!("{}h ago", seconds / 3600)
        } else {
            format!("{}d ago", seconds / 86400)
        };
        format!("{relative} · {}", time.format("%Y-%m-%d %H:%M %:z"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct HistoryFile {
    pub change: Change,
    pub old_mode: String,
    pub new_mode: String,
    pub old_oid: String,
    pub new_oid: String,
}

pub(super) struct Page {
    pub commits: Vec<Commit>,
    pub more: bool,
    pub lanes: Vec<String>,
}

pub(super) fn valid_oid(oid: &str) -> bool {
    matches!(oid.len(), 40 | 64) && oid.bytes().all(|b| b.is_ascii_hexdigit())
}

pub(super) fn load_page(
    root: &Path,
    tip: &str,
    skip: usize,
    mut lanes: Vec<String>,
    cancelled: &AtomicBool,
) -> Result<Page, String> {
    if !valid_oid(tip) {
        return Err("Invalid history commit id".into());
    }
    let bytes = read(
        root,
        vec![
            "log".into(),
            "--topo-order".into(),
            "--no-show-signature".into(),
            "--no-notes".into(),
            "--no-color".into(),
            "--decorate=short".into(),
            "-z".into(),
            FORMAT.into(),
            format!("--max-count={}", PAGE_SIZE + 1).into(),
            format!("--skip={skip}").into(),
            tip.into(),
            "--".into(),
        ],
        cancelled,
    )?;
    let mut commits = parse_commits(&bytes)?;
    let more = commits.len() > PAGE_SIZE;
    commits.truncate(PAGE_SIZE);
    for commit in &mut commits {
        // Active lanes point at the next unvisited ancestor. Reuse a lane for
        // the first parent, introduce the others, and converge duplicate lanes.
        let lane = lanes
            .iter()
            .position(|oid| oid == &commit.oid)
            .unwrap_or_else(|| {
                lanes.push(commit.oid.clone());
                lanes.len() - 1
            });
        commit.graph = (0..lanes.len())
            .map(|i| if i == lane { "● " } else { "│ " })
            .collect::<String>();
        if commit.parents.len() > 1 {
            commit.graph.push('╲');
        }
        lanes.remove(lane);
        for (offset, parent) in commit.parents.iter().enumerate() {
            if !lanes.contains(parent) {
                lanes.insert((lane + offset).min(lanes.len()), parent.clone());
            }
        }
    }
    Ok(Page {
        commits,
        more,
        lanes,
    })
}

fn parse_commits(bytes: &[u8]) -> Result<Vec<Commit>, String> {
    let mut fields: Vec<_> = bytes.split(|b| *b == 0).collect();
    if fields.last() == Some(&&b""[..]) {
        fields.pop();
    }
    if !fields.len().is_multiple_of(11) {
        return Err("Could not parse commit metadata".into());
    }
    fields
        .chunks(11)
        .map(|fields| {
            let mut values: Vec<_> = fields
                .iter()
                .map(|value| String::from_utf8_lossy(value).into_owned())
                .collect();
            if !valid_oid(&values[0]) {
                return Err("Invalid commit id in history".into());
            }
            let parents: Vec<String> = values[1].split_whitespace().map(Into::into).collect();
            if parents.iter().any(|oid| !valid_oid(oid)) {
                return Err("Invalid parent id in history".into());
            }
            let mut metadata_truncated = false;
            for (index, value) in values.iter_mut().enumerate().skip(2) {
                let cap = if index == 10 { 64 * 1024 } else { 4 * 1024 };
                if value.len() > cap {
                    let mut boundary = cap;
                    while !value.is_char_boundary(boundary) {
                        boundary -= 1;
                    }
                    value.truncate(boundary);
                    metadata_truncated = true;
                }
            }
            Ok(Commit {
                oid: values[0].clone(),
                parents,
                subject: values[2].clone(),
                author: values[3].clone(),
                email: values[4].clone(),
                authored: values[5].clone(),
                committer: values[6].clone(),
                committer_email: values[7].clone(),
                committed: values[8].clone(),
                decorations: values[9].clone(),
                body: values[10].clone(),
                metadata_truncated,
                graph: String::new(),
            })
        })
        .collect()
}

pub(super) fn load_files(
    root: &Path,
    commit: &Commit,
    cancelled: &AtomicBool,
) -> Result<Vec<HistoryFile>, String> {
    if !valid_oid(&commit.oid) || commit.parents.iter().any(|oid| !valid_oid(oid)) {
        return Err("Invalid commit id".into());
    }
    let mut args: Vec<OsString> = [
        "diff-tree",
        "--no-ext-diff",
        "--no-textconv",
        "--no-commit-id",
        "--raw",
        "--no-abbrev",
        "-z",
        "-r",
        "-M",
        "--root",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    if let Some(parent) = commit.parents.first() {
        args.push(parent.into());
    }
    args.extend([commit.oid.as_str(), "--"].map(Into::into));
    parse_files(&read(root, args, cancelled)?)
}

fn parse_files(bytes: &[u8]) -> Result<Vec<HistoryFile>, String> {
    let mut records = bytes.split(|b| *b == 0).peekable();
    let mut files = Vec::new();
    while let Some(record) = records.next() {
        if record.is_empty() {
            continue;
        }
        let metadata = std::str::from_utf8(record).map_err(|_| "Invalid history file metadata")?;
        let fields: Vec<_> = metadata
            .trim_start_matches(':')
            .split_whitespace()
            .collect();
        if fields.len() != 5 {
            return Err("Could not parse history files".into());
        }
        let path = records
            .next()
            .filter(|path| !path.is_empty())
            .ok_or("Missing history path")?;
        let status = fields[4].as_bytes()[0];
        let (path, original_path) = if matches!(status, b'R' | b'C') {
            (
                records
                    .next()
                    .filter(|path| !path.is_empty())
                    .ok_or("Missing rename path")?,
                Some(raw_path(path)),
            )
        } else {
            (path, None)
        };
        files.push(HistoryFile {
            change: Change {
                path: raw_path(path),
                original_path,
                kind: ChangeKind::from_status(status).unwrap_or(ChangeKind::Unknown),
                index_mode: None,
                index_oid: None,
            },
            old_mode: fields[0].into(),
            new_mode: fields[1].into(),
            old_oid: fields[2].into(),
            new_oid: fields[3].into(),
        });
    }
    Ok(files)
}

fn raw_path(bytes: &[u8]) -> std::path::PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt as _;
        OsString::from_vec(bytes.to_vec()).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(bytes).into_owned().into()
    }
}

fn read(root: &Path, args: Vec<OsString>, cancelled: &AtomicBool) -> Result<Vec<u8>, String> {
    let output = run_git_cancellable(root, args, MAX_HISTORY_BYTES, cancelled).map_err(
        |error| match error {
            LoadError::Failed(message) => message,
            LoadError::GitUnavailable => "Git is unavailable".into(),
            LoadError::NotRepository => "Not a Git repository".into(),
        },
    )?;
    if !output.status.success() {
        return Err(stderr_message(&output));
    }
    if output.stdout_truncated {
        return Err(
            "History metadata exceeds the 8 MiB limit. Select a smaller history scope.".into(),
        );
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, process::Command, sync::atomic::Ordering};

    fn git(root: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().into()
    }
    fn setup() -> tempfile::TempDir {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init", "-b", "main"]);
        git(temp.path(), &["config", "user.name", "History Author"]);
        git(
            temp.path(),
            &["config", "user.email", "history@example.com"],
        );
        git(temp.path(), &["config", "commit.gpgsign", "false"]);
        temp
    }
    fn commit(root: &Path, path: &str, text: &str, message: &str) {
        fs::write(root.join(path), text).unwrap();
        git(root, &["add", "--", path]);
        git(root, &["commit", "-m", message]);
    }
    fn page(root: &Path, tip: &str, skip: usize, lanes: Vec<String>) -> Page {
        load_page(root, tip, skip, lanes, &AtomicBool::new(false)).unwrap()
    }

    #[test]
    fn pages_are_bounded_and_pinned_when_head_moves() {
        let temp = setup();
        let root = temp.path();
        // Import the entire bounded fixture in one Git process.
        use std::io::Write as _;
        use std::process::Stdio;
        let mut stream = String::new();
        for i in 0..(PAGE_SIZE + 4) {
            let message = format!("commit {i}");
            stream.push_str(&format!("commit refs/heads/main\nmark :{}\nauthor History Author <history@example.com> 1700000000 +0000\ncommitter History Author <history@example.com> 1700000000 +0000\ndata {}\n{}\n", i + 1, message.len(), message));
            if i > 0 {
                stream.push_str(&format!("from :{i}\n"));
            }
            stream.push('\n');
        }
        let mut child = Command::new("git")
            .args(["fast-import", "--quiet"])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(stream.as_bytes())
            .unwrap();
        assert!(child.wait_with_output().unwrap().status.success());
        let tip = git(root, &["rev-parse", "HEAD"]);
        let tree = git(root, &["rev-parse", "HEAD^{tree}"]);
        git(root, &["-c", "tag.gpgSign=false", "tag", "history-tag"]);
        let first = page(root, &tip, 0, vec![]);
        assert_eq!(first.commits.len(), PAGE_SIZE);
        assert!(first.more);
        assert!(first.commits[0].decorations.contains("history-tag"));
        assert!(first.commits[0].matches("history author"));
        assert!(first.commits[0].matches("history@example.com"));
        let moved = git(root, &["commit-tree", &tree, "-p", &tip, "-m", "new head"]);
        git(root, &["update-ref", "refs/heads/main", &moved]);
        let second = page(root, &tip, PAGE_SIZE, first.lanes);
        assert_eq!(second.commits.len(), 4);
        assert!(!second.more);
        assert!(second.commits.last().unwrap().parents.is_empty());
        assert!(second.lanes.is_empty());
        assert!(
            !first
                .commits
                .iter()
                .any(|a| second.commits.iter().any(|b| a.oid == b.oid))
        );
        assert!(second.commits.iter().all(|c| c.graph == "● "));
    }

    #[test]
    fn root_merge_rename_and_deletion_use_exact_tree_objects() {
        let temp = setup();
        let root = temp.path();
        commit(
            root,
            "a file.txt",
            "root\n",
            "root subject\n\nFull body\nSecond line",
        );
        let root_oid = git(root, &["rev-parse", "HEAD"]);
        let root_commit = page(root, &root_oid, 0, vec![]).commits.remove(0);
        assert!(root_commit.body.contains("Full body\nSecond line"));
        let files = load_files(root, &root_commit, &AtomicBool::new(false)).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].old_mode, "000000");
        assert_eq!(files[0].change.kind, ChangeKind::Added);
        assert!(
            crate::git::diff::load_history_diff(root, &files[0], &AtomicBool::new(false)).is_ok()
        );
        git(root, &["switch", "-c", "topic"]);
        commit(root, "topic.txt", "topic\n", "topic commit");
        git(root, &["switch", "main"]);
        git(root, &["mv", "a file.txt", "renamed file.txt"]);
        git(root, &["commit", "-m", "rename"]);
        let renamed = page(root, &git(root, &["rev-parse", "HEAD"]), 0, vec![])
            .commits
            .remove(0);
        let files = load_files(root, &renamed, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            files[0].change.original_path.as_deref(),
            Some(Path::new("a file.txt"))
        );
        assert_eq!(files[0].change.path, Path::new("renamed file.txt"));
        git(root, &["merge", "--no-ff", "topic", "-m", "merge topic"]);
        let history = page(root, &git(root, &["rev-parse", "HEAD"]), 0, vec![]);
        let merge = &history.commits[0];
        assert_eq!(merge.parents.len(), 2);
        assert!(merge.graph.contains('╲'));
        assert!(history.commits.iter().any(|c| c.graph.contains('│')));
        let files = load_files(root, merge, &AtomicBool::new(false)).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].change.path, Path::new("topic.txt"));
        git(root, &["rm", "renamed file.txt"]);
        git(root, &["commit", "-m", "delete"]);
        let deleted = page(root, &git(root, &["rev-parse", "HEAD"]), 0, vec![])
            .commits
            .remove(0);
        let files = load_files(root, &deleted, &AtomicBool::new(false)).unwrap();
        assert_eq!(files[0].new_mode, "000000");
        assert!(
            crate::git::diff::load_history_diff(root, &files[0], &AtomicBool::new(false)).is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn raw_paths_and_binary_history_remain_usable() {
        use std::os::unix::ffi::OsStringExt as _;
        let raw = parse_files(b":000000 100644 0000000000000000000000000000000000000000 1111111111111111111111111111111111111111 A\0odd\xff\nname\0").unwrap();
        assert_eq!(
            raw[0].change.path.as_os_str(),
            OsString::from_vec(b"odd\xff\nname".to_vec())
        );
        let temp = setup();
        let root = temp.path();
        let path = OsString::from("a file.bin");
        fs::write(root.join(&path), [0, 1, 2]).unwrap();
        git(root, &["add", "-A"]);
        git(root, &["commit", "-m", "binary"]);
        let commit = page(root, &git(root, &["rev-parse", "HEAD"]), 0, vec![])
            .commits
            .remove(0);
        let files = load_files(root, &commit, &AtomicBool::new(false)).unwrap();
        assert_eq!(files[0].change.path.as_os_str(), path);
        assert!(
            crate::git::diff::load_history_diff(root, &files[0], &AtomicBool::new(false)).is_ok()
        );
    }

    #[test]
    fn long_commit_metadata_keeps_the_page_accessible() {
        let oid = "1".repeat(40);
        let record = format!(
            "{oid}\0\0subject\0author\0email\0date\0committer\0email\0date\0main\0{}\0",
            "🦀".repeat(20000)
        );
        let commits = parse_commits(format!("{record}{record}").as_bytes()).unwrap();
        assert_eq!(commits.len(), 2);
        assert!(commits[0].metadata_truncated);
        assert!(commits[0].body.len() <= 64 * 1024);
    }

    #[test]
    fn cancelled_reads_and_invalid_oids_are_rejected() {
        let temp = setup();
        let root = temp.path();
        commit(root, "file", "content", "initial");
        let tip = git(root, &["rev-parse", "HEAD"]);
        let cancel = AtomicBool::new(true);
        let error = load_page(root, &tip, 0, vec![], &cancel).err().unwrap();
        assert!(error.contains("cancelled"), "{error}");
        cancel.store(false, Ordering::Relaxed);
        assert!(load_page(root, "--all", 0, vec![], &cancel).is_err());
        assert!(parse_commits(b"bad\0metadata").is_err());
        assert!(parse_files(b":100644 100644\0path\0").is_err());
    }
}
