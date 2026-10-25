//! A plain-text record of every movie compressed, so what happened, errors included, can
//! be read after the fact. It lives where macOS keeps app logs:
//! `~/Library/Logs/clipcraft/clipcraft.log`, which Console.app also shows.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// When the log passes this size it is moved aside to `clipcraft.log.old`
/// and a new one started, so it never grows without bound.
const ROLL_AT: u64 = 20 * 1024 * 1024;

pub fn default_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Logs/clipcraft/clipcraft.log"))
}

/// Append one timestamped line. A log that cannot be written must never
/// stop a compress, so failures are ignored.
pub fn append(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::metadata(path).is_ok_and(|meta| meta.len() > ROLL_AT) {
        let _ = std::fs::rename(path, path.with_extension("log.old"));
    }
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        for line in text.lines() {
            let _ = writeln!(file, "{stamp}  {line}");
        }
    }
}
