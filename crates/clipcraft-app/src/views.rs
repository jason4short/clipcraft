//! The one screen: the compressor setting, the queue of movies, and what
//! the compressor is doing. The whole window takes dropped movies and
//! folders, and ⌘V pastes them. Two windows open over it: a preset's
//! settings, and Find.
//!
//! Each function is a pure reading of [`ClipCraft`], so what is on screen
//! is always exactly what the state says.

use std::fmt::Write as _;

use std::path::Path;

use clipcraft_core::job::Outcome;
use clipcraft_core::options::{
    Audio, Codec, Container, Denoise, MAX_AT_ONCE, MIN_SAVINGS, MaxSize,
    Options, Speed, command_line,
};
use clipcraft_core::presets::Saved;
use clipcraft_core::size::human_bytes;
use gpui_kit::base::CheckboxState;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, AppContext as _, ClickEvent, Context, Div, ElementId,
    ExternalPaths, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement, Render, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled, Window, div, relative,
};
use gpui_omarchy::{
    ActiveTheme, Button, ButtonVariant, IconName, MenuItem, Theme, button,
    checkbox, dialog, dialog_popup, dialog_title, input, menu, select,
};

use crate::state::{
    ClipCraft, DraggedRow, FINDER_SHOWN, Row, Status, file_name,
};
use crate::ui::{size, space, text};
use crate::{Copy, Paste, RemoveSelected, SelectAll};

/// The whole window.
pub fn root(
    app: &ClipCraft,
    window: &mut Window,
    cx: &mut Context<'_, ClipCraft>,
) -> Stateful<Div> {
    let theme = cx.omarchy().clone();
    let column = div()
        .flex()
        .flex_col()
        .size_full()
        .gap(space::LG)
        .p(space::XL)
        .child(header(&theme))
        .child(controls(app, &theme, window, cx))
        .child(status_line(app, &theme))
        .when_some(app.notice.clone(), |this, notice| {
            this.child(
                div()
                    .text_color(theme.warning)
                    .child(SharedString::from(notice)),
            )
        })
        .when(!app.rows.is_empty(), |this| {
            this.child(list_bar(app, &theme, cx))
        })
        .child(queue(app, &theme, cx))
        .child(actions(app, &theme, cx));

    let drop_highlight = theme.selection;
    div()
        .id("clipcraft-root")
        .debug_selector(|| "clipcraft-root".into())
        .key_context("ClipCraft")
        .track_focus(&app.focus)
        .on_action(cx.listener(|this, _: &Paste, _, cx| this.paste(cx)))
        .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy_selected(cx)))
        .on_action(
            cx.listener(|this, _: &SelectAll, _, cx| this.select_all(cx)),
        )
        .on_action(cx.listener(|this, _: &RemoveSelected, _, cx| {
            this.remove_selected(cx);
        }))
        .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| {
            this.add_paths(paths.paths(), cx);
        }))
        .drag_over::<ExternalPaths>(move |style, _, _, _| {
            style.bg(drop_highlight)
        })
        .size_full()
        .bg(theme.background)
        .text_color(theme.foreground)
        .font_family(theme.font.clone())
        .text_size(text::BODY)
        .child(column)
        .when_some(app.settings.clone(), |this, draft| {
            this.child(settings_dialog(app, &draft, &theme, cx))
        })
        .when(app.finder.is_some(), |this| {
            this.child(finder_dialog(app, &theme, window, cx))
        })
        .when(app.sampler.is_some(), |this| {
            this.child(sampler_dialog(app, &theme, cx))
        })
}

fn header(theme: &Theme) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(space::XS)
        .child(
            div()
                .text_size(text::HEADING)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.bright)
                .child("clipcraft"),
        )
        .child(dim(
            "Drop movies or folders anywhere in this window, or paste their \
             paths with ⌘V. Add more while it runs: they join the queue, which \
             stops once everything is done.",
            theme,
        ))
}

/// The preset, its menu, and what it does.
fn controls(
    app: &ClipCraft,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<'_, ClipCraft>,
) -> Div {
    let preset = app.preset().clone();
    div()
        .flex()
        .flex_col()
        .gap(space::SM)
        .child(preset_picker(app, window, cx))
        .child(dim(describe_preset(&preset), theme))
        .child(originals(&preset, theme))
}

/// Under the list: adding movies, tidying the list, and running it.
fn actions(app: &ClipCraft, theme: &Theme, cx: &Context<'_, ClipCraft>) -> Div {
    let has_rows = !app.rows.is_empty();
    let has_finished = app.rows.iter().any(Row::finished);
    let waiting = app.rows.iter().any(|row| row.status == Status::Waiting);
    // The one button that sets everything going: filled, and bigger than
    // the rest.
    let (run_button, fill) = if app.running() {
        (
            button("stop", "Stop", ButtonVariant::Danger, cx)
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx))),
            theme.danger,
        )
    } else {
        (
            button("start", "Start", ButtonVariant::Primary, cx)
                .disabled(!waiting)
                .on_click(cx.listener(|this, _, _, cx| this.start(cx))),
            theme.accent,
        )
    };
    let run_button = run_button
        .min_w(size::RUN_W)
        .px(space::XL)
        .py(space::SM)
        .bg(fill)
        .border_color(fill)
        .text_color(theme.background)
        .text_size(text::HEADING)
        .font_weight(FontWeight::BOLD);
    div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(space::SM)
        .child(
            button("add", "Add…", ButtonVariant::Outline, cx)
                .on_click(cx.listener(|_, _, _, cx| ClipCraft::pick(cx))),
        )
        .child(
            button("find", "Find…", ButtonVariant::Outline, cx).on_click(
                cx.listener(|this, _, window, cx| this.open_finder(window, cx)),
            ),
        )
        .child(
            button("paste", "Paste", ButtonVariant::Outline, cx)
                .on_click(cx.listener(|this, _, _, cx| this.paste(cx))),
        )
        .child(
            button("copy", "Copy list", ButtonVariant::Outline, cx)
                .disabled(!has_rows)
                .on_click(cx.listener(|this, _, _, cx| this.copy_list(cx))),
        )
        .child(
            button("clear", "Clear finished", ButtonVariant::Outline, cx)
                .disabled(!has_finished)
                .on_click(
                    cx.listener(|this, _, _, cx| this.clear_finished(cx)),
                ),
        )
        .child(
            button("try", "Try settings…", ButtonVariant::Outline, cx)
                .disabled(!has_rows)
                .on_click(cx.listener(|this, _, window, cx| {
                    this.open_sampler(window, cx);
                })),
        )
        .child(div().flex_1())
        .child({
            let entity = cx.entity();
            let state = if app.sleep_when_done {
                CheckboxState::Checked
            } else {
                CheckboxState::Unchecked
            };
            checkbox("sleep", "Sleep when done", state, cx).on_change(
                move |_, _, _, cx| {
                    entity.update(cx, ClipCraft::toggle_sleep_when_done);
                },
            )
        })
        .child(run_button)
}

/// What the preset does, in a sentence.
fn describe_preset(preset: &Saved) -> String {
    format!("ffmpeg: {}.", summary(preset.options))
}

/// What happens to the originals: kept, or replaced once checked. On its
/// own line, so it is never lost at the end of the description.
fn originals(preset: &Saved, theme: &Theme) -> Div {
    let line = div().text_size(text::CAPTION);
    if preset.replace {
        let named = match preset.options.container {
            Container::Same => String::new(),
            other => format!(" as {}", other.label()),
        };
        line.text_color(theme.warning)
            .child(SharedString::from(format!(
                "Replaces originals: once the new file plays for the same \
             length, the original is deleted and the new file takes its \
             name{named}."
            )))
    } else {
        line.text_color(theme.secondary.opacity(0.75)).child(
            "Keeps originals: the new file is saved next to each as \
             name_compressed.",
        )
    }
}

/// The options in a few words: "HEVC 10-bit, on the GPU, Quality 65, ...".
fn summary(options: Options) -> String {
    let mut parts = vec![if options.ten_bit() {
        format!("{} 10-bit", options.codec.label())
    } else {
        options.codec.label().to_string()
    }];
    parts.push(if options.gpu {
        "on the GPU".to_string()
    } else {
        format!("on the CPU, {}", options.speed.name())
    });
    parts.push(options.quality_label());
    parts.push(options.audio.label());
    parts.push(match options.container {
        Container::Same => "same file type".to_string(),
        other => other.label().to_string(),
    });
    if options.keep_dates {
        parts.push("keeps dates".into());
    }
    if options.skip_hevc {
        parts.push("skips HEVC".into());
    }
    parts.join(", ")
}

/// The quality buttons and readout. The GPU and CPU encoders count in
/// opposite directions, so the buttons say what they do instead of − and +;
/// `nudge` gets `true` for better quality.
fn stepper(
    id: &'static str,
    options: Options,
    theme: &Theme,
    cx: &Context<'_, ClipCraft>,
    nudge: fn(&mut ClipCraft, bool, &mut Context<'_, ClipCraft>),
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::XS)
        .child(
            button((id, 0_usize), "Smaller file", ButtonVariant::Outline, cx)
                .disabled(!options.can_nudge(false))
                .on_click(
                    cx.listener(move |this, _, _, cx| nudge(this, false, cx)),
                ),
        )
        .child(
            div()
                .min_w(size::CRF_W)
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .text_color(theme.bright)
                        .child(SharedString::from(options.quality_label())),
                )
                .child(dim(options.quality_word(), theme)),
        )
        .child(
            button((id, 1_usize), "Better quality", ButtonVariant::Outline, cx)
                .disabled(!options.can_nudge(true))
                .on_click(
                    cx.listener(move |this, _, _, cx| nudge(this, true, cx)),
                ),
        )
}

/// The presets dropdown with its menu, or, while renaming, the name box:
/// Enter or Save keeps the name, Esc or Cancel drops it.
fn preset_picker(
    app: &ClipCraft,
    window: &mut Window,
    cx: &mut Context<'_, ClipCraft>,
) -> Div {
    let row = div().flex().items_center().gap(space::SM);
    if let Some(name) = &app.renaming {
        return row
            .on_key_down(cx.listener(
                |this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.finish_rename(false, window, cx);
                    }
                },
            ))
            .child(div().w(size::SELECT_W).child(input(
                "preset-name",
                name,
                window,
                cx,
            )))
            .child(
                button("rename-save", "Save", ButtonVariant::Primary, cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.finish_rename(true, window, cx);
                    })),
            )
            .child(
                button("rename-cancel", "Cancel", ButtonVariant::Outline, cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.finish_rename(false, window, cx);
                    })),
            );
    }
    let entity = cx.entity();
    let items = vec![
        MenuItem::new("Settings…"),
        MenuItem::new("Rename"),
        MenuItem::new("Duplicate"),
        MenuItem::new("Delete").disabled(app.presets.list.len() < 2),
        MenuItem::new("Replace originals")
            .checked(app.preset().replace)
            .separator_before(),
    ];
    let theme = cx.omarchy().clone();
    let trigger = button("preset-menu", "Preset", ButtonVariant::Outline, cx)
        .styles(|styles| styles.selected(|style| style.bg(theme.border)))
        .child(gpui_omarchy::icon(IconName::ChevronDown));
    row.child(div().w(size::SELECT_W).child(select(
        "preset",
        &app.choice,
        window,
        cx,
    )))
    .child(menu(
        "preset-actions",
        trigger,
        items,
        move |index, window, cx| {
            entity.update(cx, |this, cx| match index {
                0 => this.open_settings(window, cx),
                1 => this.start_rename(window, cx),
                2 => this.duplicate_preset(window, cx),
                3 => this.delete_preset(window, cx),
                _ => this.toggle_replace(cx),
            });
        },
    ))
}

/// What the compressor is doing, in one line.
fn status_line(app: &ClipCraft, theme: &Theme) -> Div {
    let totals = app.totals();
    let (line, color): (String, Hsla) = if let Some(row) = app.current() {
        let others = app
            .rows
            .iter()
            .filter(|row| matches!(row.status, Status::Compressing { .. }))
            .count()
            - 1;
        let mut parts = vec![if others > 0 {
            format!("Compressing {} and {others} more", file_name(&row.path))
        } else {
            format!("Compressing {}", file_name(&row.path))
        }];
        if let Status::Compressing {
            fraction,
            eta,
            speed,
        } = &row.status
        {
            parts.extend(fraction.map(percent));
            parts.extend(speed.clone());
            parts.extend(eta.map(|secs| format!("{} left", clock(secs))));
        }
        let more = totals.waiting.saturating_sub(others + 1);
        if more > 0 {
            parts.push(format!("{more} waiting"));
        }
        if let Some(estimate) = app.estimate() {
            parts.push(format!(
                "≈ {} from {}, done around {}",
                human_bytes(estimate.bytes),
                human_bytes(estimate.from),
                estimate.finish.format("%-I:%M %p")
            ));
        }
        (parts.join("  ·  "), theme.accent)
    } else if app.running() {
        ("Finishing up.".into(), theme.foreground)
    } else if totals.waiting > 0 {
        (
            format!("{} ready. Press Start.", movies(totals.waiting)),
            theme.foreground,
        )
    } else if totals.compressed + totals.skipped + totals.failed > 0 {
        let mut line = format!("Finished: {} compressed", totals.compressed);
        if totals.compressed > 0 {
            let _ = write!(
                line,
                ", {} → {}",
                human_bytes(totals.before),
                human_bytes(totals.after)
            );
        }
        if totals.skipped > 0 {
            let _ = write!(line, ", {} skipped", totals.skipped);
        }
        if totals.failed > 0 {
            let _ = write!(line, ", {} not done", totals.failed);
        }
        (line, theme.success)
    } else {
        (String::new(), theme.foreground)
    };
    div()
        .min_h(text::HEADING)
        .font_weight(FontWeight::MEDIUM)
        .text_color(color)
        .child(SharedString::from(line))
}

/// Above the list: sorting, and how to work the rows.
fn list_bar(
    app: &ClipCraft,
    theme: &Theme,
    cx: &Context<'_, ClipCraft>,
) -> Div {
    let next = if app.largest_first {
        "Sort by size: smallest first"
    } else {
        "Sort by size: largest first"
    };
    div()
        .flex()
        .items_center()
        .gap(space::MD)
        .child(
            button("sort-size", next, ButtonVariant::Outline, cx)
                .on_click(cx.listener(|this, _, _, cx| this.sort_by_size(cx))),
        )
        .child(dim(
            "Drag rows to reorder the queue. Click to select (⌘ and ⇧ for \
             more, ⌘A for all). ⌘C copies the paths; Delete removes them, \
             except the one compressing. Click a folder to show it in Finder.",
            theme,
        ))
}

fn queue(
    app: &ClipCraft,
    theme: &Theme,
    cx: &Context<'_, ClipCraft>,
) -> Stateful<Div> {
    let list = div()
        .id("queue")
        .flex_1()
        .min_h(size::DROP_H)
        .overflow_y_scroll()
        .border_1()
        .border_color(theme.border)
        .bg(theme.inset);
    if app.rows.is_empty() {
        return list.child(
            div()
                .size_full()
                .min_h(size::DROP_H)
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(space::SM)
                .child(
                    div()
                        .text_size(text::HEADING)
                        .text_color(theme.secondary)
                        .child("Drop movies here"),
                )
                .child(dim(
                    "or folders of them, or paste paths with ⌘V",
                    theme,
                )),
        );
    }
    list.children(
        app.rows.iter().map(|row| {
            row_view(row, app.selected.contains(&row.id), theme, cx)
        }),
    )
}

impl Render for DraggedRow {
    fn render(
        &mut self,
        _: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = cx.omarchy();
        div()
            .px(space::MD)
            .py(space::SM)
            .border_1()
            .border_color(theme.accent)
            .bg(theme.background)
            .text_color(theme.bright)
            .font_family(theme.font.clone())
            .text_size(text::BODY)
            .child(SharedString::from(self.name.clone()))
    }
}

#[allow(clippy::too_many_lines)]
fn row_view(
    row: &Row,
    selected: bool,
    theme: &Theme,
    cx: &Context<'_, ClipCraft>,
) -> Stateful<Div> {
    let (label, color) = match &row.status {
        Status::Waiting => ("waiting".to_string(), theme.secondary),
        Status::Compressing { fraction, .. } => (
            fraction.map_or_else(|| "starting".into(), percent),
            theme.accent,
        ),
        Status::Finished(Outcome::Compressed { .. }) => {
            ("done".into(), theme.success)
        }
        Status::Finished(Outcome::Skipped(_)) => {
            ("skipped".into(), theme.warning)
        }
        Status::Finished(Outcome::Failed(_)) => ("failed".into(), theme.danger),
        Status::Finished(Outcome::Stopped) => ("stopped".into(), theme.warning),
    };
    let folder = row
        .path
        .parent()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    let id = row.id;
    let compressing = matches!(row.status, Status::Compressing { .. });
    let dragged = DraggedRow {
        id,
        name: file_name(&row.path),
    };
    let drop_line = theme.accent;
    let path = row.path.clone();

    div()
        .id(("row", row.id))
        .flex()
        .items_center()
        .gap(space::MD)
        .px(space::MD)
        .py(space::SM)
        .border_b_1()
        .border_t_1()
        .border_color(theme.border)
        .when(compressing, |this| this.bg(theme.accent.opacity(0.12)))
        .when(selected, |this| this.bg(theme.selection))
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.select(id, event.modifiers(), window, cx);
        }))
        .on_drag(dragged, |dragged, _, _, cx| cx.new(|_| dragged.clone()))
        .drag_over::<DraggedRow>(move |style, _, _, _| {
            style.border_color(drop_line)
        })
        .on_drop(cx.listener(move |this, dragged: &DraggedRow, _, cx| {
            this.move_row(dragged.id, id, cx);
        }))
        .child(
            div()
                .w(size::STATUS_LANE)
                .flex_shrink_0()
                .text_color(color)
                .font_weight(FontWeight::MEDIUM)
                .child(SharedString::from(label)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .child(
                    div()
                        .text_color(theme.bright)
                        .truncate()
                        .child(SharedString::from(file_name(&row.path))),
                )
                .child(
                    dim(folder, theme)
                        .id(("folder", id))
                        .truncate()
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.reveal(&path, cx);
                        })),
                ),
        )
        .child(
            div()
                .w(size::SIZE_LANE)
                .flex_shrink_0()
                .text_color(theme.secondary)
                .child(SharedString::from(human_bytes(row.bytes))),
        )
        .child(detail(row, theme, cx))
        .child(
            div()
                .id(("remove", row.id))
                .flex_shrink_0()
                .px(space::SM)
                .text_color(theme.secondary)
                .when(!compressing, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.text_color(theme.danger))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.remove(id, cx);
                        }))
                        .child("✕")
                }),
        )
}

/// The right side of a row: progress, the result, or why not.
fn detail(row: &Row, theme: &Theme, cx: &Context<'_, ClipCraft>) -> Div {
    let lane = div()
        .flex()
        .flex_col()
        .gap(space::XS)
        .w(size::BAR_W * 2.0)
        .flex_shrink_0()
        .text_size(text::CAPTION);
    match &row.status {
        Status::Waiting => lane.child(dim(
            row.sent
                .as_ref()
                .map_or_else(String::new, |name| format!("queued: {name}")),
            theme,
        )),
        Status::Compressing {
            fraction,
            eta,
            speed,
        } => {
            let mut parts = Vec::new();
            parts.extend(speed.clone());
            parts.extend(eta.map(|secs| format!("{} left", clock(secs))));
            lane.child(bar(fraction.unwrap_or(0.0), theme))
                .child(dim(parts.join("  ·  "), theme))
        }
        Status::Finished(Outcome::Compressed {
            output,
            before,
            after,
        }) => {
            let saved = if *before > 0 {
                (*after as f64 / *before as f64).mul_add(-100.0, 100.0)
            } else {
                0.0
            };
            lane.child(div().text_color(theme.success).child(
                SharedString::from(format!(
                    "{} → {}  ({saved:.0}% smaller)",
                    human_bytes(*before),
                    human_bytes(*after)
                )),
            ))
            .child({
                let output = output.clone();
                dim(file_name(&output), theme)
                    .id(("output", row.id))
                    .truncate()
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.reveal(&output, cx);
                    }))
            })
        }
        Status::Finished(Outcome::Skipped(why) | Outcome::Failed(why)) => lane
            .child(
                div()
                    .text_color(theme.secondary)
                    .child(SharedString::from(why.clone())),
            ),
        Status::Finished(Outcome::Stopped) => lane,
    }
}

fn bar(fraction: f64, theme: &Theme) -> Div {
    div()
        .w(size::BAR_W * 2.0)
        .h(size::BAR_H)
        .bg(theme.border)
        .child(div().h_full().w(relative(fraction as f32)).bg(theme.accent))
}

fn dim(label: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .text_size(text::CAPTION)
        .text_color(theme.secondary.opacity(0.75))
        .child(label.into())
}

fn percent(fraction: f64) -> String {
    format!("{:.0}%", fraction * 100.0)
}

fn movies(n: usize) -> String {
    if n == 1 {
        "1 movie".into()
    } else {
        format!("{n} movies")
    }
}

/// Seconds as `1:02:03` or `2:03`.
pub fn clock(seconds: f64) -> String {
    let total = seconds.round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

// ── settings window ─────────────────────────────────────────────────────

/// Every option of the preset being edited, and the ffmpeg command they
/// make.
#[allow(clippy::too_many_lines)]
fn settings_dialog(
    app: &ClipCraft,
    draft: &Saved,
    theme: &Theme,
    cx: &mut Context<'_, ClipCraft>,
) -> impl IntoElement {
    let options = draft.options;
    let entity = cx.entity();

    let encoders = [(true, "GPU: fast"), (false, "CPU: smallest files")]
        .into_iter()
        .enumerate()
        .map(|(i, (gpu, label))| {
            pick(("encoder", i), label, options.gpu == gpu, false, cx).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.edit_settings(|draft| draft.options.gpu = gpu, cx);
                }),
            )
        });
    let codecs = Codec::ALL.iter().enumerate().map(|(i, &codec)| {
        pick(
            ("codec", i),
            codec.label(),
            options.codec == codec,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.codec = codec, cx);
        }))
    });
    let depths = [false, true].into_iter().enumerate().map(|(i, ten)| {
        let label = if ten { "10-bit" } else { "8-bit" };
        let blocked = ten && options.codec != Codec::Hevc;
        pick(("depth", i), label, options.ten_bit() == ten, blocked, cx)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.edit_settings(|draft| draft.options.ten_bit = ten, cx);
            }))
    });
    let speeds = Speed::ALL.iter().enumerate().map(|(i, &speed)| {
        pick(
            ("speed", i),
            speed.name(),
            options.speed == speed && !options.gpu,
            options.gpu,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.speed = speed, cx);
        }))
    });
    let audios = Audio::ALL.iter().enumerate().map(|(i, &audio)| {
        pick(
            ("audio", i),
            audio.label(),
            options.audio == audio,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.audio = audio, cx);
        }))
    });
    let containers = Container::ALL.iter().enumerate().map(|(i, &file)| {
        pick(
            ("file", i),
            file.label(),
            options.container == file,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.container = file, cx);
        }))
    });

    let denoises = Denoise::ALL.iter().enumerate().map(|(i, &denoise)| {
        pick(
            ("denoise", i),
            denoise.label(),
            options.denoise == denoise,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.denoise = denoise, cx);
        }))
    });
    let sizes = MaxSize::ALL.iter().enumerate().map(|(i, &size)| {
        pick(
            ("size", i),
            size.label(),
            options.max_size == size,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.max_size = size, cx);
        }))
    });
    let savings = MIN_SAVINGS.iter().enumerate().map(|(i, &saving)| {
        let label = if saving == 0 {
            "Always".to_string()
        } else {
            format!("{saving}% smaller")
        };
        pick(
            ("saving", i),
            label,
            options.min_saving == saving,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.min_saving = saving, cx);
        }))
    });
    let at_once = (1..=MAX_AT_ONCE).map(|n| {
        pick(
            ("at-once", usize::from(n)),
            n.to_string(),
            options.at_once == n,
            false,
            cx,
        )
        .on_click(cx.listener(move |this, _, _, cx| {
            this.edit_settings(|draft| draft.options.at_once = n, cx);
        }))
    });

    let flag = |id: &'static str,
                label: &'static str,
                on: bool,
                edit: fn(&mut Saved)| {
        let entity = entity.clone();
        let state = if on {
            CheckboxState::Checked
        } else {
            CheckboxState::Unchecked
        };
        checkbox(id, label, state, cx).on_change(move |_, _, _, cx| {
            entity.update(cx, |this, cx| this.edit_settings(edit, cx));
        })
    };
    let flags = div()
        .flex()
        .flex_wrap()
        .gap(space::LG)
        .child(flag(
            "keep-dates",
            "Keep the original's dates",
            options.keep_dates,
            |draft| draft.options.keep_dates = !draft.options.keep_dates,
        ))
        .child(flag(
            "skip-hevc",
            "Skip movies already in HEVC",
            options.skip_hevc,
            |draft| draft.options.skip_hevc = !draft.options.skip_hevc,
        ))
        .child(flag(
            "draft-replace",
            "Replace originals",
            draft.replace,
            |draft| draft.replace = !draft.replace,
        ));

    let input = Path::new("IN.mov");
    let output = options.output(input);
    let mut command =
        command_line("ffmpeg", &options.ffmpeg_args(input, &output, true));
    if draft.replace {
        let result = options.result(input, true);
        let _ = write!(
            command,
            "\nthen, if the lengths match: delete IN.mov, rename to {}",
            result.display()
        );
    }

    let body = div()
        .flex()
        .flex_col()
        .gap(space::MD)
        .child(dialog_title(format!("Settings: {}", draft.name), cx))
        .child(setting("Encoder", encoders, theme))
        .child(setting("Video", codecs, theme))
        .child(setting("Color", depths, theme))
        .child(setting("Speed", speeds, theme))
        .child(setting("Sound", audios, theme))
        .child(setting("File", containers, theme))
        .child(setting("Denoise", denoises, theme))
        .child(setting("Max size", sizes, theme))
        .child(setting("Keep if", savings, theme))
        .child(setting("At once", at_once, theme))
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::MD)
                .child(label_lane("Quality", theme))
                .child(stepper(
                    "draft-quality",
                    options,
                    theme,
                    cx,
                    |this, better, cx| {
                        this.edit_settings(
                            |draft| draft.options.nudge_quality(better),
                            cx,
                        );
                    },
                )),
        )
        .child(flags)
        .child(dim(
            "The command each movie gets, with IN.mov for its name:",
            theme,
        ))
        .child(
            div()
                .p(space::SM)
                .bg(theme.inset)
                .border_1()
                .border_color(theme.border)
                .font_family("Menlo")
                .text_size(text::CAPTION)
                .text_color(theme.secondary)
                .child(SharedString::from(command)),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap(space::SM)
                .child(
                    button(
                        "settings-cancel",
                        "Cancel",
                        ButtonVariant::Outline,
                        cx,
                    )
                    .on_click(cx.listener(
                        |this, _, window, cx| {
                            this.close_settings(false, window, cx);
                        },
                    )),
                )
                .child(
                    button("settings-save", "Save", ButtonVariant::Primary, cx)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_settings(true, window, cx);
                        })),
                ),
        );

    let popup = div()
        .id("settings-surface")
        .occlude()
        .max_w(relative(0.95))
        .child(dialog_popup(cx).w(size::SETTINGS_W).child(body));
    let close = cx.listener(|this, keep: &bool, window, cx| {
        this.close_settings(*keep, window, cx);
    });
    dialog(&app.settings_focus, cx)
        .popup(popup)
        .request_close(move |keep, window, cx| close(&keep, window, cx))
}

/// One button of a set where one is chosen; `blocked` ones cannot be
/// picked.
fn pick(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    chosen: bool,
    blocked: bool,
    cx: &Context<'_, ClipCraft>,
) -> Button {
    let variant = if chosen {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Outline
    };
    button(id, label, variant, cx).disabled(blocked && !chosen)
}

/// A setting's name and its buttons.
fn setting(
    label: &'static str,
    buttons: impl Iterator<Item = Button>,
    theme: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(space::MD)
        .child(label_lane(label, theme))
        .child(div().flex().flex_wrap().gap(space::XS).children(buttons))
}

fn label_lane(label: &'static str, theme: &Theme) -> Div {
    div()
        .w(size::LABEL_W)
        .flex_shrink_0()
        .text_color(theme.secondary)
        .child(label)
}

// ── find window ─────────────────────────────────────────────────────────

/// Movies in one folder and below it, narrowed by what is typed. A click
/// adds one; Add all adds every match.
#[allow(clippy::too_many_lines)]
fn finder_dialog(
    app: &ClipCraft,
    theme: &Theme,
    window: &Window,
    cx: &mut Context<'_, ClipCraft>,
) -> AnyElement {
    let Some(finder) = &app.finder else {
        return div().into_any_element();
    };
    let matches = app.finder_matches(cx);
    let new: Vec<_> = matches
        .iter()
        .filter(|(path, _)| !app.listed(path))
        .map(|(path, _)| path.clone())
        .collect();
    let folder = finder.folder.clone();
    let status = if finder.searching {
        "Looking…".to_string()
    } else if folder.is_none() {
        "Choose a folder to look in.".to_string()
    } else if finder.found.is_empty() {
        "No movies in that folder.".to_string()
    } else {
        let total: u64 = matches.iter().map(|(_, bytes)| bytes).sum();
        let mut line = format!(
            "{} of {} match, {}",
            matches.len(),
            movies_word(finder.found.len()),
            human_bytes(total)
        );
        if matches.len() > FINDER_SHOWN {
            let _ = write!(line, ", the first {FINDER_SHOWN} shown");
        }
        line
    };

    let results = div()
        .id("finder-results")
        .h(size::FINDER_H)
        .overflow_y_scroll()
        .border_1()
        .border_color(theme.border)
        .bg(theme.inset)
        .children(matches.iter().take(FINDER_SHOWN).enumerate().map(
            |(i, (path, bytes))| {
                let listed = app.listed(path);
                let below = folder
                    .as_deref()
                    .and_then(|folder| path.parent()?.strip_prefix(folder).ok())
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_default();
                let add = path.clone();
                div()
                    .id(("found", i))
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .px(space::MD)
                    .py(space::XS)
                    .border_b_1()
                    .border_color(theme.border)
                    .when(!listed, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(theme.selection))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_paths(std::slice::from_ref(&add), cx);
                            }))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_color(theme.bright)
                                    .truncate()
                                    .child(SharedString::from(file_name(path))),
                            )
                            .when(!below.is_empty(), |this| {
                                this.child(dim(below, theme).truncate())
                            }),
                    )
                    .child(
                        div()
                            .w(size::SIZE_LANE)
                            .flex_shrink_0()
                            .text_color(theme.secondary)
                            .child(SharedString::from(human_bytes(*bytes))),
                    )
                    .child(
                        div()
                            .w(size::STATUS_LANE)
                            .flex_shrink_0()
                            .text_color(if listed {
                                theme.secondary
                            } else {
                                theme.accent
                            })
                            .child(if listed { "in list" } else { "Add" }),
                    )
            },
        ));

    let body = div()
        .flex()
        .flex_col()
        .gap(space::MD)
        .child(dialog_title("Find movies", cx))
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::SM)
                .child(
                    div().flex_1().min_w_0().child(
                        dim(
                            folder.as_ref().map_or_else(
                                || "No folder chosen".to_string(),
                                |folder| folder.display().to_string(),
                            ),
                            theme,
                        )
                        .truncate(),
                    ),
                )
                .child(
                    button(
                        "finder-folder",
                        "Choose folder…",
                        ButtonVariant::Outline,
                        cx,
                    )
                    .on_click(cx.listener(
                        |_, _, _, cx| {
                            ClipCraft::choose_finder_folder(cx);
                        },
                    )),
                ),
        )
        .child(input("finder-query", &finder.query, window, cx))
        .child(dim(status, theme))
        .child(results)
        .child(
            div()
                .flex()
                .justify_end()
                .gap(space::SM)
                .child(
                    button("finder-done", "Done", ButtonVariant::Outline, cx)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.close_finder(window, cx);
                        })),
                )
                .child(
                    button(
                        "finder-add-all",
                        format!("Add {}", movies_word(new.len())),
                        ButtonVariant::Primary,
                        cx,
                    )
                    .disabled(new.is_empty())
                    .on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.add_paths(&new, cx);
                        },
                    )),
                ),
        );

    let popup = div()
        .id("finder-surface")
        .occlude()
        .max_w(relative(0.95))
        .child(dialog_popup(cx).w(size::FINDER_W).child(body));
    let close = cx.listener(|this, _: &bool, window, cx| {
        this.close_finder(window, cx);
    });
    dialog(&app.finder_focus, cx)
        .popup(popup)
        .request_close(move |keep, window, cx| close(&keep, window, cx))
        .into_any_element()
}

fn movies_word(n: usize) -> String {
    movies(n)
}

// ── sample window ───────────────────────────────────────────────────────

/// Samples of one movie: the original's seconds and the preset a step
/// either side, with what the whole movie would come to at each.
fn sampler_dialog(
    app: &ClipCraft,
    theme: &Theme,
    cx: &mut Context<'_, ClipCraft>,
) -> AnyElement {
    let Some(sampler) = &app.sampler else {
        return div().into_any_element();
    };
    let mut body = div()
        .flex()
        .flex_col()
        .gap(space::MD)
        .child(dialog_title("Try settings", cx))
        .child(dim(
            format!(
                "{} seconds of {}, at {} and a step either side.",
                clipcraft_core::sample::SECONDS,
                file_name(&sampler.input),
                app.preset().options.quality_label()
            ),
            theme,
        ));
    let mut watch = false;
    match &sampler.result {
        None => body = body.child("Encoding samples…"),
        Some(Err(why)) => {
            body = body.child(
                div()
                    .text_color(theme.danger)
                    .child(SharedString::from(why.clone())),
            );
        }
        Some(Ok(samples)) => {
            watch = true;
            for (i, sample) in samples.iter().enumerate() {
                let mut row = div()
                    .flex()
                    .items_center()
                    .gap(space::MD)
                    .child(label_lane_owned(sample.label.clone(), theme))
                    .child(
                        div()
                            .w(size::SIZE_LANE)
                            .text_color(theme.secondary)
                            .child(SharedString::from(human_bytes(
                                sample.bytes,
                            ))),
                    )
                    .child(div().flex_1().text_color(theme.bright).child(
                        SharedString::from(sample.projected.map_or_else(
                            String::new,
                            |bytes| {
                                format!("whole movie ≈ {}", human_bytes(bytes))
                            },
                        )),
                    ));
                if let Some(options) = sample.options {
                    row = row.child(
                        button(
                            ("use-sample", i),
                            "Use this",
                            ButtonVariant::Outline,
                            cx,
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.use_sample(options, window, cx);
                            },
                        )),
                    );
                }
                body = body.child(row);
            }
        }
    }
    body = body.child(
        div()
            .flex()
            .justify_end()
            .gap(space::SM)
            .child(
                button("sample-close", "Close", ButtonVariant::Outline, cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_sampler(window, cx);
                    })),
            )
            .child(
                button(
                    "sample-watch",
                    "Watch in QuickTime",
                    ButtonVariant::Primary,
                    cx,
                )
                .disabled(!watch)
                .on_click(cx.listener(|this, _, _, cx| this.watch_samples(cx))),
            ),
    );
    let popup = div()
        .id("sampler-surface")
        .occlude()
        .max_w(relative(0.95))
        .child(dialog_popup(cx).w(size::SETTINGS_W).child(body));
    let close = cx.listener(|this, _: &bool, window, cx| {
        this.close_sampler(window, cx);
    });
    dialog(&app.sampler_focus, cx)
        .popup(popup)
        .request_close(move |keep, window, cx| close(&keep, window, cx))
        .into_any_element()
}

fn label_lane_owned(label: String, theme: &Theme) -> Div {
    div()
        .w(size::LABEL_W)
        .flex_shrink_0()
        .text_color(theme.secondary)
        .child(SharedString::from(label))
}
