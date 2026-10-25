//! Turning what was dropped into the movie files to compress: files are
//! taken if they are movies, folders are searched all the way down.

use std::path::{Path, PathBuf};

/// Extensions taken as movies, lowercase.
pub const MOVIE_EXTENSIONS: [&str; 13] = [
    "mp4", "mov", "m4v", "mkv", "avi", "mts", "m2ts", "mpg", "mpeg", "wmv",
    "webm", "3gp", "insv",
];

/// Whether `path` names a movie the scripts should compress: a movie
/// extension, not hidden (macOS's `._` sidecars), and not a script's own
/// `_compressed` output.
pub fn is_movie(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|n| n.to_string_lossy()) else {
        return false;
    };
    if name.starts_with('.') {
        return false;
    }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    if stem.ends_with("_compressed") {
        return false;
    }
    path.extension().is_some_and(|ext| {
        MOVIE_EXTENSIONS
            .contains(&ext.to_string_lossy().to_lowercase().as_str())
    })
}

/// What a set of dropped or pasted paths holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Found {
    /// Movies, in the order given, each folder's sorted by path.
    pub movies: Vec<PathBuf>,
    /// Paths that are not there.
    pub missing: Vec<PathBuf>,
    /// Files given directly that are not movies.
    pub not_movies: Vec<PathBuf>,
}

/// The movies in `paths`. A file named directly is taken if it is a movie;
/// a folder gives every movie under it, hidden folders skipped.
pub fn expand(paths: &[PathBuf]) -> Found {
    let mut found = Found::default();
    for path in paths {
        let Ok(meta) = std::fs::metadata(path) else {
            found.missing.push(path.clone());
            continue;
        };
        if meta.is_dir() {
            let mut movies = Vec::new();
            walk(path, &mut movies);
            movies.sort();
            found.movies.extend(movies);
        } else if is_movie(path) {
            found.movies.push(path.clone());
        } else {
            found.not_movies.push(path.clone());
        }
    }
    found
}

fn walk(dir: &Path, movies: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        match entry.file_type() {
            Ok(kind) if kind.is_dir() && !hidden => walk(&path, movies),
            Ok(kind) if kind.is_file() && is_movie(&path) => movies.push(path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movies_are_told_apart() {
        assert!(is_movie(Path::new("/a/GX010042.MP4")));
        assert!(is_movie(Path::new("/a/clip.mov")));
        assert!(!is_movie(Path::new("/a/._clip.mov")), "AppleDouble");
        assert!(!is_movie(Path::new("/a/clip_compressed.mp4")));
        assert!(!is_movie(Path::new("/a/notes.txt")));
        assert!(!is_movie(Path::new("/a/mov")));
    }

    #[test]
    fn folders_are_searched_all_the_way_down() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let root = temp.path();
        for file in [
            "b.mov",
            "a.MP4",
            "a_compressed.mp4",
            "notes.txt",
            "Day 2/c.mov",
            ".hidden/d.mov",
            "Day 2/._c.mov",
        ] {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().expect("parent"))
                .expect("mkdir");
            std::fs::write(path, "x").expect("write");
        }
        let found = expand(&[
            root.to_path_buf(),
            root.join("notes.txt"),
            root.join("gone.mov"),
        ]);
        assert_eq!(
            found.movies,
            [
                root.join("Day 2/c.mov"),
                root.join("a.MP4"),
                root.join("b.mov")
            ]
        );
        assert_eq!(found.not_movies, [root.join("notes.txt")]);
        assert_eq!(found.missing, [root.join("gone.mov")]);
    }
}
