//! Application state and every mutation the UI can perform.
//!
//! The screen in [`crate::views`] is a pure function of this state; what a
//! button does, and when it is allowed to, is decided here.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clipcraft_core::job::{JobEvent, JobHandle, Outcome, Settings, Task};
use clipcraft_core::options::Options;
use clipcraft_core::presets::{Presets, Saved};
use clipcraft_core::sample::{self, Sample};
use clipcraft_core::size::human_bytes;
use clipcraft_core::{paste, scan, tools};
use gpui_kit::base::input::{InputEvent, InputState};
use gpui_kit::{
    AppContext as _, ClipboardEntry, ClipboardItem, Context, Entity,
    FocusHandle, Modifiers, PathPromptOptions, PromptLevel, Window,
};
use gpui_omarchy::{ChoiceItem, ChoiceState};

/// Where one movie is in the queue.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Waiting,
    Compressing {
        fraction: Option<f64>,
        eta: Option<f64>,
        speed: Option<String>,
    },
    Finished(Outcome),
}

/// One movie in the list.
#[derive(Clone, Debug)]
pub struct Row {
    pub id: usize,
    pub path: PathBuf,
    pub bytes: u64,
    /// The preset it was handed to the compressor with; `None` until then.
    pub sent: Option<String>,
    /// Where it is being written, once handed over.
    pub output: Option<PathBuf>,
    pub status: Status,
}

impl Row {
    pub const fn finished(&self) -> bool {
        matches!(self.status, Status::Finished(_))
    }
}

/// A row as kept between runs, in `list.json` next to the presets. A
/// movie that was compressing comes back waiting.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SavedRow {
    path: PathBuf,
    bytes: u64,
    #[serde(default)]
    outcome: Option<Outcome>,
}

/// Where the list is kept between runs.
pub fn default_list_path() -> Option<PathBuf> {
    Presets::default_path().map(|path| path.with_file_name("list.json"))
}

/// The list saved at `path`; empty when there is none or it is broken.
fn load_list(path: &Path) -> Vec<SavedRow> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// A row being dragged to a new place in the list.
#[derive(Clone, Debug)]
pub struct DraggedRow {
    pub id: usize,
    pub name: String,
}

/// The Find window: movies in one folder, narrowed by what is typed.
pub struct Finder {
    pub folder: Option<PathBuf>,
    pub query: Entity<InputState>,
    /// Every movie in the folder and its subfolders, with its size.
    pub found: Vec<(PathBuf, u64)>,
    pub searching: bool,
}

/// The sample window: one movie, encoded a few seconds at a time.
pub struct Sampler {
    pub input: PathBuf,
    /// The samples, once made, or why not.
    pub result: Option<Result<Vec<Sample>, String>>,
}

/// One run of the queue, for its estimate.
pub struct Run {
    started: Instant,
    /// Input bytes handed to it, and of those, ended.
    sent_bytes: u64,
    ended_bytes: u64,
    /// Before and after, of the movies it compressed.
    before: u64,
    after: u64,
    /// The outputs being written now, by row, and when they were looked at.
    writing: HashMap<usize, u64>,
    looked: Instant,
}

/// Where a run is heading.
pub struct Estimate {
    /// The size of everything it makes.
    pub bytes: u64,
    /// Of what it was given.
    pub from: u64,
    /// When it will be done.
    pub finish: chrono::DateTime<chrono::Local>,
}

/// How long a run goes before its estimate is worth showing.
const SETTLE: Duration = Duration::from_secs(60);

/// How many matches the Find window lists; Add all takes every one.
pub const FINDER_SHOWN: usize = 300;

/// Totals for the status line.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub waiting: usize,
    pub compressed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub before: u64,
    pub after: u64,
}

/// Everything the app knows and everything it can do.
pub struct ClipCraft {
    pub rows: Vec<Row>,
    next_id: usize,
    pub presets: Presets,
    /// Where the presets are saved; `None` keeps them in memory (the tests).
    pub presets_path: Option<PathBuf>,
    /// The presets dropdown.
    pub choice: Entity<ChoiceState>,
    /// The rename box, while the current preset is being renamed.
    pub renaming: Option<Entity<InputState>>,
    /// The running queue; `None` when stopped.
    pub handle: Option<JobHandle>,
    /// The last thing worth telling the user that is not one movie's.
    pub notice: Option<String>,
    /// Where runs are logged; `None` logs nothing (the tests).
    pub log_path: Option<PathBuf>,
    pub focus: FocusHandle,
    /// The selected rows, by id, in the order they were picked.
    pub selected: Vec<usize>,
    /// Where a shift-click selects from.
    anchor: Option<usize>,
    /// The last size sort was largest first.
    pub largest_first: bool,
    /// The settings window's copy of the current preset, while it is open.
    pub settings: Option<Saved>,
    pub settings_focus: FocusHandle,
    /// The Find window, while it is open.
    pub finder: Option<Finder>,
    pub finder_focus: FocusHandle,
    /// Where the list is kept between runs; `None` keeps it in memory.
    list_path: Option<PathBuf>,
    /// The queue has been told to stop and is finishing.
    stopping: bool,
    /// It stopped because everything was done, not because of Stop.
    all_done: bool,
    /// Put the Mac to sleep once everything is done.
    pub sleep_when_done: bool,
    /// `caffeinate`, keeping the Mac awake while the queue runs.
    awake: Option<std::process::Child>,
    pub run: Option<Run>,
    /// The sample window, while it is open.
    pub sampler: Option<Sampler>,
    pub sampler_focus: FocusHandle,
    /// The list as last written there.
    saved_list: Vec<SavedRow>,
}

impl ClipCraft {
    pub fn new(
        presets_path: Option<PathBuf>,
        list_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let saved_list =
            list_path.as_deref().map(load_list).unwrap_or_default();
        let rows: Vec<Row> = saved_list
            .iter()
            .enumerate()
            .map(|(id, saved)| Row {
                id,
                path: saved.path.clone(),
                bytes: saved.bytes,
                sent: None,
                output: None,
                status: saved
                    .outcome
                    .clone()
                    .map_or(Status::Waiting, Status::Finished),
            })
            .collect();
        // Whatever changes the list shows on screen, so a change seen here
        // is saved.
        cx.observe_self(|this, _| this.save_list()).detach();
        let presets = presets_path
            .as_deref()
            .map(Presets::load)
            .unwrap_or_default();
        let choice = Self::choice(&presets, window, cx);
        // ffmpeg runs in its own process group, so quitting would leave
        // ffmpeg running unless it is stopped here.
        cx.on_app_quit(|this, _| {
            this.stop_now();
            async {}
        })
        .detach();
        Self {
            next_id: rows.len(),
            rows,
            presets,
            presets_path,
            choice,
            renaming: None,
            handle: None,
            notice: None,
            log_path: crate::log::default_path(),
            focus: cx.focus_handle(),
            selected: Vec::new(),
            anchor: None,
            largest_first: false,
            settings: None,
            settings_focus: cx.focus_handle(),
            finder: None,
            finder_focus: cx.focus_handle(),
            list_path,
            saved_list,
            stopping: false,
            all_done: false,
            sleep_when_done: false,
            awake: None,
            run: None,
            sampler: None,
            sampler_focus: cx.focus_handle(),
        }
    }

    /// The dropdown over `presets`, by index, following the user's pick.
    fn choice(
        presets: &Presets,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Entity<ChoiceState> {
        let items = presets
            .list
            .iter()
            .enumerate()
            .map(|(index, saved)| {
                ChoiceItem::new(index.to_string(), saved.name.clone())
            })
            .collect();
        let selected = presets.selected;
        let choice = cx.new(|cx| {
            ChoiceState::new(items, window, cx)
                .label("Preset")
                .default_selected(selected)
        });
        cx.observe(&choice, |this, choice, cx| {
            let picked = choice
                .read(cx)
                .selected()
                .and_then(|item| item.value.parse::<usize>().ok());
            if let Some(index) = picked
                && index != this.presets.selected
            {
                this.presets.selected = index;
                this.save_presets();
            }
            cx.notify();
        })
        .detach();
        choice
    }

    /// Write the list to `list.json` if it changed since last time.
    fn save_list(&mut self) {
        let Some(path) = &self.list_path else {
            return;
        };
        let list: Vec<SavedRow> = self
            .rows
            .iter()
            .map(|row| SavedRow {
                path: row.path.clone(),
                bytes: row.bytes,
                outcome: match &row.status {
                    Status::Finished(outcome) => Some(outcome.clone()),
                    _ => None,
                },
            })
            .collect();
        if list == self.saved_list {
            return;
        }
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                let temp = path.with_extension("json.tmp");
                let text = serde_json::to_string_pretty(&list)?;
                std::fs::write(&temp, text)?;
                std::fs::rename(&temp, path)
            });
        if let Err(err) = written {
            self.log(&format!("cannot save the list: {err}"));
        }
        // Even when it failed: one try per change, not one per frame.
        self.saved_list = list;
    }

    pub fn log(&self, text: &str) {
        if let Some(path) = &self.log_path {
            crate::log::append(path, text);
        }
    }

    pub fn tell(&mut self, text: String) {
        self.log(&text);
        self.notice = Some(text);
    }

    pub const fn running(&self) -> bool {
        self.handle.is_some()
    }

    /// The preset in the dropdown.
    pub fn preset(&self) -> &Saved {
        self.presets.current()
    }

    /// The preset in use, as the queue reads it.
    fn settings(&self) -> Settings {
        let preset = self.preset();
        Settings {
            name: preset.name.clone(),
            replace: preset.replace,
            options: preset.options,
        }
    }

    /// Save the presets, and hand the one in use to a running queue: each
    /// movie that starts from now on gets it.
    fn save_presets(&mut self) {
        if let Some(handle) = &self.handle {
            handle.set_settings(self.settings());
        }
        let Some(path) = &self.presets_path else {
            return;
        };
        if let Err(err) = self.presets.save(path) {
            let text =
                format!("Cannot save presets to {}: {err}", path.display());
            self.tell(text);
        }
    }

    // ── presets ─────────────────────────────────────────────────────────

    /// Open the rename box on the current preset.
    pub fn start_rename(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let name = self.preset().name.clone();
        let input =
            cx.new(|cx| InputState::new(window, cx).default_value(name));
        cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.finish_rename(true, window, cx);
            }
        })
        .detach();
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        self.renaming = Some(input);
        cx.notify();
    }

    /// Close the rename box, keeping the new name when `keep`. A blank
    /// name keeps the old one.
    pub fn finish_rename(
        &mut self,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(input) = self.renaming.take() else {
            return;
        };
        let name = input.read(cx).value().to_string();
        let selected = self.presets.selected;
        if keep && self.presets.rename(selected, &name) {
            self.presets_changed(window, cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Save the presets and show the new list in the dropdown.
    fn presets_changed(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.save_presets();
        self.choice = Self::choice(&self.presets, window, cx);
        cx.notify();
    }

    /// Copy the current preset, and use the copy.
    pub fn duplicate_preset(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.presets.duplicate(self.presets.selected);
        self.presets_changed(window, cx);
    }

    /// Delete the current preset, once the user says so. The last one
    /// cannot be deleted.
    pub fn delete_preset(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        if self.presets.list.len() < 2 {
            return;
        }
        let index = self.presets.selected;
        let message = format!(
            "Delete the preset \u{201c}{}\u{201d}?",
            self.preset().name
        );
        let answer = window.prompt(
            PromptLevel::Warning,
            &message,
            Some("Movies already queued keep their settings."),
            &["Delete", "Cancel"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                if this.presets.selected == index && this.presets.delete(index)
                {
                    this.presets_changed(window, cx);
                }
            });
        })
        .detach();
    }

    // ── settings window ─────────────────────────────────────────────────

    /// Open the settings window on a copy of the current preset.
    pub fn open_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.settings = Some(self.preset().clone());
        window.focus(&self.settings_focus, cx);
        cx.notify();
    }

    /// Change the settings window's copy.
    pub fn edit_settings(
        &mut self,
        edit: impl FnOnce(&mut Saved),
        cx: &mut Context<'_, Self>,
    ) {
        if let Some(draft) = &mut self.settings {
            edit(draft);
            cx.notify();
        }
    }

    /// Close the settings window, keeping its changes when `keep`.
    pub fn close_settings(
        &mut self,
        keep: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(draft) = self.settings.take() else {
            return;
        };
        if keep && draft != *self.preset() {
            *self.presets.current_mut() = draft;
            self.save_presets();
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    // ── find window ─────────────────────────────────────────────────────

    /// Open the Find window, asking for the folder to look in.
    pub fn open_finder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let query = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Part of a name, e.g. DJI 2024")
        });
        cx.subscribe(&query, |_, _, event, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        query.update(cx, |input, cx| input.focus(window, cx));
        self.finder = Some(Finder {
            folder: None,
            query,
            found: Vec::new(),
            searching: false,
        });
        cx.notify();
        Self::choose_finder_folder(cx);
    }

    /// Ask for the folder the Find window looks in.
    pub fn choose_finder_folder(cx: &Context<'_, Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Look Here".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(folder) = paths.into_iter().next() else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.search_folder(folder, cx));
        })
        .detach();
    }

    /// List every movie under `folder`, off the main thread: a big drive
    /// takes a while.
    pub fn search_folder(
        &mut self,
        folder: PathBuf,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(finder) = &mut self.finder else {
            return;
        };
        finder.folder = Some(folder.clone());
        finder.found.clear();
        finder.searching = true;
        cx.notify();
        let search = cx.background_executor().spawn(async move {
            scan::expand(std::slice::from_ref(&folder))
                .movies
                .into_iter()
                .map(|path| {
                    let bytes = std::fs::metadata(&path).map_or(0, |m| m.len());
                    (path, bytes)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let found = search.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(finder) = &mut this.finder {
                    finder.found = found;
                    finder.searching = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The Find window's movies whose path below the folder holds every
    /// word typed, in any case.
    pub fn finder_matches(&self, cx: &gpui_kit::App) -> Vec<(PathBuf, u64)> {
        let Some(finder) = &self.finder else {
            return Vec::new();
        };
        let query = finder.query.read(cx).value().to_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        finder
            .found
            .iter()
            .filter(|(path, _)| {
                let below = finder
                    .folder
                    .as_deref()
                    .and_then(|folder| path.strip_prefix(folder).ok())
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_lowercase();
                words.iter().all(|word| below.contains(word))
            })
            .cloned()
            .collect()
    }

    /// Whether `path` is already in the list, and not finished.
    pub fn listed(&self, path: &Path) -> bool {
        self.rows
            .iter()
            .any(|row| row.path == path && !row.finished())
    }

    pub fn close_finder(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.finder = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Turn replacing originals on or off for the current preset.
    pub fn toggle_replace(&mut self, cx: &mut Context<'_, Self>) {
        let saved = self.presets.current_mut();
        saved.replace = !saved.replace;
        self.save_presets();
        cx.notify();
    }

    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        for row in &self.rows {
            match &row.status {
                Status::Waiting | Status::Compressing { .. } => {
                    totals.waiting += 1;
                }
                Status::Finished(Outcome::Compressed {
                    before, after, ..
                }) => {
                    totals.compressed += 1;
                    totals.before += before;
                    totals.after += after;
                }
                Status::Finished(Outcome::Skipped(_)) => totals.skipped += 1,
                Status::Finished(Outcome::Failed(_) | Outcome::Stopped) => {
                    totals.failed += 1;
                }
            }
        }
        totals
    }

    /// The movie being compressed now.
    pub fn current(&self) -> Option<&Row> {
        self.rows
            .iter()
            .find(|row| matches!(row.status, Status::Compressing { .. }))
    }

    // ── adding ──────────────────────────────────────────────────────────

    /// Add the movies in `paths`, folders searched all the way down. A
    /// movie already waiting or compressing is not added twice. While the
    /// queue runs, they join its end.
    pub fn add_paths(&mut self, paths: &[PathBuf], cx: &mut Context<'_, Self>) {
        let found = scan::expand(paths);
        let mut added = 0;
        for path in found.movies {
            if self.listed(&path) {
                continue;
            }
            let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());
            let id = self.next_id;
            self.next_id += 1;
            self.rows.push(Row {
                id,
                path,
                bytes,
                sent: None,
                output: None,
                status: Status::Waiting,
            });
            if self.running() {
                self.send(id);
            }
            added += 1;
        }
        let mut notes = Vec::new();
        if added == 0 && found.missing.is_empty() && found.not_movies.is_empty()
        {
            notes.push("No new movies there.".to_string());
        }
        if !found.not_movies.is_empty() {
            notes.push(format!("Not a movie: {}", names(&found.not_movies)));
        }
        if !found.missing.is_empty() {
            notes.push(format!("Not found: {}", names(&found.missing)));
        }
        self.notice = (!notes.is_empty()).then(|| notes.join("  "));
        if added > 0 {
            self.log(&format!("added {added} movie(s)"));
        }
        cx.notify();
    }

    /// Paste: files copied in Finder, or paths as text.
    pub fn paste(&mut self, cx: &mut Context<'_, Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut paths: Vec<PathBuf> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                ClipboardEntry::ExternalPaths(paths) => {
                    Some(paths.paths().to_vec())
                }
                _ => None,
            })
            .flatten()
            .collect();
        if paths.is_empty()
            && let Some(text) = item.text()
        {
            paths = paste::paths(&text);
        }
        if paths.is_empty() {
            self.notice = Some(
                "Nothing to paste: copy movies or their paths first.".into(),
            );
            cx.notify();
            return;
        }
        self.add_paths(&paths, cx);
    }

    /// Choose movies or folders with the standard file picker.
    pub fn pick(cx: &Context<'_, Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let _ = this.update(cx, |this, cx| this.add_paths(&paths, cx));
        })
        .detach();
    }

    /// Copy every movie's path, one per line.
    pub fn copy_list(&mut self, cx: &mut Context<'_, Self>) {
        let list: Vec<String> = self
            .rows
            .iter()
            .map(|row| row.path.display().to_string())
            .collect();
        cx.write_to_clipboard(ClipboardItem::new_string(list.join("\n")));
        self.notice = Some(format!("Copied {} path(s).", list.len()));
        cx.notify();
    }

    /// Select row `id`: on its own, or with ⌘ added or taken away, or with
    /// ⇧ everything from the last one picked.
    pub fn select(
        &mut self,
        id: usize,
        modifiers: Modifiers,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let position = |id| self.rows.iter().position(|row| row.id == id);
        if modifiers.shift
            && let Some(from) = self.anchor.and_then(position)
            && let Some(to) = position(id)
        {
            let (start, end) = (from.min(to), from.max(to));
            self.selected =
                self.rows[start..=end].iter().map(|row| row.id).collect();
        } else if modifiers.platform {
            if let Some(at) =
                self.selected.iter().position(|picked| *picked == id)
            {
                self.selected.remove(at);
            } else {
                self.selected.push(id);
            }
            self.anchor = Some(id);
        } else {
            self.selected = vec![id];
            self.anchor = Some(id);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<'_, Self>) {
        self.selected = self.rows.iter().map(|row| row.id).collect();
        cx.notify();
    }

    /// Copy the selected movies' paths, one per line, in list order.
    pub fn copy_selected(&mut self, cx: &mut Context<'_, Self>) {
        let list: Vec<String> = self
            .rows
            .iter()
            .filter(|row| self.selected.contains(&row.id))
            .map(|row| row.path.display().to_string())
            .collect();
        if list.is_empty() {
            self.notice = Some(
                "Click a movie to select it, then ⌘C copies its path.".into(),
            );
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(list.join("\n")));
            self.notice = Some(if list.len() == 1 {
                format!("Copied {}", list[0])
            } else {
                format!("Copied {} paths.", list.len())
            });
        }
        cx.notify();
    }

    /// Show `path` selected in a Finder window.
    /// A file that is gone shows its folder instead.
    pub fn reveal(&mut self, path: &Path, cx: &mut Context<'_, Self>) {
        let mut open = std::process::Command::new("/usr/bin/open");
        if path.exists() {
            open.arg("-R").arg(path);
        } else if let Some(folder) = path.parent().filter(|dir| dir.is_dir()) {
            open.arg(folder);
        } else {
            self.tell(format!("{} is not there any more.", path.display()));
            cx.notify();
            return;
        }
        let shown = open.spawn().and_then(|mut child| child.wait());
        if !shown.is_ok_and(|status| status.success()) {
            self.tell(format!("Cannot show {} in Finder.", path.display()));
            cx.notify();
        }
    }

    /// Show row `id`'s movie in Finder: the original, or once that has
    /// been replaced, the compressed movie that took its place.
    pub fn reveal_row(&mut self, id: usize, cx: &mut Context<'_, Self>) {
        let Some(row) = self.rows.iter().find(|row| row.id == id) else {
            return;
        };
        let path = match &row.status {
            Status::Finished(Outcome::Compressed { output, .. })
                if !row.path.exists() =>
            {
                output.clone()
            }
            _ => row.path.clone(),
        };
        self.reveal(&path, cx);
    }

    /// Sort by size, smallest first; again, largest first. The queue runs
    /// in the new order.
    pub fn sort_by_size(&mut self, cx: &mut Context<'_, Self>) {
        self.largest_first = !self.largest_first;
        if self.largest_first {
            self.rows.sort_by_key(|row| std::cmp::Reverse(row.bytes));
        } else {
            self.rows.sort_by_key(|row| row.bytes);
        }
        self.sync_queue();
        cx.notify();
    }

    /// Move row `id` to where row `to` is. The queue runs in the new order.
    pub fn move_row(
        &mut self,
        id: usize,
        to: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let from = self.rows.iter().position(|row| row.id == id);
        let to = self.rows.iter().position(|row| row.id == to);
        let (Some(from), Some(to)) = (from, to) else {
            return;
        };
        if from == to {
            return;
        }
        let row = self.rows.remove(from);
        self.rows.insert(to, row);
        self.sync_queue();
        cx.notify();
    }

    /// Put the running queue in the list's order.
    fn sync_queue(&self) {
        if let Some(handle) = &self.handle {
            let order: Vec<usize> =
                self.rows.iter().map(|row| row.id).collect();
            handle.reorder(&order);
        }
    }

    /// Take a movie off the list. One being compressed stays.
    pub fn remove(&mut self, id: usize, cx: &mut Context<'_, Self>) {
        let Some(index) = self.rows.iter().position(|row| row.id == id) else {
            return;
        };
        if matches!(self.rows[index].status, Status::Compressing { .. }) {
            return;
        }
        if let Some(handle) = &self.handle {
            handle.remove(id);
        }
        self.rows.remove(index);
        self.selected.retain(|picked| *picked != id);
        cx.notify();
    }

    /// Take the selected movies off the list, except the one being
    /// compressed, which stays selected.
    pub fn remove_selected(&mut self, cx: &mut Context<'_, Self>) {
        for id in self.selected.clone() {
            self.remove(id, cx);
        }
    }

    /// Drop every finished movie from the list.
    pub fn clear_finished(&mut self, cx: &mut Context<'_, Self>) {
        self.rows.retain(|row| !row.finished());
        let rows = &self.rows;
        self.selected
            .retain(|id| rows.iter().any(|row| row.id == *id));
        self.notice = None;
        cx.notify();
    }

    // ── running ─────────────────────────────────────────────────────────

    /// Start the queue: every waiting movie goes to the compressor with
    /// the setting in the dropdown, and so does anything added later,
    /// until Stop.
    pub fn start(&mut self, cx: &mut Context<'_, Self>) {
        if self.running() {
            return;
        }
        let preset = self.preset().clone();
        if tools::find("ffmpeg").is_none() {
            self.tell(
                "Cannot find ffmpeg. Install it with: brew install ffmpeg"
                    .into(),
            );
            cx.notify();
            return;
        }
        self.notice = None;
        self.handle = Some(JobHandle::spawn([], self.settings()));
        self.all_done = false;
        self.run = Some(Run {
            started: Instant::now(),
            sent_bytes: 0,
            ended_bytes: 0,
            before: 0,
            after: 0,
            writing: HashMap::new(),
            looked: Instant::now(),
        });
        // Idle sleep would pause the encode; -w ends it with the app.
        self.awake = std::process::Command::new("/usr/bin/caffeinate")
            .args(["-i", "-w", &std::process::id().to_string()])
            .spawn()
            .ok();
        let waiting: Vec<usize> = self
            .rows
            .iter()
            .filter(|row| row.status == Status::Waiting)
            .map(|row| row.id)
            .collect();
        self.log(&format!(
            "start {} ({}) on {} movie(s){}",
            preset.name,
            preset.options.quality_label(),
            waiting.len(),
            if preset.replace {
                ", replacing originals"
            } else {
                ""
            }
        ));
        for id in waiting {
            self.send(id);
        }
        Self::poll(cx);
        cx.notify();
    }

    /// Hand one waiting movie to the running queue.
    fn send(&mut self, id: usize) {
        let Some(handle) = &self.handle else {
            return;
        };
        let Some(row) = self.rows.iter_mut().find(|row| row.id == id) else {
            return;
        };
        let Some(ffmpeg) = tools::find("ffmpeg") else {
            row.status =
                Status::Finished(Outcome::Failed("cannot find ffmpeg".into()));
            return;
        };
        row.sent = Some(String::new());
        if let Some(run) = &mut self.run {
            run.sent_bytes += row.bytes;
        }
        handle.push(Task {
            id,
            input: row.path.clone(),
            ffmpeg,
        });
    }

    /// Stop: the movie being compressed is abandoned and its partial
    /// output removed; the rest stay waiting.
    pub fn stop(&self, cx: &mut Context<'_, Self>) {
        self.stop_now();
        cx.notify();
    }

    fn stop_now(&self) {
        if let Some(handle) = &self.handle {
            self.log("stop");
            handle.stop();
        }
    }

    fn poll(cx: &Context<'_, Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                let keep_going =
                    this.update(cx, Self::poll_once).unwrap_or(false);
                if !keep_going {
                    break;
                }
            }
        })
        .detach();
    }

    /// Drain the queue's events once. Returns whether to keep polling.
    pub(crate) fn poll_once(&mut self, cx: &mut Context<'_, Self>) -> bool {
        let Some(handle) = &self.handle else {
            return false;
        };
        let events = handle.drain();
        // Nothing left waiting or compressing: the queue is done, so it
        // stops. Movies dropped before then join it; after, Start again.
        let busy = self.rows.iter().any(|row| {
            matches!(row.status, Status::Waiting | Status::Compressing { .. })
        });
        if !busy && handle.waiting() == 0 && !self.stopping {
            self.stopping = true;
            self.all_done = true;
            self.log("queue done");
            handle.stop();
        }
        self.look_at_outputs();
        if events.is_empty() {
            return true;
        }
        let mut done = false;
        for event in events {
            match event {
                JobEvent::Started { id, settings } => {
                    if let Some(row) = self.row_mut(id) {
                        row.sent = Some(settings.label());
                        row.output = Some(settings.options.output(&row.path));
                        row.status = Status::Compressing {
                            fraction: None,
                            eta: None,
                            speed: None,
                        };
                        let line =
                            format!("compressing {}", row.path.display());
                        self.log(&line);
                    }
                }
                JobEvent::Progress {
                    id,
                    fraction,
                    eta,
                    speed,
                } => {
                    if let Some(row) = self.row_mut(id) {
                        row.status = Status::Compressing {
                            fraction,
                            eta,
                            speed,
                        };
                    }
                }
                JobEvent::Line { text, .. } => self.log(&format!("  {text}")),
                JobEvent::Finished { id, outcome } => {
                    self.log(&format!("  {}", describe(&outcome)));
                    self.count(id, &outcome);
                    if let Some(row) = self.row_mut(id) {
                        // A stopped movie goes back to waiting, to be run
                        // again on the next Start.
                        if outcome == Outcome::Stopped {
                            row.status = Status::Waiting;
                            row.sent = None;
                        } else {
                            row.status = Status::Finished(outcome);
                        }
                    }
                }
                JobEvent::Done => done = true,
            }
        }
        if done {
            self.handle = None;
            self.stopping = false;
            for row in &mut self.rows {
                if row.status == Status::Waiting {
                    row.sent = None;
                }
            }
            let totals = self.totals();
            self.log(&format!(
                "stopped: {} compressed, {} skipped, {} failed",
                totals.compressed, totals.skipped, totals.failed
            ));
            if let Some(mut awake) = self.awake.take() {
                let _ = awake.kill();
                let _ = awake.wait();
            }
            if self.all_done {
                self.finished_all();
            }
            self.run = None;
        }
        cx.notify();
        !done
    }

    /// Add a movie that ended to its run's totals.
    fn count(&mut self, id: usize, outcome: &Outcome) {
        let bytes = self
            .rows
            .iter()
            .find(|row| row.id == id)
            .map_or(0, |row| row.bytes);
        let Some(run) = &mut self.run else {
            return;
        };
        run.writing.remove(&id);
        match outcome {
            Outcome::Stopped => {
                run.sent_bytes = run.sent_bytes.saturating_sub(bytes);
            }
            Outcome::Compressed { before, after, .. } => {
                run.ended_bytes += bytes;
                run.before += before;
                run.after += after;
            }
            _ => run.ended_bytes += bytes,
        }
    }

    /// Every couple of seconds, how big the outputs being written are.
    fn look_at_outputs(&mut self) {
        let Some(run) = &mut self.run else {
            return;
        };
        if run.looked.elapsed() < Duration::from_secs(2) {
            return;
        }
        run.looked = Instant::now();
        run.writing = self
            .rows
            .iter()
            .filter(|row| matches!(row.status, Status::Compressing { .. }))
            .filter_map(|row| {
                let output = row.output.as_ref()?;
                Some((row.id, std::fs::metadata(output).ok()?.len()))
            })
            .collect();
    }

    /// Where the run is heading, once it has gone long enough to tell.
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    pub fn estimate(&self) -> Option<Estimate> {
        let run = self.run.as_ref()?;
        let elapsed = run.started.elapsed();
        if elapsed < SETTLE {
            return None;
        }
        // Input done so far, counting the part of each movie in progress.
        let (mut done_in, mut part_in, mut part_out) =
            (run.ended_bytes as f64, 0.0, 0.0);
        for row in &self.rows {
            if let Status::Compressing {
                fraction: Some(fraction),
                ..
            } = row.status
            {
                let part = row.bytes as f64 * fraction;
                done_in += part;
                if let Some(written) = run.writing.get(&row.id) {
                    part_in += part;
                    part_out += *written as f64;
                }
            }
        }
        let rate = done_in / elapsed.as_secs_f64();
        let ratio_in = run.before as f64 + part_in;
        if rate <= 0.0 || ratio_in <= 0.0 {
            return None;
        }
        let ratio = (run.after as f64 + part_out) / ratio_in;
        let left = (run.sent_bytes as f64 - done_in).max(0.0);
        let finish = chrono::Local::now()
            + chrono::Duration::seconds((left / rate) as i64);
        Some(Estimate {
            bytes: (run.sent_bytes as f64 * ratio) as u64,
            from: run.sent_bytes,
            finish,
        })
    }

    /// The queue ran out: say so, and sleep if asked.
    fn finished_all(&self) {
        let Some(run) = &self.run else {
            return;
        };
        let mut message = format!(
            "{} compressed",
            self.rows
                .iter()
                .filter(|row| matches!(
                    row.status,
                    Status::Finished(Outcome::Compressed { .. })
                ))
                .count()
        );
        if run.before > 0 {
            let _ = write!(
                message,
                ", {} → {}",
                human_bytes(run.before),
                human_bytes(run.after)
            );
        }
        notify(&message);
        if self.sleep_when_done {
            self.log("sleeping, as asked");
            let _ = std::process::Command::new("/usr/bin/pmset")
                .arg("sleepnow")
                .spawn();
        }
    }

    pub fn toggle_sleep_when_done(&mut self, cx: &mut Context<'_, Self>) {
        self.sleep_when_done = !self.sleep_when_done;
        cx.notify();
    }

    // ── samples ─────────────────────────────────────────────────────────

    /// Encode samples of the selected movie, or the first waiting one, at
    /// the current preset's quality and a step either side.
    pub fn open_sampler(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let target = self
            .rows
            .iter()
            .find(|row| self.selected.contains(&row.id))
            .or_else(|| {
                self.rows.iter().find(|row| row.status == Status::Waiting)
            })
            .or_else(|| self.rows.first())
            .map(|row| row.path.clone());
        let Some(input) = target else {
            self.notice = Some("Add a movie to try the settings on.".into());
            cx.notify();
            return;
        };
        let Some(ffmpeg) = tools::find("ffmpeg") else {
            self.tell(
                "Cannot find ffmpeg. Install it with: brew install ffmpeg"
                    .into(),
            );
            cx.notify();
            return;
        };
        let options = self.preset().options;
        let dir = samples_dir();
        // Only ever this app's own samples.
        let _ = std::fs::remove_dir_all(&dir);
        self.sampler = Some(Sampler {
            input: input.clone(),
            result: None,
        });
        window.focus(&self.sampler_focus, cx);
        cx.notify();
        let made = cx
            .background_executor()
            .spawn(async move { sample::make(&ffmpeg, &input, options, &dir) });
        cx.spawn(async move |this, cx| {
            let result = made.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(sampler) = &mut this.sampler {
                    sampler.result = Some(result);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Open every sample in `QuickTime Player`, to watch side by side.
    pub fn watch_samples(&mut self, cx: &mut Context<'_, Self>) {
        let Some(Sampler {
            result: Some(Ok(samples)),
            ..
        }) = &self.sampler
        else {
            return;
        };
        let opened = std::process::Command::new("/usr/bin/open")
            .args(["-a", "QuickTime Player"])
            .args(samples.iter().map(|sample| &sample.path))
            .status();
        if !opened.is_ok_and(|status| status.success()) {
            self.tell("Cannot open the samples in QuickTime Player.".into());
            cx.notify();
        }
    }

    /// Give the current preset a sample's quality.
    pub fn use_sample(
        &mut self,
        options: Options,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let current = &mut self.presets.current_mut().options;
        current.crf = options.crf;
        current.gpu_quality = options.gpu_quality;
        self.save_presets();
        self.close_sampler(window, cx);
    }

    pub fn close_sampler(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.sampler = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn row_mut(&mut self, id: usize) -> Option<&mut Row> {
        self.rows.iter_mut().find(|row| row.id == id)
    }
}

/// One line for the log about how a movie went.
pub fn describe(outcome: &Outcome) -> String {
    match outcome {
        Outcome::Compressed {
            output,
            before,
            after,
        } => format!(
            "done: {} ({} → {})",
            output.display(),
            human_bytes(*before),
            human_bytes(*after)
        ),
        Outcome::Skipped(why) => format!("skipped: {why}"),
        Outcome::Failed(why) => format!("failed: {why}"),
        Outcome::Stopped => "stopped".into(),
    }
}

/// Where samples are made: the app's cache, emptied each time.
fn samples_dir() -> PathBuf {
    let home =
        std::env::var_os("HOME").map_or_else(PathBuf::new, PathBuf::from);
    home.join("Library/Caches/clipcraft/samples")
}

/// A macOS notification.
fn notify(message: &str) {
    let quoted = message.replace('\\', "\\\\").replace('"', "\\\"");
    let _ = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(format!(
            "display notification \"{quoted}\" with title \"clipcraft\" sound name \"Glass\""
        ))
        .spawn();
}

/// A short list of file names for a notice.
fn names(paths: &[PathBuf]) -> String {
    let mut list: Vec<String> =
        paths.iter().take(3).map(|path| file_name(path)).collect();
    if paths.len() > 3 {
        list.push(format!("and {} more", paths.len() - 3));
    }
    list.join(", ")
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl gpui_kit::Render for ClipCraft {
    fn render(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl gpui_kit::IntoElement {
        crate::views::root(self, window, cx)
    }
}
