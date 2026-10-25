//! The saved presets: a name the user chose, its encoding options, and
//! whether it replaces originals. Kept in
//! `~/Library/Application Support/clipcraft/presets.json`.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::options::Options;

/// One saved preset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "SavedFile")]
pub struct Saved {
    pub name: String,
    /// Replace the original, once the result plays for the same length.
    pub replace: bool,
    pub options: Options,
}

/// A preset as saved, now or by the versions that handed off to the
/// `compressVideo` and `compressAerial` scripts.
#[derive(Deserialize)]
struct SavedFile {
    name: String,
    #[serde(default)]
    replace: bool,
    options: Option<Options>,
    /// The script it ran: "compressVideo", "compressAerial" or "ffmpeg".
    #[serde(default)]
    script: Option<String>,
    /// The CRF it was given.
    #[serde(default)]
    crf: Option<u8>,
}

impl From<SavedFile> for Saved {
    /// An old preset becomes the ffmpeg command its script ran.
    fn from(file: SavedFile) -> Self {
        let mut options = match file.script.as_deref() {
            Some("compressVideo") => Options::VIDEO,
            Some("compressAerial") => Options::AERIAL,
            _ => file.options.unwrap_or_default(),
        };
        if file.script.as_deref() != Some("compressVideo")
            && let Some(crf) = file.crf
        {
            options.crf = crf;
        }
        Self {
            name: file.name,
            replace: file.replace,
            options,
        }
    }
}

/// Every preset, and the one in use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Presets {
    pub list: Vec<Saved>,
    #[serde(default)]
    pub selected: usize,
}

impl Default for Presets {
    /// One preset for everyday video and one for drone footage.
    fn default() -> Self {
        Self {
            list: vec![
                Saved {
                    name: "Compress Video".into(),
                    replace: false,
                    options: Options::VIDEO,
                },
                Saved {
                    name: "Compress Aerial".into(),
                    replace: false,
                    options: Options::AERIAL,
                },
            ],
            selected: 0,
        }
    }
}

impl Presets {
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        Some(
            PathBuf::from(home)
                .join("Library/Application Support/clipcraft/presets.json"),
        )
    }

    /// The presets saved at `path`, or the defaults when there are none
    /// yet or the file cannot be read.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<Self>(&text).ok())
            .filter(|presets| !presets.list.is_empty())
            .map(|mut presets| {
                presets.selected = presets.selected.min(presets.list.len() - 1);
                presets
            })
            .unwrap_or_default()
    }

    /// Save through a rename, so the file is never half written.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&temp, path)
    }

    /// The preset in use.
    pub fn current(&self) -> &Saved {
        &self.list[self.selected.min(self.list.len() - 1)]
    }

    pub fn current_mut(&mut self) -> &mut Saved {
        let index = self.selected.min(self.list.len() - 1);
        &mut self.list[index]
    }

    /// Rename preset `index`. A blank name is refused.
    pub fn rename(&mut self, index: usize, name: &str) -> bool {
        let name = name.trim();
        match self.list.get_mut(index) {
            Some(saved) if !name.is_empty() => {
                saved.name = name.to_string();
                true
            }
            _ => false,
        }
    }

    /// Copy preset `index` next to itself as "<name> copy", and use it.
    pub fn duplicate(&mut self, index: usize) {
        let Some(saved) = self.list.get(index) else {
            return;
        };
        let mut copy = saved.clone();
        copy.name = format!("{} copy", saved.name);
        self.list.insert(index + 1, copy);
        self.selected = index + 1;
    }

    /// Delete preset `index`. The last one is kept, so there is always one
    /// to use. Returns whether it was deleted.
    pub fn delete(&mut self, index: usize) -> bool {
        if self.list.len() < 2 || index >= self.list.len() {
            return false;
        }
        self.list.remove(index);
        if self.selected > index || self.selected >= self.list.len() {
            self.selected = self.selected.saturating_sub(1);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renamed_presets_round_trip() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = temp.path().join("sub/presets.json");
        let mut presets = Presets::load(&path);
        assert_eq!(presets, Presets::default(), "nothing saved yet");

        assert!(presets.rename(1, "  Drone  "));
        assert!(!presets.rename(1, "   "), "blank names are refused");
        presets.selected = 1;
        presets.current_mut().replace = true;
        presets.current_mut().options.gpu_quality = 70;
        presets.save(&path).expect("save");

        let loaded = Presets::load(&path);
        assert_eq!(loaded.current().name, "Drone");
        assert!(loaded.current().replace);
        assert_eq!(loaded.current().options.gpu_quality, 70);
        assert_eq!(loaded, presets);
    }

    #[test]
    fn presets_from_the_script_days_become_their_commands() {
        let old = r#"{"list": [
            {"name": "Compress Video", "script": "compressVideo", "crf": null},
            {"name": "Drone", "script": "compressAerial", "replace": true, "crf": 22}
        ], "selected": 1}"#;
        let presets: Presets = serde_json::from_str(old).expect("parse");
        assert_eq!(presets.list[0].options, Options::VIDEO);
        assert_eq!(
            presets.current().options,
            Options {
                crf: 22,
                ..Options::AERIAL
            }
        );
        assert!(presets.current().replace);
    }

    #[test]
    fn presets_are_duplicated_and_deleted_but_one_always_stays() {
        let mut presets = Presets {
            selected: 1,
            ..Presets::default()
        };
        presets.current_mut().options.nudge_quality(true);
        presets.duplicate(1);
        assert_eq!(presets.selected, 2);
        assert_eq!(presets.current().name, "Compress Aerial copy");
        assert_eq!(presets.current().options.gpu_quality, 66);

        assert!(presets.delete(0));
        assert_eq!(presets.selected, 1, "still on the copy");
        assert_eq!(presets.current().name, "Compress Aerial copy");
        assert!(presets.delete(1));
        assert_eq!(presets.current().name, "Compress Aerial");
        assert!(!presets.delete(0), "the last preset stays");
    }

    #[test]
    fn a_broken_file_falls_back_to_the_defaults() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let path = temp.path().join("presets.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert_eq!(Presets::load(&path), Presets::default());
        std::fs::write(&path, r#"{"list":[],"selected":3}"#).expect("write");
        assert_eq!(Presets::load(&path), Presets::default());
    }
}
