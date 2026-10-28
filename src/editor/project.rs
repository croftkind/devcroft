//! Bounded checkout discovery for the built-in editor. The same index feeds
//! the file tree, fuzzy opener, and text search. Git-ignored files stay
//! visible in the tree and finder, but project text search skips them so
//! build output and local configuration are never read in bulk.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _};
use std::{
    collections::HashSet,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

const MAX_FILES: usize = 100_000;
const MAX_IGNORED_FILES: usize = 10_000;
const MAX_IGNORED_DIR_FILES: usize = 1_000;
const MAX_SEARCH_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RESULTS: usize = 200;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ProjectFile {
    pub path: PathBuf,
    pub label: String,
    /// Matched by `.gitignore`, a global exclude, or `info/exclude`. Kept in
    /// the tree and finder, but skipped by project text search.
    pub ignored: bool,
}

pub(super) fn is_git_component(part: &std::ffi::OsStr) -> bool {
    part.to_string_lossy()
        .trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(".git")
}

pub(super) struct FileChanges {
    watcher: Option<RecommendedWatcher>,
    changed: Arc<AtomicBool>,
}

impl FileChanges {
    pub(super) fn new(root: &Path) -> Self {
        let changed = Arc::new(AtomicBool::new(false));
        let pending = changed.clone();
        let checkout = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        let git_dirs = match gix::discover(&checkout) {
            Ok(repo) => vec![repo.git_dir().to_owned(), repo.common_dir().to_owned()],
            Err(_) => Vec::new(),
        };
        let exclude_files: Vec<PathBuf> = git_dirs
            .iter()
            .map(|dir| dir.join("info/exclude"))
            .collect();
        let watched_checkout = checkout.clone();
        let watched_git_dirs = git_dirs.clone();
        let watched_exclude_files = exclude_files.clone();
        let watcher = (|| -> notify::Result<RecommendedWatcher> {
            let mut watcher = notify::recommended_watcher(move |event: notify::Result<Event>| {
                if event.map_or(true, |event| {
                    affects_project(
                        &watched_checkout,
                        &watched_git_dirs,
                        &watched_exclude_files,
                        &event,
                    )
                }) {
                    pending.store(true, Ordering::Release);
                }
            })?;
            watcher.watch(&checkout, RecursiveMode::Recursive)?;
            for git_dir in &git_dirs {
                if !git_dir.starts_with(&checkout) {
                    let info_dir = git_dir.join("info");
                    if info_dir.is_dir() {
                        let _ = watcher.watch(&info_dir, RecursiveMode::NonRecursive);
                    }
                }
            }
            Ok(watcher)
        })()
        .ok();
        Self { watcher, changed }
    }

    pub(super) fn take_changed(&self) -> bool {
        // Fall back to polling when the platform cannot watch this checkout.
        self.watcher.is_none() || self.changed.swap(false, Ordering::AcqRel)
    }

    pub(super) fn mark_changed(&self) {
        self.changed.store(true, Ordering::Release);
    }

    #[cfg(test)]
    pub(super) fn is_watching(&self) -> bool {
        self.watcher.is_some()
    }
}

fn affects_project(
    checkout: &Path,
    git_dirs: &[PathBuf],
    exclude_files: &[PathBuf],
    event: &Event,
) -> bool {
    if matches!(event.kind, EventKind::Access(_)) {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    event.paths.iter().any(|path| {
        // `info/exclude` may live in a git directory outside the checkout.
        if exclude_files.iter().any(|exclude| path == exclude) {
            return true;
        }
        if git_dirs.iter().any(|dir| path.starts_with(dir)) {
            return false;
        }
        if let Ok(relative) = path.strip_prefix(checkout)
            && relative
                .components()
                .any(|component| is_git_component(component.as_os_str()))
        {
            return false;
        }
        true
    })
}

#[derive(Clone, Debug)]
pub(super) struct TextMatch {
    pub path: PathBuf,
    pub label: String,
    pub line: usize,
    pub column: usize,
    pub preview: String,
}

#[derive(Default)]
pub(super) struct TextMatches {
    pub items: Vec<TextMatch>,
    pub truncated: bool,
    pub error: Option<String>,
}

struct LiteralMatcher(regex::bytes::Regex);

impl fff_grep::Matcher for LiteralMatcher {
    type Error = fff_grep::NoError;

    fn find_at(&self, bytes: &[u8], at: usize) -> Result<Option<fff_grep::Match>, Self::Error> {
        Ok(self
            .0
            .find_at(bytes, at)
            .map(|m| fff_grep::Match::new(m.start(), m.end())))
    }

    fn line_terminator(&self) -> Option<fff_grep::LineTerminator> {
        Some(fff_grep::LineTerminator::byte(b'\n'))
    }
}

struct TextSink<'a, F> {
    file: &'a ProjectFile,
    out: &'a mut TextMatches,
    cancelled: &'a F,
    matcher: &'a LiteralMatcher,
}

impl<F: Fn() -> bool> fff_grep::Sink for TextSink<'_, F> {
    type Error = std::io::Error;

    fn matched(
        &mut self,
        _: &fff_grep::Searcher,
        m: &fff_grep::SinkMatch<'_>,
    ) -> Result<bool, Self::Error> {
        if (self.cancelled)() {
            return Ok(false);
        }
        if self.out.items.len() == MAX_RESULTS {
            self.out.truncated = true;
            return Ok(false);
        }
        self.out.items.push(TextMatch {
            path: self.file.path.clone(),
            label: self.file.label.clone(),
            line: m.line_number().unwrap_or(1) as usize,
            column: self.matcher.0.find(m.bytes()).map_or(0, |m| m.start()),
            preview: String::from_utf8_lossy(m.bytes())
                .trim()
                .chars()
                .take(160)
                .collect(),
        });
        Ok(true)
    }
}

/// `ignore` applies .gitignore, global excludes, and parent rules. Hidden
/// source files remain visible; repository metadata and nested checkouts do
/// not enter the index. Symlinks are deliberately not followed.
///
/// The tree and finder also include ignored files: files matched directly,
/// files inside visible folders, and small ignored folders. Huge ignored
/// folders (build output, dependencies) are left out so they cannot crowd
/// real source files out of `MAX_FILES` or make every refresh reindex them.
/// Search filters ignored entries out again.
pub(super) fn scan(root: &Path) -> Vec<ProjectFile> {
    let Ok(root) = root.canonicalize() else {
        return Vec::new();
    };
    let root = root.as_path();
    let mut files = collect_visible(root);
    if files.len() < MAX_FILES {
        let visible_files: HashSet<PathBuf> = files.iter().map(|file| file.path.clone()).collect();
        let visible_dirs = visible_dirs(root, &files);
        let (ignored, ignored_dirs) =
            collect_ignored_in_visible_dirs(root, &visible_files, visible_dirs, MAX_IGNORED_FILES);
        let mut budget = MAX_IGNORED_FILES.min(MAX_FILES - files.len());
        for file in ignored {
            if budget == 0 {
                break;
            }
            files.push(file);
            budget -= 1;
        }
        let mut attempts = MAX_IGNORED_FILES;
        for dir in ignored_dirs {
            if budget == 0 || attempts == 0 {
                break;
            }
            let limit = MAX_IGNORED_DIR_FILES.min(budget).min(attempts);
            match collect_ignored_dir(root, &dir, limit) {
                Some(dir_files) => {
                    budget -= dir_files.len();
                    files.extend(dir_files);
                }
                None => attempts = attempts.saturating_sub(limit + 1),
            }
        }
    }
    files.sort_unstable_by(|a, b| a.label.cmp(&b.label));
    files
}

fn collect_visible(root: &Path) -> Vec<ProjectFile> {
    let filter_root = root.to_owned();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(true)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .parents(true)
        .require_git(false)
        .filter_entry(move |entry| {
            if is_git_component(entry.file_name()) {
                return false;
            }
            !(entry.file_type().is_some_and(|kind| kind.is_dir())
                && entry.path() != filter_root
                && entry.path().join(".git").exists())
        })
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        if files.len() >= MAX_FILES {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.push(ProjectFile {
            path: entry.path().to_owned(),
            label: relative_label(relative),
            ignored: false,
        });
    }
    files
}

/// Every directory that contains a visible file, plus the checkout root.
fn visible_dirs(root: &Path, files: &[ProjectFile]) -> HashSet<PathBuf> {
    let mut dirs = HashSet::from([root.to_owned()]);
    for file in files {
        let mut parent = file.path.parent();
        while let Some(dir) = parent {
            if dir == root || !dir.starts_with(root) || !dirs.insert(dir.to_owned()) {
                break;
            }
            parent = dir.parent();
        }
    }
    dirs
}

/// Walk only visible directories to find ignored files and the topmost ignored
/// directory on each branch. Ignored directories are not descended into here;
/// `scan` revisits the small ones with a bounded walk.
fn collect_ignored_in_visible_dirs(
    root: &Path,
    visible_files: &HashSet<PathBuf>,
    visible_dirs: HashSet<PathBuf>,
    limit: usize,
) -> (Vec<ProjectFile>, Vec<PathBuf>) {
    let candidates = Arc::new(std::sync::Mutex::new(Vec::new()));
    let discovered = Arc::clone(&candidates);
    let filter_root = root.to_owned();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        .filter_entry(move |entry| {
            if is_git_component(entry.file_name()) {
                return false;
            }
            if !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                return true;
            }
            if entry.path() == filter_root {
                return true;
            }
            if entry.path().join(".git").exists() {
                return false;
            }
            if !visible_dirs.contains(entry.path()) {
                discovered
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(entry.path().to_owned());
                return false;
            }
            true
        })
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        if files.len() >= limit {
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || visible_files.contains(entry.path())
        {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.push(ProjectFile {
            path: entry.path().to_owned(),
            label: relative_label(relative),
            ignored: true,
        });
    }
    let candidates = candidates
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    (files, candidates)
}

/// Collect an ignored directory only when it fits in `limit` files. A huge
/// directory is abandoned early instead of filling the index with build output.
fn collect_ignored_dir(root: &Path, dir: &Path, limit: usize) -> Option<Vec<ProjectFile>> {
    let filter_root = dir.to_owned();
    let walker = ignore::WalkBuilder::new(dir)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        .filter_entry(move |entry| {
            if is_git_component(entry.file_name()) {
                return false;
            }
            !(entry.file_type().is_some_and(|kind| kind.is_dir())
                && entry.path() != filter_root
                && entry.path().join(".git").exists())
        })
        .build();
    let mut files = Vec::new();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else {
            continue;
        };
        files.push(ProjectFile {
            path: entry.path().to_owned(),
            label: relative_label(relative),
            ignored: true,
        });
        if files.len() > limit {
            return None;
        }
    }
    Some(files)
}

fn relative_label(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Search only indexed files and cap memory, per-file I/O, and result count.
/// Results are line based so choosing one can open at that location.
pub(super) fn search_text(
    root: &Path,
    files: &[ProjectFile],
    query: &str,
    cancelled: impl Fn() -> bool,
) -> Option<TextMatches> {
    if query.is_empty() {
        return Some(TextMatches::default());
    }
    let Ok(root) = root.canonicalize() else {
        return Some(TextMatches::default());
    };
    // Literal, Unicode-aware case-insensitive matching. fff's slice grep
    // engine runs over our bounded reads instead of reopening checkout paths.
    let matcher = match regex::bytes::RegexBuilder::new(&regex::escape(query))
        .case_insensitive(true)
        .build()
    {
        Ok(regex) => LiteralMatcher(regex),
        Err(error) => {
            return Some(TextMatches {
                error: Some(format!("Search unavailable: {error}")),
                ..Default::default()
            });
        }
    };
    let searcher = fff_grep::SearcherBuilder::new().line_number(true).build();
    let mut out = TextMatches::default();
    for file in files {
        if file.ignored {
            continue;
        }
        if cancelled() {
            return None;
        }
        if out.truncated {
            break;
        }
        let Ok(bytes) = read_indexed_file(&root, file) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(_) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let _ = searcher.search_slice(
            &matcher,
            &bytes,
            TextSink {
                file,
                out: &mut out,
                cancelled: &cancelled,
                matcher: &matcher,
            },
        );
    }
    if cancelled() { None } else { Some(out) }
}

/// Reopen every component relative to the checkout so a path swapped to a
/// symlink after indexing cannot make project search read outside files.
#[cfg(unix)]
pub(super) fn read_indexed_file(root: &Path, entry: &ProjectFile) -> std::io::Result<Vec<u8>> {
    use std::os::unix::{ffi::OsStrExt as _, fs::OpenOptionsExt as _};
    use std::{
        ffi::CString,
        os::fd::{AsRawFd as _, FromRawFd as _},
    };

    // Retain original OS path bytes, including non-UTF-8 filenames. The
    // display label is lossy and must never be used to reopen a file.
    // Normalize an alias root only when needed to align it with the scan's
    // canonical paths. A replaced canonical root still goes through the
    // O_NOFOLLOW open below instead of resolving a new symlink target.
    let canonical;
    let (root, relative) = match entry.path.strip_prefix(root) {
        Ok(relative) => (root, relative),
        Err(_) => {
            canonical = root.canonicalize()?;
            let relative = entry
                .path
                .strip_prefix(&canonical)
                .map_err(|_| std::io::Error::other("indexed path escaped checkout"))?;
            (canonical.as_path(), relative)
        }
    };
    let mut directory = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        let std::path::Component::Normal(part) = part else {
            return Err(std::io::Error::other("invalid indexed path"));
        };
        let part = CString::new(part.as_bytes())
            .map_err(|_| std::io::Error::other("invalid indexed path"))?;
        let flags = libc::O_RDONLY
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | if parts.peek().is_some() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: the parent descriptor is live and the component is NUL terminated.
        let descriptor = unsafe { libc::openat(directory.as_raw_fd(), part.as_ptr(), flags) };
        if descriptor < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        let opened = unsafe { fs::File::from_raw_fd(descriptor) };
        if parts.peek().is_some() {
            directory = opened;
            continue;
        }
        let metadata = opened.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_SEARCH_BYTES {
            return Err(std::io::Error::other("not a searchable file"));
        }
        let mut bytes = Vec::new();
        opened.take(MAX_SEARCH_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_SEARCH_BYTES {
            return Err(std::io::Error::other("file grew beyond search limit"));
        }
        return Ok(bytes);
    }
    Err(std::io::Error::other("empty indexed path"))
}

#[cfg(windows)]
pub(super) fn read_indexed_file(root: &Path, entry: &ProjectFile) -> std::io::Result<Vec<u8>> {
    use std::os::windows::{ffi::OsStringExt as _, io::AsRawHandle as _};
    use windows_sys::Win32::Storage::FileSystem::GetFinalPathNameByHandleW;

    let file = fs::File::open(&entry.path)?;
    // Check the opened handle, rather than a pathname that could be swapped
    // between canonicalization and open.
    let mut name = vec![0u16; 32_768];
    // SAFETY: the file handle is open and `name` provides its stated capacity.
    let length = unsafe {
        GetFinalPathNameByHandleW(
            file.as_raw_handle(),
            name.as_mut_ptr(),
            name.len() as u32,
            0,
        )
    } as usize;
    if length == 0 || length >= name.len() {
        return Err(std::io::Error::last_os_error());
    }
    let opened_path = PathBuf::from(std::ffi::OsString::from_wide(&name[..length]));
    if !opened_path.starts_with(root) {
        return Err(std::io::Error::other("indexed path escaped checkout"));
    }
    if !file.metadata()?.is_file() {
        return Err(std::io::Error::other("not a searchable file"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_SEARCH_BYTES + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_SEARCH_BYTES {
        return Err(std::io::Error::other("file grew beyond search limit"));
    }
    Ok(bytes)
}

#[cfg(not(any(unix, windows)))]
pub(super) fn read_indexed_file(_root: &Path, _entry: &ProjectFile) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other(
        "project search is unsupported on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_includes_ignored_files_but_search_skips_them() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(".gitignore"),
            "ignored.txt\nbuild/\n*.log\n",
        )
        .unwrap();
        fs::write(dir.path().join("visible.rs"), "let needle = 1;\n").unwrap();
        fs::write(dir.path().join("ignored.txt"), "needle\n").unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested/child.rs"), "child\n").unwrap();
        fs::write(dir.path().join("nested/local.log"), "needle\n").unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        fs::write(dir.path().join("build/generated.rs"), "needle\n").unwrap();
        let files = scan(dir.path());
        assert!(files.iter().any(|file| file.label == "visible.rs"));
        assert!(files.iter().any(|file| file.label == "nested/child.rs"));
        for ignored in ["ignored.txt", "nested/local.log", "build/generated.rs"] {
            assert!(
                files
                    .iter()
                    .find(|file| file.label == ignored)
                    .is_some_and(|file| file.ignored),
                "expected {ignored} to be indexed as ignored"
            );
        }
        assert!(files.iter().filter(|file| !file.ignored).all(|file| {
            !["ignored.txt", "nested/local.log", "build/generated.rs"]
                .contains(&file.label.as_str())
        }));
        let matches = search_text(dir.path(), &files, "needle", || false)
            .unwrap()
            .items;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].line, 1);
    }

    #[test]
    fn large_ignored_directories_stay_out_of_the_index() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "build/\n.env\n").unwrap();
        fs::write(dir.path().join(".env"), "TOKEN=dev\n").unwrap();
        fs::write(dir.path().join("visible.rs"), "fn visible() {}\n").unwrap();
        fs::create_dir(dir.path().join("build")).unwrap();
        for index in 0..=MAX_IGNORED_DIR_FILES {
            fs::write(
                dir.path()
                    .join("build")
                    .join(format!("generated-{index}.rs")),
                "fn generated() {}\n",
            )
            .unwrap();
        }
        let files = scan(dir.path());
        assert!(
            files
                .iter()
                .find(|file| file.label == ".env")
                .is_some_and(|file| file.ignored)
        );
        assert!(files.iter().any(|file| file.label == "visible.rs"));
        assert!(
            !files.iter().any(|file| file.label.starts_with("build/")),
            "large ignored directories must not flood the index"
        );
    }

    #[test]
    fn fff_grep_matches_literal_unicode_text_and_preserves_line_locations() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("a.txt"),
            "before\r\n  CAFÉ [x].*\r\nlast [X].*",
        )
        .unwrap();
        fs::write(dir.path().join("b.txt"), "café [x].*\n").unwrap();
        fs::write(dir.path().join("binary.txt"), b"[x].*\0").unwrap();
        fs::write(dir.path().join("invalid.txt"), b"[x].*\xff").unwrap();
        fs::write(dir.path().join("huge.txt"), "[x].*".repeat(500_000)).unwrap();
        let files = scan(dir.path());
        let found = search_text(dir.path(), &files, "[x].*", || false).unwrap();
        assert_eq!(
            found
                .items
                .iter()
                .map(|m| (m.label.as_str(), m.line))
                .collect::<Vec<_>>(),
            vec![("a.txt", 2), ("a.txt", 3), ("b.txt", 1)]
        );
        assert_eq!(found.items[0].preview, "CAFÉ [x].*");
        assert_eq!(
            search_text(dir.path(), &files, "café", || false)
                .unwrap()
                .items
                .len(),
            2
        );
        assert!(
            search_text(dir.path(), &files, "", || false)
                .unwrap()
                .items
                .is_empty()
        );
        assert!(search_text(dir.path(), &files, "[x].*", || true).is_none());
    }

    #[test]
    fn fff_grep_caps_matching_lines_and_reports_more_results() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("many.txt");
        fs::write(&path, "needle\n".repeat(MAX_RESULTS)).unwrap();
        let files = scan(dir.path());
        let exact = search_text(dir.path(), &files, "needle", || false).unwrap();
        assert_eq!(exact.items.len(), MAX_RESULTS);
        assert!(!exact.truncated);
        fs::write(path, "needle\n".repeat(MAX_RESULTS + 1)).unwrap();
        let extra = search_text(dir.path(), &files, "needle", || false).unwrap();
        assert_eq!(extra.items.len(), MAX_RESULTS);
        assert!(extra.truncated);
    }

    // macOS filesystems reject filenames containing invalid UTF-8.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn grep_reads_original_non_utf8_filenames() {
        use std::os::unix::ffi::OsStringExt;
        let dir = tempfile::tempdir().unwrap();
        let name = std::ffi::OsString::from_vec(b"bad-\xff.txt".to_vec());
        fs::write(dir.path().join(&name), "needle\n").unwrap();
        let files = scan(dir.path());
        let found = search_text(dir.path(), &files, "needle", || false).unwrap();
        assert_eq!(found.items.len(), 1);
        assert_eq!(found.items[0].path.file_name(), Some(name.as_os_str()));
    }

    #[cfg(unix)]
    #[test]
    fn grep_rejects_indexed_parent_directory_replaced_by_a_symlink() {
        use std::os::unix::fs::symlink;
        let checkout = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir(checkout.path().join("src")).unwrap();
        fs::write(checkout.path().join("src/file.txt"), "safe\n").unwrap();
        let files = scan(checkout.path());
        fs::write(outside.path().join("file.txt"), "outside-secret\n").unwrap();
        fs::remove_file(checkout.path().join("src/file.txt")).unwrap();
        fs::remove_dir(checkout.path().join("src")).unwrap();
        symlink(outside.path(), checkout.path().join("src")).unwrap();
        assert!(
            search_text(checkout.path(), &files, "outside-secret", || false)
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn search_skips_indexed_file_replaced_with_outside_symlink() {
        use std::os::unix::fs::symlink;
        let checkout = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let indexed = checkout.path().join("indexed.txt");
        fs::write(&indexed, "safe\n").unwrap();
        let files = scan(checkout.path());
        fs::write(outside.path().join("secret.txt"), "outside-secret\n").unwrap();
        fs::remove_file(&indexed).unwrap();
        symlink(outside.path().join("secret.txt"), &indexed).unwrap();
        assert!(
            search_text(checkout.path(), &files, "outside-secret", || false)
                .unwrap()
                .items
                .is_empty()
        );
    }

    #[test]
    fn project_watch_tracks_worktree_and_ignored_entries_but_skips_reads_and_git_internals() {
        use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("target/debug")).unwrap();
        let git_dirs = [root.join(".git")];
        let exclude_files = [root.join(".git/info/exclude")];
        for access_path in ["src/main.rs", ".gitignore", ".git/info/exclude"] {
            assert!(!affects_project(
                root,
                &git_dirs,
                &exclude_files,
                &Event::new(EventKind::Access(AccessKind::Open(AccessMode::Any)))
                    .add_path(root.join(access_path))
            ));
        }
        for ignored in [".git/index", ".GIT/config"] {
            assert!(
                !affects_project(
                    root,
                    &git_dirs,
                    &exclude_files,
                    &Event::new(EventKind::Modify(ModifyKind::Any)).add_path(root.join(ignored))
                ),
                "expected {ignored} to be ignored by watcher"
            );
        }
        for tracked in [
            "ignored.txt",
            "target",
            "target/debug/deps/app.d",
            "src/keep.log",
            "src/new.rs",
            ".gitignore",
            ".git/info/exclude",
        ] {
            assert!(
                affects_project(
                    root,
                    &git_dirs,
                    &exclude_files,
                    &Event::new(EventKind::Create(CreateKind::Any)).add_path(root.join(tracked))
                ),
                "expected {tracked} to trigger project refresh"
            );
        }
    }
}
