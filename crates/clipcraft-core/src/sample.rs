//! Samples: a few seconds of one movie at the preset's quality.
//!
//! Each is encoded at the preset's quality and a step either side, next to
//! the same seconds of the original. Watching them side by side picks a
//! quality better than any number can.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::job::{duration, has_audio};
use crate::options::{Container, Options};
use crate::tools::search_path;

/// How long a sample runs, at most.
pub const SECONDS: f64 = 10.0;

/// One encoded sample.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// "CRF 26", or "Original" for the untouched seconds.
    pub label: String,
    /// The options it was made with; `None` for the original.
    pub options: Option<Options>,
    pub path: PathBuf,
    pub bytes: u64,
    /// The whole movie's size at this quality, going by the sample.
    pub projected: Option<u64>,
}

/// Encode samples of `input` into `dir`: the original's seconds, then the
/// preset a step smaller, as it is, and a step better. Blocks until done.
pub fn make(
    ffmpeg: &Path,
    input: &Path,
    options: Options,
    dir: &Path,
) -> Result<Vec<Sample>, String> {
    std::fs::create_dir_all(dir)
        .map_err(|err| format!("cannot make {}: {err}", dir.display()))?;
    let length = duration(input).ok_or("ffprobe cannot read this movie")?;
    let seconds = SECONDS.min(length);
    // From two-fifths in: past the take-off, before the landing.
    let start = if length > seconds * 2.0 {
        length * 0.4
    } else {
        0.0
    };
    let stem = input.file_stem().unwrap_or_default().to_string_lossy();
    let sound = has_audio(input);

    let ext = Container::Same.extension(input);
    let original = dir.join(format!("{stem}_sample_original.{ext}"));
    let copy: Vec<String> = [
        "-nostdin",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        &format!("{start:.2}"),
        "-t",
        &format!("{seconds:.2}"),
        "-i",
        &input.to_string_lossy(),
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-c",
        "copy",
        &original.to_string_lossy(),
    ]
    .iter()
    .map(|arg| (*arg).to_string())
    .collect();
    run(ffmpeg, &copy)?;
    let mut samples =
        vec![sample("Original".into(), None, original, seconds, length)];

    for steps in [-1, 0, 1] {
        let stepped = options.stepped(steps);
        let label = stepped.quality_label();
        let ext = stepped.container.extension(input);
        let name = label.replace(' ', "");
        let path = dir.join(format!("{stem}_sample_{name}.{ext}"));
        let _ = std::fs::remove_file(&path);
        run(
            ffmpeg,
            &stepped.sample_args(input, &path, sound, start, seconds),
        )?;
        samples.push(sample(label, Some(stepped), path, seconds, length));
    }
    Ok(samples)
}

fn sample(
    label: String,
    options: Option<Options>,
    path: PathBuf,
    seconds: f64,
    length: f64,
) -> Sample {
    let bytes = std::fs::metadata(&path).map_or(0, |meta| meta.len());
    #[allow(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss
    )]
    let projected = (seconds > 0.0 && bytes > 0)
        .then(|| (bytes as f64 / seconds * length) as u64);
    Sample {
        label,
        options,
        path,
        bytes,
        projected,
    }
}

fn run(ffmpeg: &Path, args: &[String]) -> Result<(), String> {
    let output = Command::new(ffmpeg)
        .args(args)
        .env("PATH", search_path())
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("cannot run ffmpeg: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    Err(err
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("ffmpeg failed")
        .trim()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the real ffmpeg on a made-up clip; skipped without it.
    #[test]
    fn samples_are_made_with_the_real_ffmpeg() {
        let Some(ffmpeg) = crate::tools::find("ffmpeg") else {
            return;
        };
        let temp = tempfile::TempDir::new().expect("tempdir");
        let input = temp.path().join("clip.mov");
        let status = Command::new(&ffmpeg)
            .args(["-v", "error", "-f", "lavfi", "-i"])
            .arg("testsrc=duration=4:size=640x360:rate=30")
            .args(["-f", "lavfi", "-i", "sine=duration=4"])
            .args(["-c:v", "libx264", "-c:a", "aac", "-shortest"])
            .arg(&input)
            .status()
            .expect("run ffmpeg");
        assert!(status.success());
        let options = Options {
            gpu: false,
            speed: crate::options::Speed::Veryfast,
            ..Options::AERIAL
        };
        let samples = make(&ffmpeg, &input, options, &temp.path().join("s"))
            .expect("samples");
        let labels: Vec<&str> =
            samples.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels, ["Original", "CRF 29", "CRF 26", "CRF 23"]);
        assert!(samples.iter().all(|s| s.bytes > 0), "{samples:?}");
        assert!(
            samples[1].bytes < samples[3].bytes,
            "better quality is bigger: {samples:?}"
        );
    }
}
