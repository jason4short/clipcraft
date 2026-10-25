//! `clipcraft`: drop movies in, and the compress scripts run on each.
//!
//! Choose `compressVideo` or `compressAerial`, drop movies or folders on the
//! window (or paste their paths), and press Start. The queue stays open:
//! anything dropped while it runs is compressed next.

mod log;
mod state;
#[cfg(test)]
mod tests;
mod ui;
mod views;

use gpui_kit::{AppContext as _, KeyBinding, WindowOptions, px, size};
use state::ClipCraft;

mod actions {
    // The macro derives `PartialEq` without `Eq`; it is not ours to change.
    #![allow(clippy::derive_partial_eq_without_eq)]
    gpui_kit::actions!(
        clipcraft,
        [Copy, Paste, Quit, RemoveSelected, SelectAll]
    );
}
pub use actions::{Copy, Paste, Quit, RemoveSelected, SelectAll};

const USAGE: &str = "\
clipcraft — drop movies in, and compressVideo or compressAerial runs on each

usage: clipcraft [PATH...]

Movies and folders named on the command line start in the list.
";

fn main() {
    let mut paths = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                return;
            }
            // macOS hands apps opened from Finder a process serial number.
            other if other.starts_with("-psn_") => {}
            other => paths.push(std::path::PathBuf::from(other)),
        }
    }

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_omarchy::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-c", Copy, Some("ClipCraft")),
                KeyBinding::new("cmd-v", Paste, Some("ClipCraft")),
                KeyBinding::new("cmd-a", SelectAll, Some("ClipCraft")),
                KeyBinding::new("backspace", RemoveSelected, Some("ClipCraft")),
                KeyBinding::new("delete", RemoveSelected, Some("ClipCraft")),
                KeyBinding::new("cmd-q", Quit, None),
            ]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            let bounds =
                gpui_kit::Bounds::centered(None, size(px(960.), px(680.)), cx);
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(gpui_kit::WindowBounds::Windowed(
                            bounds,
                        )),
                        titlebar: Some(gpui_kit::TitlebarOptions {
                            title: Some("clipcraft".into()),
                            ..Default::default()
                        }),
                        window_min_size: Some(size(px(640.), px(420.))),
                        ..Default::default()
                    },
                    |window, cx| {
                        cx.new(|cx| {
                            ClipCraft::new(
                                clipcraft_core::presets::Presets::default_path(
                                ),
                                state::default_list_path(),
                                window,
                                cx,
                            )
                        })
                    },
                )
                .expect("open the clipcraft window");
            let _ = window.update(cx, |this, window, cx| {
                this.log("clipcraft started");
                let focus = this.focus.clone();
                window.focus(&focus, cx);
                if !paths.is_empty() {
                    this.add_paths(&paths, cx);
                }
            });
            // One window: closing it is quitting, which also stops ffmpeg.
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
        });
}
