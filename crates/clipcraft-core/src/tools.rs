//! Finding ffmpeg and ffprobe. An app opened from Finder gets only the
//! system folders on its `PATH`, where Homebrew's ffmpeg does not live.

use std::path::PathBuf;

/// Where `name` lives: the first on [`search_path`].
pub fn find(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&search_path())
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// Homebrew's folders first, then the inherited `PATH`, then the system's.
pub fn search_path() -> std::ffi::OsString {
    let mut dirs: Vec<PathBuf> = ["/opt/homebrew/bin", "/usr/local/bin"]
        .iter()
        .map(PathBuf::from)
        .collect();
    if let Some(inherited) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&inherited));
    }
    for system in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(PathBuf::from(system));
    }
    let mut seen = Vec::new();
    dirs.retain(|dir| {
        let new = !seen.contains(dir);
        seen.push(dir.clone());
        new
    });
    std::env::join_paths(dirs).unwrap_or_default()
}
