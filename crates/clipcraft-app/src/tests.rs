//! End-to-end tests through the real window harness: draw a frame, add
//! movies the way a drop does, and check what the state says.

use gpui_kit::{Bounds, Entity, Pixels, TestAppContext, VisualTestContext, px};

use crate::state::{ClipCraft, Status};

fn open(
    cx: &mut TestAppContext,
) -> (Entity<ClipCraft>, &mut VisualTestContext) {
    cx.update(gpui_omarchy::init);
    cx.add_window_view(|window, cx| {
        let mut app = ClipCraft::new(None, None, window, cx);
        app.log_path = None;
        app
    })
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
}

#[gpui_kit::test]
fn the_window_draws_empty(cx: &mut TestAppContext) {
    let (_, cx) = open(cx);
    draw(cx);
    let bounds: Bounds<Pixels> =
        cx.debug_bounds("clipcraft-root").expect("root element");
    assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
}

#[gpui_kit::test]
fn a_dropped_folder_lists_its_movies_once(cx: &mut TestAppContext) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dir = temp.path();
    std::fs::create_dir(dir.join("Day 1")).expect("mkdir");
    for name in ["Day 1/a.mov", "b.MP4", "notes.txt", "b_compressed.mp4"] {
        std::fs::write(dir.join(name), "movie").expect("write");
    }
    let (view, cx) = open(cx);
    let folder = vec![dir.to_path_buf()];
    view.update(cx, |app, cx| app.add_paths(&folder, cx));
    // Dropping it again, or one of its movies, adds nothing.
    let again = vec![dir.to_path_buf(), dir.join("b.MP4")];
    view.update(cx, |app, cx| app.add_paths(&again, cx));
    draw(cx);
    let rows = view.read_with(cx, |app, _| app.rows.clone());
    let paths: Vec<_> = rows.iter().map(|row| row.path.clone()).collect();
    assert_eq!(paths, [dir.join("Day 1/a.mov"), dir.join("b.MP4")]);
    assert!(rows.iter().all(|row| row.status == Status::Waiting));
    assert_eq!(rows[0].bytes, 5);

    let first = rows[0].id;
    view.update(cx, |app, cx| app.remove(first, cx));
    draw(cx);
    assert_eq!(view.read_with(cx, |app, _| app.rows.len()), 1);
}

#[gpui_kit::test]
fn a_preset_is_renamed_and_its_settings_kept(cx: &mut TestAppContext) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let path = temp.path().join("presets.json");
    cx.update(gpui_omarchy::init);
    let saved = path.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let mut app = ClipCraft::new(Some(saved), None, window, cx);
        app.log_path = None;
        app
    });
    draw(cx);
    view.update(cx, |app, cx| {
        app.presets.selected = 1;
        app.toggle_replace(cx);
    });
    view.update_in(cx, ClipCraft::start_rename);
    draw(cx);
    view.update_in(cx, |app, window, cx| {
        let input = app.renaming.clone().expect("rename box open");
        input.update(cx, |input, cx| input.set_value("Drone", window, cx));
        app.finish_rename(true, window, cx);
    });
    draw(cx);
    let loaded = clipcraft_core::presets::Presets::load(&path);
    assert_eq!(loaded.current().name, "Drone");
    assert!(loaded.current().replace);
    assert_eq!(
        loaded.list[0].name, "Compress Video",
        "the other is untouched"
    );
    let label = view.read_with(cx, |app, cx| {
        app.choice
            .read(cx)
            .selected()
            .map(|item| item.label.to_string())
    });
    assert_eq!(label.as_deref(), Some("Drone"), "the dropdown shows it");
}

/// Three movies of 3, 1 and 2 bytes, listed in that order.
fn three(
    cx: &mut TestAppContext,
) -> (tempfile::TempDir, Entity<ClipCraft>, &mut VisualTestContext) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let paths: Vec<_> = [("a.mov", "aaa"), ("b.mov", "b"), ("c.mov", "cc")]
        .iter()
        .map(|(name, body)| {
            let path = temp.path().join(name);
            std::fs::write(&path, body).expect("write");
            path
        })
        .collect();
    let (view, cx) = open(cx);
    view.update(cx, |app, cx| app.add_paths(&paths, cx));
    (temp, view, cx)
}

fn names(view: &Entity<ClipCraft>, cx: &VisualTestContext) -> Vec<String> {
    view.read_with(cx, |app, _| {
        app.rows
            .iter()
            .map(|row| crate::state::file_name(&row.path))
            .collect()
    })
}

#[gpui_kit::test]
fn rows_are_selected_copied_sorted_and_moved(cx: &mut TestAppContext) {
    use gpui_kit::Modifiers;
    let (temp, view, cx) = three(cx);
    draw(cx);
    let ids: Vec<usize> =
        view.read_with(cx, |app, _| app.rows.iter().map(|r| r.id).collect());

    view.update_in(cx, |app, window, cx| {
        app.select(ids[0], Modifiers::default(), window, cx);
        app.select(ids[2], Modifiers::shift(), window, cx);
    });
    assert_eq!(view.read_with(cx, |app, _| app.selected.len()), 3);
    view.update_in(cx, |app, window, cx| {
        app.select(ids[1], Modifiers::command(), window, cx);
        app.copy_selected(cx);
    });
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    let dir = temp.path();
    assert_eq!(
        copied.as_deref(),
        Some(
            format!(
                "{}\n{}",
                dir.join("a.mov").display(),
                dir.join("c.mov").display()
            )
            .as_str()
        ),
        "⌘-click took b out"
    );

    view.update(cx, ClipCraft::sort_by_size);
    assert_eq!(
        names(&view, cx),
        ["a.mov", "c.mov", "b.mov"],
        "largest first"
    );
    view.update(cx, ClipCraft::sort_by_size);
    assert_eq!(names(&view, cx), ["b.mov", "c.mov", "a.mov"]);

    view.update(cx, |app, cx| app.move_row(ids[0], ids[1], cx));
    assert_eq!(
        names(&view, cx),
        ["a.mov", "b.mov", "c.mov"],
        "a dragged up"
    );

    // Select all, then Delete; the movie compressing stays.
    view.update(cx, |app, cx| {
        app.rows[1].status = Status::Compressing {
            fraction: Some(0.5),
            eta: None,
            speed: None,
        };
        app.select_all(cx);
        app.remove_selected(cx);
    });
    assert_eq!(names(&view, cx), ["b.mov"]);
    draw(cx);
}

#[gpui_kit::test]
fn settings_change_only_when_saved(cx: &mut TestAppContext) {
    use clipcraft_core::options::{Codec, Options};
    let (view, cx) = open(cx);
    view.update_in(cx, |app, window, cx| {
        app.presets.selected = 1;
        app.open_settings(window, cx);
        app.edit_settings(|draft| draft.options.codec = Codec::H264, cx);
    });
    draw(cx);
    view.update_in(cx, |app, window, cx| app.close_settings(false, window, cx));
    let current = view.read_with(cx, |app, _| app.preset().clone());
    assert_eq!(current.options, Options::AERIAL, "cancelled");

    view.update_in(cx, |app, window, cx| {
        app.open_settings(window, cx);
        app.edit_settings(|draft| draft.options.gpu = false, cx);
        app.edit_settings(|draft| draft.options.nudge_quality(true), cx);
        app.close_settings(true, window, cx);
    });
    draw(cx);
    let current = view.read_with(cx, |app, _| app.preset().clone());
    assert_eq!(
        current.options,
        Options {
            gpu: false,
            crf: 25,
            ..Options::AERIAL
        }
    );

    view.update_in(cx, ClipCraft::duplicate_preset);
    draw(cx);
    let (count, name) = view.read_with(cx, |app, _| {
        (app.presets.list.len(), app.preset().name.clone())
    });
    assert_eq!((count, name.as_str()), (3, "Compress Aerial copy"));
}

#[gpui_kit::test]
fn find_lists_matching_movies_in_a_folder(cx: &mut TestAppContext) {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let dir = temp.path();
    std::fs::create_dir(dir.join("2024 Trip")).expect("mkdir");
    for name in ["2024 Trip/DJI_0001.MP4", "DJI_0002.mov", "home.mov"] {
        std::fs::write(dir.join(name), "movie").expect("write");
    }
    let (view, cx) = open(cx);
    view.update_in(cx, |app, window, cx| {
        app.open_finder(window, cx);
        app.search_folder(dir.to_path_buf(), cx);
    });
    cx.run_until_parked();
    draw(cx);
    let query = view.read_with(cx, |app, _| {
        app.finder.as_ref().map(|f| f.query.clone()).expect("open")
    });
    let matching = |view: &Entity<ClipCraft>, cx: &mut VisualTestContext| {
        view.read_with(cx, |app, cx| app.finder_matches(cx).len())
    };
    assert_eq!(matching(&view, cx), 3);
    cx.update(|window, cx| {
        query.update(cx, |input, cx| input.set_value("dji trip", window, cx));
    });
    assert_eq!(matching(&view, cx), 1, "every word, in any case");
    draw(cx);
}

#[gpui_kit::test]
fn the_list_comes_back_after_a_restart(cx: &mut TestAppContext) {
    use clipcraft_core::job::Outcome;
    let temp = tempfile::TempDir::new().expect("tempdir");
    let list = temp.path().join("list.json");
    let movies: Vec<_> = ["a.mov", "b.mov"]
        .iter()
        .map(|name| {
            let path = temp.path().join(name);
            std::fs::write(&path, "movie").expect("write");
            path
        })
        .collect();
    cx.update(gpui_omarchy::init);
    let saved = list.clone();
    let (view, cx) = cx.add_window_view(move |window, cx| {
        let mut app = ClipCraft::new(None, Some(saved), window, cx);
        app.log_path = None;
        app
    });
    view.update(cx, |app, cx| {
        app.add_paths(&movies, cx);
        app.rows[1].status =
            Status::Finished(Outcome::Skipped("already HEVC".into()));
        cx.notify();
    });
    draw(cx);

    let again = list.clone();
    let (reopened, cx) = cx.add_window_view(move |window, cx| {
        let mut app = ClipCraft::new(None, Some(again), window, cx);
        app.log_path = None;
        app
    });
    draw(cx);
    let rows = reopened.read_with(cx, |app, _| app.rows.clone());
    assert_eq!(
        rows.len(),
        2,
        "{}",
        std::fs::read_to_string(&list).unwrap_or_default()
    );
    assert_eq!(rows[0].status, Status::Waiting);
    assert_eq!(
        rows[1].status,
        Status::Finished(Outcome::Skipped("already HEVC".into()))
    );
    assert_ne!(rows[0].id, rows[1].id);
}

#[test]
fn clocks_read_naturally() {
    use crate::views::clock;
    assert_eq!(clock(5.4), "0:05");
    assert_eq!(clock(125.0), "2:05");
    assert_eq!(clock(3725.0), "1:02:05");
}
