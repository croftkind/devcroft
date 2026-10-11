use super::super::conflicts::{MAX_TEXT_BYTES, Source};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Draft {
    pub source: Source,
    pub text: String,
}
#[derive(Default)]
pub(super) struct Writer {
    revision: u64,
}
pub(super) type SharedWriter = Arc<Mutex<Writer>>;
static WRITERS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<Writer>>>>> = OnceLock::new();
pub(super) fn writer(path: &Path) -> SharedWriter {
    let mut map = WRITERS
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    map.retain(|_, v| v.strong_count() > 0);
    if let Some(writer) = map.get(path).and_then(Weak::upgrade) {
        return writer;
    }
    let writer = Arc::new(Mutex::new(Writer::default()));
    map.insert(path.to_owned(), Arc::downgrade(&writer));
    writer
}
fn directory(data_root: &Path, root: &Path) -> PathBuf {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    data_root.join("git-conflict-drafts").join(format!(
        "{:x}",
        Sha256::digest(root.as_os_str().as_encoded_bytes())
    ))
}
pub(super) fn path(data_root: &Path, root: &Path, path: &Path) -> PathBuf {
    directory(data_root, root).join(format!(
        "{:x}.json",
        Sha256::digest(path.as_os_str().as_encoded_bytes())
    ))
}
pub(in crate::git) struct Discovery {
    pub paths: Vec<PathBuf>,
    pub warning: Option<String>,
}
pub(in crate::git) fn directory_for(data_root: &Path, root: &Path) -> PathBuf {
    directory(data_root, root)
}
pub(in crate::git) fn discover(data_root: &Path, root: &Path) -> Result<Discovery, String> {
    let directory = directory(data_root, root);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Discovery {
                paths: Vec::new(),
                warning: None,
            });
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut result = Vec::new();
    let mut total = 0usize;
    let mut warning = None;
    for (index, entry) in entries.take(257).enumerate() {
        if index >= 256 {
            warning=Some("More than 256 recovery records are stored. Open the recovery folder to inspect the rest.".into());
            break;
        }
        let inspected = (|| {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || entry.path().extension().is_none_or(|e| e != "json")
            {
                return Ok(None);
            }
            let mut bytes = Vec::new();
            fs::File::open(entry.path())
                .map_err(|e| e.to_string())?
                .take((MAX_TEXT_BYTES * 8 + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            total += bytes.len();
            if bytes.len() > MAX_TEXT_BYTES * 8 || total > 32 * 1024 * 1024 {
                return Err("Recovery discovery exceeds its size limit. Open the recovery folder to inspect the remaining records.".into());
            }
            let draft: Draft = serde_json::from_slice(&bytes)
                .map_err(|e| format!("Could not inspect a conflict recovery record: {e}"))?;
            super::super::conflicts::validate_path(&draft.source.index.path)?;
            if draft.text.len() > MAX_TEXT_BYTES
                || entry.path() != path(data_root, root, &draft.source.index.path)
            {
                return Err("A conflict recovery record has an invalid identity or size.".into());
            }
            Ok(Some(draft.source.index.path))
        })();
        match inspected {
            Ok(Some(path)) => result.push(path),
            Ok(None) => {}
            Err(error) => warning = Some(error),
        }
        if total > 32 * 1024 * 1024 {
            break;
        }
    }
    result.sort();
    Ok(Discovery {
        paths: result,
        warning,
    })
}
pub(super) fn read(path: &Path, relative: &Path) -> Result<Option<Draft>, String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_TEXT_BYTES * 8 + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_TEXT_BYTES * 8 {
        return Err("Recovery draft is too large to load.".into());
    }
    let draft: Draft = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Could not read recovery draft: {e}"))?;
    if draft.source.index.path != relative || draft.text.len() > MAX_TEXT_BYTES {
        return Err("Recovery draft does not match this file or exceeds the text limit.".into());
    }
    Ok(Some(draft))
}
pub(super) fn next(writer: &SharedWriter) -> u64 {
    let mut writer = writer.lock().unwrap_or_else(|e| e.into_inner());
    writer.revision += 1;
    writer.revision
}
pub(super) fn write_latest(
    path: &Path,
    writer: &SharedWriter,
    revision: u64,
    draft: Option<&Draft>,
) -> Result<(), String> {
    let state = writer.lock().unwrap_or_else(|e| e.into_inner());
    if state.revision != revision {
        return Ok(());
    }
    write(path, draft)
}
pub(super) fn flush(
    path: &Path,
    writer: &SharedWriter,
    draft: Option<&Draft>,
) -> Result<(), String> {
    let mut state = writer.lock().unwrap_or_else(|e| e.into_inner());
    state.revision += 1;
    write(path, draft)
}
fn write(path: &Path, draft: Option<&Draft>) -> Result<(), String> {
    if let Some(draft) = draft {
        if draft.text.len() > MAX_TEXT_BYTES {
            return Err("Result exceeds the 2 MiB recovery limit. Copy it before closing.".into());
        }
        let parent = path.parent().ok_or("No journal directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        serde_json::to_writer(&mut temporary, draft).map_err(|e| e.to_string())?;
        temporary.flush().map_err(|e| e.to_string())?;
        temporary.as_file().sync_all().map_err(|e| e.to_string())?;
        temporary.persist(path).map_err(|e| e.to_string())?;
    } else {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::conflicts::{Baseline, IndexConflict};
    fn draft() -> Draft {
        Draft {
            source: Source {
                index: IndexConflict {
                    path: "file.txt".into(),
                    stages: Default::default(),
                },
                disk: Baseline::Missing,
                operation: None,
            },
            text: "unsaved\r\n".into(),
        }
    }
    #[test]
    fn journal_is_scoped_atomic_and_never_modifies_repository() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("checkout");
        fs::create_dir(&root).unwrap();
        let file = root.join("file.txt");
        fs::write(&file, "on disk\n").unwrap();
        let journal = path(dir.path(), &root, Path::new("file.txt"));
        let writer = writer(&journal);
        flush(&journal, &writer, Some(&draft())).unwrap();
        assert_eq!(
            read(&journal, Path::new("file.txt")).unwrap().unwrap().text,
            "unsaved\r\n"
        );
        assert!(read(&journal, Path::new("other.txt")).is_err());
        assert_ne!(
            journal,
            path(dir.path(), &root.join("other"), Path::new("file.txt"))
        );
        assert_eq!(fs::read_to_string(&file).unwrap(), "on disk\n");
        flush(&journal, &writer, None).unwrap();
        assert!(read(&journal, Path::new("file.txt")).unwrap().is_none());
    }
    #[test]
    fn delayed_write_cannot_restore_a_cleared_or_newer_draft() {
        let dir = tempfile::tempdir().unwrap();
        let journal = dir.path().join("draft.json");
        let first = writer(&journal);
        let old = next(&first);
        let second = writer(&journal);
        assert!(Arc::ptr_eq(&first, &second));
        flush(&journal, &second, None).unwrap();
        write_latest(&journal, &first, old, Some(&draft())).unwrap();
        assert!(!journal.exists());
        let old = next(&first);
        let newer = next(&second);
        let mut latest = draft();
        latest.text = "newer".into();
        write_latest(&journal, &second, newer, Some(&latest)).unwrap();
        write_latest(&journal, &first, old, Some(&draft())).unwrap();
        assert_eq!(
            read(&journal, Path::new("file.txt")).unwrap().unwrap().text,
            "newer"
        );
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::git::conflicts::{
        Resolution, load, resolve,
        tests::{setup, text_conflict},
    };
    #[test]
    fn restart_discovery_finds_drafts_after_an_external_resolution_and_ignores_other_checkouts() {
        let dir = setup();
        let root = dir.path();
        let data = tempfile::tempdir().unwrap();
        text_conflict(root, Path::new("file.txt"));
        let source = load(root, Path::new("file.txt")).unwrap().source;
        let journal = path(data.path(), root, Path::new("file.txt"));
        let draft = Draft {
            source: source.clone(),
            text: "unsaved result\n".into(),
        };
        flush(&journal, &writer(&journal), Some(&draft)).unwrap();
        resolve(root, &source, &Resolution::Current).unwrap();
        assert_eq!(
            discover(data.path(), root).unwrap().paths,
            vec![PathBuf::from("file.txt")]
        );
        assert!(
            discover(data.path(), &root.join("different"))
                .unwrap()
                .paths
                .is_empty()
        );
        assert_eq!(
            read(&journal, Path::new("file.txt")).unwrap().unwrap().text,
            draft.text
        );
        assert_eq!(
            fs::read_to_string(root.join("file.txt")).unwrap(),
            "current\n"
        );
        fs::write(directory(data.path(), root).join("corrupt.json"), "broken").unwrap();
        let found = discover(data.path(), root).unwrap();
        assert_eq!(found.paths, vec![PathBuf::from("file.txt")]);
        assert!(found.warning.is_some());
    }
}
