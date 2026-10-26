//! Opt-in CPU rendering comparisons using GPUI's test platform.
//! Fixture construction and warmup are untimed. These measurements exclude
//! native GPU presentation, display pacing, PTY output and WebView rendering.

use std::{fs, path::Path, process::Command, time::Instant};

use gpui_kit::component::Root;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    App, AppContext as _, PlatformInput, ScrollDelta, ScrollEvent, TestAppContext,
    VisualTestContext, Window, point, px, size,
};

const WARMUP: usize = 40;
const SAMPLES: usize = 240;

fn init(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::fonts::register_bundled_fonts(cx).unwrap();
    });
}

fn measure(workload: &str, mut frame: impl FnMut(usize)) {
    for i in 0..WARMUP {
        frame(i);
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for i in 0..SAMPLES {
        let start = Instant::now();
        frame(i);
        samples.push(start.elapsed().as_secs_f64() * 1_000.);
    }
    samples.sort_by(f64::total_cmp);
    let backend = if cfg!(feature = "gpui-fast") {
        "fast"
    } else {
        "pre"
    };
    eprintln!(
        "GPUI_BACKEND_BENCH {}",
        serde_json::json!({
            "backend": backend,
            "platform": "gpui-test",
            "workload": workload,
            "samples": SAMPLES,
            "unit": "ms/input-and-render",
            "median": samples[SAMPLES / 2],
            "p95": samples[SAMPLES * 95 / 100],
        })
    );
}

fn scroll_frame(window: &mut Window, cx: &mut App, i: usize) {
    // Traverse the same part of the fixture in both directions. Keep the
    // pointer in the reader/review stream, away from sidebar controls.
    let delta = if i % 80 < 40 { -48. } else { 48. };
    window.dispatch_event(
        PlatformInput::Scroll(ScrollEvent {
            position: point(px(800.), px(450.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
            ..Default::default()
        }),
        cx,
    );
    window.render_frame(cx);
}

fn prepare_window(cx: &mut VisualTestContext) -> gpui_kit::AnyWindowHandle {
    let handle = cx.update(|window, _| window.window_handle());
    cx.simulate_window_resize(handle, size(px(1440.), px(900.)));
    cx.update(|window, cx| window.render_frame(cx));
    handle
}

fn assert_scrolls(cx: &mut VisualTestContext, list: &gpui_kit::ListState) {
    let before = list.logical_scroll_top();
    cx.update(|window, cx| scroll_frame(window, cx, 0));
    let after = list.logical_scroll_top();
    assert!(
        before.item_ix != after.item_ix || before.offset_in_item != after.offset_in_item,
        "wheel input must move the fixture viewport"
    );
    list.scroll_to(before);
    cx.update(|window, cx| window.render_frame(cx));
}

fn markdown_fixture() -> String {
    let mut source = String::from("# Rendering comparison\n\n");
    for i in 0..200 {
        source.push_str(&format!(
            "## Section {i}\n\nUnicode café 日本語 and **bold text**, with a [link](https://example.com).\n\n\
             A paragraph long enough to wrap when the window changes width. \
             Repeatable text keeps both backends working on identical content.\n\n\
             | Name | Value |\n| --- | --- |\n| Section | {i} |\n\n\
             ```rust\nfn section_{i}() {{\n    println!(\"section {i}\");\n}}\n```\n\n"
        ));
    }
    source
}

#[gpui_kit::test]
#[ignore = "CPU comparison; run explicitly with --ignored --nocapture --test-threads=1"]
fn backend_benchmark_reader_scroll(cx: &mut TestAppContext) {
    init(cx);
    let view = cx.new(|cx| crate::preview::PreviewView::embedded(markdown_fixture().into(), cx));
    cx.run_until_parked();
    let (list, blocks) = view.read_with(cx, |view, cx| view.benchmark_list_state(cx));
    assert!(blocks >= 200, "expected the complete reader fixture");
    let (_, cx) = cx.add_window_view(move |window, cx| Root::new(view, window, cx));
    prepare_window(cx);
    assert_scrolls(cx, &list);
    measure("reader-scroll-200-sections", |i| {
        cx.update(|window, cx| scroll_frame(window, cx, i));
    });
}

#[gpui_kit::test]
#[ignore = "CPU comparison; run explicitly with --ignored --nocapture --test-threads=1"]
fn backend_benchmark_reader_resize(cx: &mut TestAppContext) {
    init(cx);
    let view = cx.new(|cx| crate::preview::PreviewView::embedded(markdown_fixture().into(), cx));
    cx.run_until_parked();
    let (_, cx) = cx.add_window_view(move |window, cx| Root::new(view, window, cx));
    let handle = prepare_window(cx);
    measure("reader-resize-200-sections", |i| {
        let width = if i % 2 == 0 { 1000. } else { 1440. };
        cx.simulate_window_resize(handle, size(px(width), px(900.)));
        cx.update(|window, cx| window.render_frame(cx));
    });
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(output.status.success(), "benchmark fixture git {args:?}");
}

fn review_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Benchmark"]);
    git(
        dir.path(),
        &["config", "user.email", "benchmark@example.com"],
    );
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    for file in 0..20 {
        fs::write(
            dir.path().join(format!("file-{file:02}.rs")),
            (0..200)
                .map(|line| format!("const VALUE_{line}: usize = {line};\n"))
                .collect::<String>(),
        )
        .unwrap();
    }
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-m", "benchmark fixture"]);
    git(dir.path(), &["checkout", "-b", "comparison"]);
    for file in 0..20 {
        fs::write(
            dir.path().join(format!("file-{file:02}.rs")),
            (0..200)
                .map(|line| format!("const VALUE_{line}: usize = {};\n", line + 1))
                .collect::<String>(),
        )
        .unwrap();
    }
    dir
}

#[gpui_kit::test]
#[ignore = "CPU comparison; run explicitly with --ignored --nocapture --test-threads=1"]
fn backend_benchmark_review_scroll(cx: &mut TestAppContext) {
    init(cx);
    let dir = review_fixture();
    let view = cx.new(|cx| crate::review::ReviewView::new(dir.path(), cx));
    cx.run_until_parked();
    let (list, files) = view
        .read_with(cx, |view, _| view.benchmark_list_state())
        .expect("review fixture must finish loading successfully");
    assert_eq!(files, 20, "expected the complete review fixture");
    let (_, cx) = cx.add_window_view(move |window, cx| Root::new(view, window, cx));
    prepare_window(cx);
    assert_scrolls(cx, &list);
    measure("review-scroll-20-files-4000-changed-lines", |i| {
        cx.update(|window, cx| scroll_frame(window, cx, i));
    });
}
