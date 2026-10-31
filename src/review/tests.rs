use super::*;
use gpui_kit::component::diff::{DiffLineRange, DiffMode, DiffSide};
use std::{fs, process::Command};

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?} failed");
}

pub(super) fn setup(cx: &mut gpui_kit::TestAppContext) -> (tempfile::TempDir, Entity<ReviewView>) {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Test"]);
    git(dir.path(), &["config", "user.email", "test@example.com"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    for path in ["a.txt", "b.txt", "manual.txt"] {
        fs::write(dir.path().join(path), "original\n").unwrap();
    }
    commit(dir.path());
    git(dir.path(), &["checkout", "-b", "review"]);
    for path in ["a.txt", "b.txt", "manual.txt"] {
        fs::write(dir.path().join(path), "reviewed\n").unwrap();
    }
    cx.update(gpui_kit::init);
    let view = cx.new(|cx| ReviewView::new(dir.path(), cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(matches!(view.state, ReviewState::Loaded(_)))
    });
    (dir, view)
}

fn commit(root: &Path) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "fixture"]);
}

#[gpui_kit::test]
fn long_file_names_scroll_independently(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (dir, view) = setup(cx);
    for prefix in 0..60 {
        fs::write(
            dir.path()
                .join(format!("{prefix:02}_{}.rs", "long_name_".repeat(15))),
            "new\n",
        )
        .unwrap();
    }
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let tree_scroll = view.read_with(cx, |view, cx| {
        view.tree_state.read(cx).scroll_handle().clone()
    });
    let (_, cx) =
        cx.add_window_view(move |window, cx| gpui_kit::component::Root::new(view, window, cx));
    cx.update(|window, cx| window.render_frame(cx));
    let first = cx.debug_bounds("review-file-name-0").unwrap();
    let second = cx.debug_bounds("review-file-name-1").unwrap();
    assert!(first.size.width > px(320.), "fixture must overflow sidebar");
    let area = cx.debug_bounds("review-name-area-0").unwrap();
    assert!(
        area.right() < px(320.),
        "filename must leave room for status"
    );
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(150.), first.center().y),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-120.), px(0.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.render_frame(cx));
    let moved = cx.debug_bounds("review-file-name-0").unwrap();
    let other = cx.debug_bounds("review-file-name-1").unwrap();
    assert_eq!(
        moved.left(),
        first.left() - px(120.),
        "long filename must scroll horizontally 1:1 with the wheel delta"
    );
    assert_eq!(other.left(), second.left(), "other filenames must not move");
    assert_eq!(
        tree_scroll.0.borrow().base_handle.offset().y,
        px(0.),
        "horizontal gestures must not move the file list"
    );
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(150.), first.center().y),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-10_000.), px(0.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.render_frame(cx));
    let at_end = cx.debug_bounds("review-file-name-0").unwrap();
    assert_eq!(
        at_end.right(),
        area.right(),
        "horizontal scroll must clamp at the end of the filename"
    );
    assert_eq!(
        tree_scroll.0.borrow().base_handle.offset().y,
        px(0.),
        "overscrolling horizontally must not move the file list"
    );
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(150.), first.center().y),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(10_000.), px(0.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.render_frame(cx));
    let at_start = cx.debug_bounds("review-file-name-0").unwrap();
    assert_eq!(
        at_start.left(),
        first.left(),
        "horizontal scroll must clamp at the start of the filename"
    );
    assert_eq!(
        tree_scroll.0.borrow().base_handle.offset().y,
        px(0.),
        "overscrolling back to the start must not move the file list"
    );
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(150.), first.center().y),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-120.))),
        ..Default::default()
    });
    cx.update(|window, cx| window.render_frame(cx));
    assert!(
        tree_scroll.0.borrow().base_handle.offset().y < px(0.),
        "vertical wheel must scroll the file list"
    );
}

#[gpui_kit::test]
fn split_diff_wraps_long_lines_and_reflows_on_resize(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (dir, view) = setup(cx);
    fs::write(
        dir.path().join("a.txt"),
        format!("{}\nnext\n", "identifier ".repeat(60)),
    )
    .unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let render_view = view.clone();
    let (_, cx) = cx
        .add_window_view(move |window, cx| gpui_kit::component::Root::new(render_view, window, cx));
    cx.simulate_resize(gpui_kit::size(px(900.), px(900.)));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.click("review-diff-split", cx);
    });
    cx.run_until_parked();
    let first = cx.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        assert!(window.try_find("git-diff-wrap").is_none());
        let first = window
            .within(("diff-file", 0usize))
            .within(("new", 0usize))
            .find("source")
            .bounds();
        assert!(first.size.height > px(48.), "Split must wrap automatically");
        first
    });
    cx.simulate_resize(gpui_kit::size(px(1600.), px(900.)));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        let wider = window
            .within(("diff-file", 0usize))
            .within(("new", 0usize))
            .find("source")
            .bounds();
        assert!(
            wider.size.height < first.size.height,
            "wider columns reflow the source"
        );
        let next = window
            .within(("diff-file", 0usize))
            .within(("new", 1usize))
            .find("source")
            .bounds();
        assert!(next.top() >= wider.bottom(), "wrapped rows cannot overlap");
    });
}

#[gpui_kit::test]
fn navigation_mode_scrolls_the_focused_kit_diff(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (dir, view) = setup(cx);
    fs::write(
        dir.path().join("a.txt"),
        (0..150)
            .map(|ix| format!("line {ix}\n"))
            .collect::<String>(),
    )
    .unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let render_view = view.clone();
    let (_, cx) = cx
        .add_window_view(move |window, cx| gpui_kit::component::Root::new(render_view, window, cx));
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.render_frame(cx);
        view.read(cx).diff_state.focus_handle(cx).focus(window, cx);
        let before = window
            .within(("diff-file", 0usize))
            .within(("new", 0usize))
            .find("source")
            .bounds();
        view.update(cx, |view, cx| view.move_diff_scroll(true, window, cx));
        window.render_frame(cx);
        let after = window
            .within(("diff-file", 0usize))
            .within(("new", 0usize))
            .find("source")
            .bounds();
        assert!(
            after.top() < before.top(),
            "navigation-mode j must scroll Kit's code"
        );
    });
}

fn assert_progress(view: &ReviewView, path: &str, viewed: bool, collapsed: bool) {
    assert_eq!(
        view.viewed.borrow().contains(path),
        viewed,
        "viewed: {path}"
    );
    assert_eq!(
        view.collapsed.borrow().contains(path),
        collapsed,
        "collapsed: {path}"
    );
    let ReviewState::Loaded(loaded) = &view.state else {
        panic!("expected loaded review");
    };
    let file_ix = loaded
        .diff
        .files
        .iter()
        .position(|file| file.path == path)
        .unwrap();
    let rows = loaded
        .rows
        .iter()
        .filter(|row| row.file() == file_ix)
        .count();
    if collapsed {
        assert_eq!(rows, 1, "collapsed file has only a header");
    } else {
        assert!(rows > 1, "expanded file includes its diff");
    }
}

#[gpui_kit::test]
fn reload_reopens_changed_viewed_files_and_preserves_other_progress(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (dir, view) = setup(cx);
    view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        view.toggle_viewed("b.txt", cx);
        view.toggle_collapsed("manual.txt", cx);
    });
    // Same addition/deletion counts, different source text.
    fs::write(dir.path().join("a.txt"), "updated!\n").unwrap();
    fs::write(dir.path().join("manual.txt"), "updated!\n").unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_progress(view, "a.txt", false, false);
        assert_progress(view, "b.txt", true, true);
        assert_progress(view, "manual.txt", false, true);
    });
    // A file can be reviewed again at its new version.
    view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        view.reload(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "a.txt", true, true));
}

#[gpui_kit::test]
fn background_refresh_keeps_unchanged_rows_and_reopens_changed_files(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (dir, view) = setup(cx);
    let before = view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        let ReviewState::Loaded(loaded) = &view.state else {
            unreachable!()
        };
        loaded.clone()
    });
    view.update(cx, |view, cx| {
        view.refresh(cx);
        let generation = view.generation;
        view.refresh(cx);
        assert_eq!(
            view.generation, generation,
            "background loads do not overlap"
        );
        assert!(matches!(view.state, ReviewState::Loaded(_)));
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let ReviewState::Loaded(loaded) = &view.state else {
            unreachable!()
        };
        assert!(
            Rc::ptr_eq(&before, loaded),
            "unchanged refresh leaves UI state intact"
        );
        assert_progress(view, "a.txt", true, true);
    });
    fs::write(dir.path().join("a.txt"), "updated!\n").unwrap();
    view.update(cx, |view, cx| view.refresh(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "a.txt", false, false));
}

#[gpui_kit::test]
fn refresh_reopens_changed_files_and_retains_the_selected_source(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (dir, view) = setup(cx);
    fs::write(dir.path().join("b.txt"), "reviewed\n".repeat(100)).unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let selection = DiffLineRange::new("b.txt", DiffSide::Modified, 10, 12);
    view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        view.last_scrolled = Some("b.txt".into());
        view.diff_state.update(cx, |diff, cx| {
            diff.set_selected_lines(Some(selection.clone()), cx)
        });
    });
    fs::write(dir.path().join("a.txt"), "updated!\n").unwrap();
    view.update(cx, |view, cx| view.refresh(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_progress(view, "a.txt", false, false);
        assert_eq!(view.last_scrolled.as_deref(), Some("b.txt"));
        assert_eq!(view.diff_state.read(cx).selected_lines(), Some(selection));
        assert!(!view.diff_state.read(cx).is_file_collapsed("a.txt"));
    });
}

#[gpui_kit::test]
fn returning_to_review_keeps_unchanged_rows(cx: &mut gpui_kit::TestAppContext) {
    let (_dir, view) = setup(cx);
    let before = view.read_with(cx, |view, _| {
        let ReviewState::Loaded(loaded) = &view.state else {
            unreachable!()
        };
        loaded.clone()
    });
    view.update(cx, |view, cx| {
        view.activate(cx);
        assert!(matches!(view.state, ReviewState::Loaded(_)));
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        let ReviewState::Loaded(loaded) = &view.state else {
            unreachable!()
        };
        assert!(Rc::ptr_eq(&before, loaded));
    });
}

#[gpui_kit::test]
fn refresh_reanchors_selected_source_when_lines_and_files_move(cx: &mut gpui_kit::TestAppContext) {
    let (dir, view) = setup(cx);
    let content = (0..150)
        .map(|ix| format!("line {ix}\n"))
        .collect::<String>();
    fs::write(dir.path().join("b.txt"), &content).unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.update(cx, |view, cx| {
        view.last_scrolled = Some("b.txt".into());
        view.diff_state.update(cx, |diff, cx| {
            diff.set_selected_lines(
                Some(DiffLineRange::new("b.txt", DiffSide::Modified, 40, 41)),
                cx,
            )
        });
    });
    for (path, value) in [
        ("aa.txt", "new file\n".to_owned()),
        ("a.txt", "original\n".to_owned()),
        ("b.txt", format!("inserted\n{content}")),
    ] {
        fs::write(dir.path().join(path), value).unwrap();
        view.update(cx, |view, cx| view.refresh(cx));
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert_eq!(
                view.diff_state.read(cx).selected_text(cx),
                "line 39\nline 40\n"
            );
            assert_eq!(view.last_scrolled.as_deref(), Some("b.txt"));
            let selected = view.tree_state.read(cx).selected_entry().unwrap();
            assert_eq!(file_path_from_id(&selected.item().id), Some("b.txt"));
        });
    }
}

#[gpui_kit::test]
fn commit_only_refresh_retains_kit_state_and_selection(cx: &mut gpui_kit::TestAppContext) {
    let (dir, view) = setup(cx);
    let selection = DiffLineRange::new("b.txt", DiffSide::Modified, 1, 1);
    let before = view.update(cx, |view, cx| {
        view.diff_state.update(cx, |diff, cx| {
            diff.set_mode(DiffMode::Split, cx);
            diff.set_selected_lines(Some(selection.clone()), cx);
        });
        view.diff_state.entity_id()
    });
    commit(dir.path());
    view.update(cx, |view, cx| view.refresh(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, cx| {
        assert_eq!(view.diff_state.entity_id(), before);
        assert_eq!(view.diff_state.read(cx).mode(), DiffMode::Split);
        assert_eq!(view.diff_state.read(cx).selected_lines(), Some(selection));
        assert_eq!(view.diff_state.read(cx).selected_text(cx), "reviewed\n");
    });
}

#[gpui_kit::test]
fn filesystem_events_gate_refreshes_and_queue_changes_during_a_load(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (dir, view) = setup(cx);
    let watched = tempfile::tempdir().unwrap();
    view.update(cx, |view, cx| {
        // Keep OS delivery separate from the deterministic queue interleaving.
        view.file_changes = watch::FileChanges::new(watched.path());
        assert!(
            view.file_changes.is_watching(),
            "fixture watcher is available"
        );
        view.file_changes.take_changed();
        let generation = view.generation;
        view.refresh_if_changed(cx);
        assert_eq!(view.generation, generation, "idle ticks do not reload");
        view.file_changes.mark_changed();
        view.refresh_if_changed(cx);
        assert_eq!(view.generation, generation + 1);
        view.file_changes.mark_changed();
        view.refresh_if_changed(cx);
        assert_eq!(
            view.generation,
            generation + 1,
            "writes during a load stay queued"
        );
    });
    cx.run_until_parked();
    fs::write(dir.path().join("a.txt"), "updated!\n").unwrap();
    view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        view.refresh_if_changed(cx);
        assert!(view.load_in_flight, "queued event starts the next refresh");
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "a.txt", false, false));
}

#[gpui_kit::test]
fn commits_preserve_unchanged_viewed_files_but_branch_and_scope_switches_reset_them(
    cx: &mut gpui_kit::TestAppContext,
) {
    let (dir, view) = setup(cx);
    view.update(cx, |view, cx| view.toggle_viewed("b.txt", cx));
    commit(dir.path());
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "b.txt", true, true));
    git(dir.path(), &["checkout", "-b", "another-review"]);
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "b.txt", false, false));
    view.update(cx, |view, cx| {
        view.toggle_viewed("b.txt", cx);
        view.scope_tab = ScopeTab::Uncommitted;
        view.reload(cx);
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.viewed.borrow().is_empty());
        assert!(view.collapsed.borrow().is_empty());
        assert!(view.viewed_versions.is_empty());
    });
}

#[gpui_kit::test]
fn removed_and_renamed_files_do_not_keep_stale_marks(cx: &mut gpui_kit::TestAppContext) {
    let (dir, view) = setup(cx);
    view.update(cx, |view, cx| {
        view.toggle_viewed("a.txt", cx);
        view.toggle_viewed("b.txt", cx);
    });
    fs::write(dir.path().join("a.txt"), "original\n").unwrap();
    fs::rename(dir.path().join("b.txt"), dir.path().join("renamed.txt")).unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(!view.viewed.borrow().contains("a.txt"));
        assert!(!view.collapsed.borrow().contains("a.txt"));
        assert_progress(view, "b.txt", false, false);
        assert_progress(view, "renamed.txt", false, false);
        assert!(view.viewed_versions.is_empty());
    });
}

#[gpui_kit::test]
fn a_failed_refresh_preserves_progress_until_a_successful_load(cx: &mut gpui_kit::TestAppContext) {
    let (dir, view) = setup(cx);
    view.update(cx, |view, cx| view.toggle_viewed("a.txt", cx));
    git(dir.path(), &["branch", "-m", "main", "base-unavailable"]);
    view.update(cx, |view, cx| view.refresh(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert_progress(view, "a.txt", true, true);
        assert!(!view.load_in_flight);
    });
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(matches!(view.state, ReviewState::Failed(_)))
    });
    git(dir.path(), &["branch", "-m", "base-unavailable", "main"]);
    fs::write(dir.path().join("a.txt"), "updated!\n").unwrap();
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    view.read_with(cx, |view, _| assert_progress(view, "a.txt", false, false));
}

#[gpui_kit::test]
fn sticky_file_header_follows_the_top_file_and_toggles_viewed(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (dir, view) = setup(cx);
    for path in ["a.txt", "b.txt"] {
        let content = (0..120)
            .map(|ix| format!("reviewed {ix} {}\n", "x".repeat(80)))
            .collect::<String>();
        fs::write(dir.path().join(path), content).unwrap();
    }
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let render_view = view.clone();
    let (_, cx) = cx
        .add_window_view(move |window, cx| gpui_kit::component::Root::new(render_view, window, cx));
    cx.simulate_resize(gpui_kit::size(px(900.), px(700.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.sticky_header_path().map(str::to_owned)),
        Some("a.txt".to_owned())
    );
    let first = cx
        .debug_bounds("review-sticky-file-header")
        .expect("the first file's header must be pinned");

    view.update(cx, |view, cx| {
        view.diff_state
            .update(cx, |diff, cx| diff.scroll_to_file("b.txt", cx));
    });
    cx.update(|window, cx| window.render_frame(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.sticky_header_path().map(str::to_owned)),
        Some("b.txt".to_owned())
    );
    let second = cx
        .debug_bounds("review-sticky-file-header")
        .expect("the second file's header must be pinned");
    assert_eq!(
        first.top(),
        second.top(),
        "the pinned header must not move with the diff"
    );

    let viewed = cx
        .debug_bounds("review-sticky-viewed")
        .expect("the pinned header's viewed toggle must be reachable");
    assert!(
        viewed.left() >= second.left() && viewed.right() <= second.right(),
        "the viewed toggle must stay inside the pinned header"
    );
    let line_before = cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .within(("diff-file", 1usize))
            .within(("new", 0usize))
            .find("source")
            .bounds()
    });
    cx.simulate_event(gpui_kit::ScrollEvent {
        position: gpui_kit::point(px(560.), px(400.)),
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(-320.), px(0.))),
        ..Default::default()
    });
    cx.run_until_parked();
    let line_after = cx.update(|window, cx| {
        window.render_frame(cx);
        window
            .within(("diff-file", 1usize))
            .within(("new", 0usize))
            .find("source")
            .bounds()
    });
    assert!(
        line_after.left() < line_before.left(),
        "the diff content must scroll horizontally"
    );
    let viewed_after = cx
        .debug_bounds("review-sticky-viewed")
        .expect("the pinned header's viewed toggle must stay visible");
    assert_eq!(
        viewed.left(),
        viewed_after.left(),
        "the pinned header must not scroll horizontally"
    );
    cx.simulate_event(gpui_kit::MouseDownEvent {
        button: gpui_kit::MouseButton::Left,
        position: viewed.center(),
        ..Default::default()
    });
    cx.run_until_parked();
    view.read_with(cx, |view, _| {
        assert!(view.viewed.borrow().contains("b.txt"));
        assert!(view.collapsed.borrow().contains("b.txt"));
    });
}

#[gpui_kit::test]
fn sticky_file_header_tracks_wheel_scrolling(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::test::TestWindowExt as _;
    let (dir, view) = setup(cx);
    for path in ["a.txt", "b.txt"] {
        let content = (0..120)
            .map(|ix| format!("reviewed {ix}\n"))
            .collect::<String>();
        fs::write(dir.path().join(path), content).unwrap();
    }
    view.update(cx, |view, cx| view.reload(cx));
    cx.run_until_parked();
    let render_view = view.clone();
    let (_, cx) = cx
        .add_window_view(move |window, cx| gpui_kit::component::Root::new(render_view, window, cx));
    cx.simulate_resize(gpui_kit::size(px(900.), px(700.)));
    cx.update(|window, cx| window.render_frame(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    assert_eq!(
        view.read_with(cx, |view, _| view.sticky_header_path().map(str::to_owned)),
        Some("a.txt".to_owned())
    );
    let sticky = cx
        .debug_bounds("review-sticky-file-header")
        .expect("the first file's header must be pinned");
    let position = gpui_kit::point(sticky.center().x, px(320.));

    let mut reached_b = false;
    for _ in 0..40 {
        cx.simulate_event(gpui_kit::ScrollEvent {
            position,
            delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(-200.))),
            ..Default::default()
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        if view.read_with(cx, |view, _| view.sticky_header_path() == Some("b.txt")) {
            reached_b = true;
            break;
        }
    }
    assert!(reached_b, "the pinned header must follow wheel scrolling");

    let pinned = cx
        .debug_bounds("review-sticky-file-header")
        .expect("the second file's header must be pinned");
    assert_eq!(
        sticky.top(),
        pinned.top(),
        "the pinned header must stay at the top while scrolling"
    );
}
