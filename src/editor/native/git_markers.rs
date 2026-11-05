//! Index-relative indicators for the active buffer. Kit owns range tracking;
//! background Git reads and line diffs refresh their meaning after edits.
use super::*;
use gpui_kit::component::input::{
    GutterDecoration, GutterDecorationCollection, OverviewRulerDecoration,
    OverviewRulerDecorationCollection, OverviewRulerLane, Rope,
};
use std::ops::Range;
use std::time::Instant;

type Baseline = Arc<Result<Option<String>, String>>;

struct CachedBaseline {
    path: PathBuf,
    read_at: Instant,
    value: Baseline,
}

#[derive(Default)]
pub(super) struct Markers {
    editor: Option<Entity<EditorState>>,
    gutter: GutterDecorationCollection,
    ruler: OverviewRulerDecorationCollection,
    generation: u64,
    task: Option<gpui_kit::Task<()>>,
    baseline: Option<CachedBaseline>,
    pub(super) unavailable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Added,
    Modified,
    Deleted,
}

#[derive(Debug, PartialEq, Eq)]
struct Mark {
    range: Range<usize>,
    kind: Kind,
}

fn line_marks(baseline: &str, text: &str, saved: &str) -> Vec<Mark> {
    let offsets = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect::<Vec<_>>();
    // Compare the bytes that Save would write. The filtered index already
    // reflects Git attributes, including -text and eol=crlf.
    let current = file_bytes_for_buffer(saved, text);
    let diff = similar::TextDiff::configure()
        .timeout(Duration::from_millis(100))
        .diff_lines(baseline, current.as_ref());
    let offset = |line: usize| offsets.get(line).copied().unwrap_or(text.len());
    let mut marks = Vec::new();
    for op in diff.ops() {
        let old = op.old_range();
        let new = op.new_range();
        let mut add = |start, end, kind| {
            marks.push(Mark {
                range: offset(start)..offset(end),
                kind,
            })
        };
        match op.tag() {
            similar::DiffTag::Equal => {}
            similar::DiffTag::Delete => add(new.start, new.start, Kind::Deleted),
            similar::DiffTag::Insert => add(new.start, new.end, Kind::Added),
            similar::DiffTag::Replace => {
                let shared = old.len().min(new.len());
                if shared > 0 {
                    add(new.start, new.start + shared, Kind::Modified);
                }
                if new.len() > shared {
                    add(new.start + shared, new.end, Kind::Added);
                }
                if old.len() > shared {
                    add(new.end, new.end, Kind::Deleted);
                }
            }
        }
    }
    marks
}

impl NativeEditor {
    pub(super) fn refresh_git_markers(&mut self, debounce: bool, cx: &mut Context<Self>) {
        self.git_markers.generation += 1;
        if self.git_markers.editor.as_ref() != Some(&self.editor) {
            self.git_markers.gutter.dispose(cx);
            self.git_markers.ruler.dispose(cx);
            let (gutter, ruler) = self.editor.update(cx, |editor, cx| {
                (
                    editor.create_gutter_decorations_collection(Vec::new(), cx),
                    editor.create_overview_ruler_decorations_collection(Vec::new(), cx),
                )
            });
            self.git_markers.editor = Some(self.editor.clone());
            self.git_markers.gutter = gutter;
            self.git_markers.ruler = ruler;
        }
        let Some(path) = self.path.clone() else {
            self.git_markers.gutter.clear(cx);
            self.git_markers.ruler.clear(cx);
            self.git_markers.unavailable = false;
            return;
        };
        // A running Git/filter process cannot be cancelled by dropping its
        // waiter. Keep one job and process the latest snapshot when it ends.
        if self.git_markers.task.is_some() {
            return;
        }
        let editor = self.editor.clone();
        let text: Rope = editor.read(cx).text().clone();
        let root = self.canonical_root.clone();
        let generation = self.git_markers.generation;
        let saved = self.saved.clone().unwrap_or_default();
        let cached = self
            .git_markers
            .baseline
            .as_ref()
            .filter(|base| base.path == path && base.read_at.elapsed() < Duration::from_secs(2))
            .map(|base| (base.read_at, base.value.clone()));
        self.git_markers.task = Some(cx.spawn(async move |view, cx| {
            if debounce {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
            }
            let snapshot = text.clone();
            let read_path = path.clone();
            let (read_at, baseline, result) = cx
                .background_spawn(async move {
                    let (read_at, baseline) = cached.unwrap_or_else(|| {
                        (
                            Instant::now(),
                            Arc::new(crate::git::editor_index_baseline(&root, &read_path)),
                        )
                    });
                    if snapshot.len() as u64 > MAX_FILE_BYTES {
                        return (
                            read_at,
                            baseline,
                            Err("Buffer is too large for Git indicators".into()),
                        );
                    }
                    let source = snapshot.to_string();
                    let result = baseline
                        .as_ref()
                        .as_ref()
                        .map(|base| line_marks(base.as_deref().unwrap_or(""), &source, &saved))
                        .map_err(Clone::clone);
                    (read_at, baseline, result)
                })
                .await;
            let _ = view.update(cx, |this, cx| {
                this.git_markers.task = None;
                this.git_markers.baseline = Some(CachedBaseline {
                    path: path.clone(),
                    read_at,
                    value: baseline,
                });
                if this.git_markers.generation != generation
                    || this.path.as_ref() != Some(&path)
                    || this.editor != editor
                    || this.editor.read(cx).text() != &text
                {
                    this.refresh_git_markers(false, cx);
                    return;
                }
                this.git_markers.unavailable = result.is_err();
                let status = &cx.theme().highlight_theme.style.status;
                let colors = [status.created(cx), status.modified(cx), status.deleted(cx)];
                let marks = result.unwrap_or_default();
                let color = |kind| {
                    colors[match kind {
                        Kind::Added => 0,
                        Kind::Modified => 1,
                        Kind::Deleted => 2,
                    }]
                };
                this.git_markers.gutter.set(
                    marks
                        .iter()
                        .map(|mark| {
                            GutterDecoration::new(mark.range.clone())
                                .with_color(color(mark.kind))
                                .with_folded_color(color(mark.kind))
                        })
                        .collect(),
                    cx,
                );
                this.git_markers.ruler.set(
                    marks
                        .iter()
                        .map(|mark| {
                            OverviewRulerDecoration::new(mark.range.clone())
                                .with_lane(OverviewRulerLane::Left)
                                .with_color(color(mark.kind))
                        })
                        .collect(),
                    cx,
                );
                cx.notify();
            });
        }));
    }

    pub(super) fn poll_git_markers(&mut self, cx: &mut Context<Self>) {
        if self.git_markers.task.is_none() {
            self.refresh_git_markers(false, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui_kit::test]
    fn queued_refreshes_follow_latest_document_text_index_and_close(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        cx.update(gpui_kit::init);
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .current_dir(&root)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            )
        };
        git(&["init", "-b", "main"]);
        let first = root.join("first.txt");
        let second = root.join("second.txt");
        fs::write(&first, "base\n").unwrap();
        fs::write(&second, "clean\n").unwrap();
        git(&["add", "."]);
        fs::write(&first, "changed\n").unwrap();
        let (view, test_cx) = super::super::tests::test_editor(cx, &root);
        view.update_in(test_cx, |view, window, cx| {
            view.open(&first, None, window, cx).unwrap();
            assert!(view.git_markers.task.is_some());
            view.open(&second, None, window, cx).unwrap();
        });
        test_cx.run_until_parked();
        view.read_with(test_cx, |view, cx| {
            assert_eq!(view.git_markers.baseline.as_ref().unwrap().path, second);
            assert!(
                view.git_markers.gutter.get_ranges(cx).is_empty(),
                "first file's result must not decorate the second"
            );
        });
        view.update_in(test_cx, |view, window, cx| {
            view.open(&first, None, window, cx).unwrap();
            view.editor
                .update(cx, |editor, cx| editor.set_value("new\n", window, cx));
        });
        test_cx.run_until_parked();
        view.read_with(test_cx, |view, cx| {
            assert_eq!(view.git_markers.gutter.get_ranges(cx), vec![0..4])
        });
        fs::write(&first, "new\n").unwrap();
        git(&["add", "first.txt"]);
        view.update(test_cx, |view, cx| {
            // Expire the baseline exactly as the wall-clock cache timeout would.
            view.git_markers.baseline.as_mut().unwrap().read_at =
                Instant::now() - Duration::from_secs(3);
            view.poll_git_markers(cx);
        });
        test_cx.run_until_parked();
        view.read_with(test_cx, |view, cx| {
            assert!(view.git_markers.gutter.get_ranges(cx).is_empty())
        });
        view.update_in(test_cx, |view, window, cx| {
            view.refresh_git_markers(false, cx);
            view.dirty = false;
            view.finish_close(&first, window, cx);
            view.finish_close(&second, window, cx);
        });
        test_cx.run_until_parked();
        view.read_with(test_cx, |view, cx| {
            assert!(view.path.is_none());
            assert!(view.git_markers.gutter.get_ranges(cx).is_empty());
            assert!(view.git_markers.ruler.get_ranges(cx).is_empty());
        });
    }

    #[test]
    fn line_changes_use_buffer_offsets_and_the_saved_line_ending_convention() {
        assert!(line_marks("one\r\ntwo\r\n", "one\ntwo\n", "one\r\ntwo\r\n").is_empty());
        assert_eq!(
            line_marks("one\r\ntwo\r\n", "one\ntwo\n", "one\ntwo\n"),
            vec![Mark {
                range: 0..8,
                kind: Kind::Modified
            }]
        );
        assert_eq!(
            line_marks("one\ntwo\n", "one\n終\n", "one\ntwo\n"),
            vec![Mark {
                range: 4..8,
                kind: Kind::Modified
            }]
        );
        assert_eq!(
            line_marks("", "🎉\n", ""),
            vec![Mark {
                range: 0..5,
                kind: Kind::Added
            }]
        );
        assert_eq!(
            line_marks("a\nb\n", "a\n", "a\nb\n"),
            vec![Mark {
                range: 2..2,
                kind: Kind::Deleted
            }]
        );
        assert_eq!(
            line_marks("a", "", "a"),
            vec![Mark {
                range: 0..0,
                kind: Kind::Deleted
            }]
        );
        assert_eq!(
            line_marks("a\nb\nc\n", "a\nx\n", "a\nb\nc\n"),
            vec![
                Mark {
                    range: 2..4,
                    kind: Kind::Modified
                },
                Mark {
                    range: 4..4,
                    kind: Kind::Deleted
                }
            ]
        );
    }
}
