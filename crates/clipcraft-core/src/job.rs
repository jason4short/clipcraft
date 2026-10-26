//! A queue of movies, each compressed by its own run of ffmpeg, with a
//! report of what is happening.
//!
//! The queue stays open while it runs: movies added mid-run join the end,
//! and an empty queue waits for more until the run is stopped. Each movie
//! is encoded with the settings as they are when it starts, so a change
//! mid-run applies to every movie not yet begun.
//!
//! Every file has its own progress, result and error. ffmpeg's progress
//! line, `time=00:01:23.45 ... speed=2.1x`, is read against the movie's
//! length from ffprobe. Around ffmpeg, this does what the old scripts did:
//! movies already in HEVC are skipped, the original's dates are copied, and
//! the original is replaced only once the result plays for the same length.
//! Stopping kills ffmpeg and removes the half-written output.

use std::collections::VecDeque;
use std::io::Read as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use crate::options::{MAX_AT_ONCE, Options, command_line};
use crate::size::human_bytes;
use crate::tools::search_path;

/// How many of ffmpeg's last lines a failure keeps, to say why.
const TAIL: usize = 12;

/// How long an empty queue sleeps before looking again.
const IDLE: std::time::Duration = std::time::Duration::from_millis(150);

/// One movie to compress.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    /// The caller's own id for it.
    pub id: usize,
    pub input: PathBuf,
    /// ffmpeg itself, found with [`crate::tools::find`].
    pub ffmpeg: PathBuf,
}

/// How movies are compressed: the preset in use, read again as each movie
/// starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The preset's name, for the list.
    pub name: String,
    /// Replace the original once the result checks out.
    pub replace: bool,
    pub options: Options,
}

impl Settings {
    /// "Compress Video, CRF 26".
    pub fn label(&self) -> String {
        format!("{}, {}", self.name, self.options.quality_label())
    }
}

/// How one file went.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Outcome {
    Compressed {
        output: PathBuf,
        before: u64,
        after: u64,
    },
    /// Nothing to do, and why.
    Skipped(String),
    Failed(String),
    /// Stopped by the user; the partial output was removed.
    Stopped,
}

/// What the runner reports, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEvent {
    /// A movie began, with these settings.
    Started {
        id: usize,
        settings: Settings,
    },
    Progress {
        id: usize,
        /// 0 to 1, when the movie's length is known.
        fraction: Option<f64>,
        /// Seconds left at the current speed.
        eta: Option<f64>,
        /// ffmpeg's speed, as it says it: "2.1x".
        speed: Option<String>,
    },
    /// A line for the log: the command run, "Done: …".
    Line {
        id: usize,
        text: String,
    },
    Finished {
        id: usize,
        outcome: Outcome,
    },
    /// The run was stopped, and every movie in it has ended.
    Done,
}

/// A running queue.
#[derive(Debug)]
pub struct JobHandle {
    queue: Arc<Mutex<VecDeque<Task>>>,
    events: Receiver<JobEvent>,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<Settings>>,
    /// Each worker's ffmpeg process group, while one runs.
    pids: Vec<Arc<AtomicU32>>,
}

impl JobHandle {
    /// Run `tasks`, and any pushed later, with `settings` until they are
    /// changed. As many run at once as the settings say, up to
    /// [`MAX_AT_ONCE`].
    pub fn spawn(
        tasks: impl IntoIterator<Item = Task>,
        settings: Settings,
    ) -> Self {
        let queue = Arc::new(Mutex::new(tasks.into_iter().collect()));
        let (tx, events) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let settings = Arc::new(Mutex::new(settings));
        let busy = Arc::new(AtomicUsize::new(0));
        let workers = usize::from(MAX_AT_ONCE);
        let live = Arc::new(AtomicUsize::new(workers));
        let pids: Vec<_> =
            (0..workers).map(|_| Arc::new(AtomicU32::new(0))).collect();
        for pid in &pids {
            let worker = Worker {
                queue: Arc::clone(&queue),
                tx: tx.clone(),
                stop: Arc::clone(&stop),
                settings: Arc::clone(&settings),
                busy: Arc::clone(&busy),
                pid: Arc::clone(pid),
                live: Arc::clone(&live),
            };
            std::thread::spawn(move || worker.run());
        }
        Self {
            queue,
            events,
            stop,
            settings,
            pids,
        }
    }

    /// Use `settings` for every movie that starts from now on.
    pub fn set_settings(&self, settings: Settings) {
        if let Ok(mut current) = self.settings.lock() {
            *current = settings;
        }
    }

    /// Add a movie to the end of the queue.
    pub fn push(&self, task: Task) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.push_back(task);
        }
    }

    /// Take a movie out of the queue, if it has not started. Returns
    /// whether it was still waiting.
    pub fn remove(&self, id: usize) -> bool {
        let Ok(mut queue) = self.queue.lock() else {
            return false;
        };
        let before = queue.len();
        queue.retain(|task| task.id != id);
        queue.len() != before
    }

    /// Put the waiting movies in the order of `ids`. Any not named keep
    /// their place after those that are.
    pub fn reorder(&self, ids: &[usize]) {
        if let Ok(mut queue) = self.queue.lock() {
            queue.make_contiguous().sort_by_key(|task| {
                ids.iter()
                    .position(|id| *id == task.id)
                    .unwrap_or(usize::MAX)
            });
        }
    }

    /// How many movies are waiting.
    pub fn waiting(&self) -> usize {
        self.queue.lock().map_or(0, |queue| queue.len())
    }

    /// Everything reported since the last drain.
    pub fn drain(&self) -> Vec<JobEvent> {
        self.events.try_iter().collect()
    }

    /// Stop after killing the files in progress.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for pid in &self.pids {
            let pid = pid.load(Ordering::SeqCst);
            if pid != 0 {
                kill_group(pid);
            }
        }
    }
}

/// Kill ffmpeg and anything it started: it runs as the leader of its own
/// process group, so the group's id is its pid.
fn kill_group(pid: u32) {
    let _ = Command::new("/bin/kill")
        .args(["-TERM", "--", &format!("-{pid}")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

struct Worker {
    queue: Arc<Mutex<VecDeque<Task>>>,
    tx: Sender<JobEvent>,
    stop: Arc<AtomicBool>,
    settings: Arc<Mutex<Settings>>,
    /// Workers compressing now, held to the settings' "at once".
    busy: Arc<AtomicUsize>,
    pid: Arc<AtomicU32>,
    /// Workers still running; the last one out reports Done.
    live: Arc<AtomicUsize>,
}

impl Worker {
    fn send(&self, event: JobEvent) {
        let _ = self.tx.send(event);
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run(&self) {
        while !self.stopped() {
            let Some((task, settings)) = self.next() else {
                std::thread::sleep(IDLE);
                continue;
            };
            self.send(JobEvent::Started {
                id: task.id,
                settings: settings.clone(),
            });
            let outcome = self.compress(&task, &settings);
            self.busy.fetch_sub(1, Ordering::SeqCst);
            self.send(JobEvent::Finished {
                id: task.id,
                outcome,
            });
        }
        if self.live.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.send(JobEvent::Done);
        }
    }

    /// The next movie and the settings now, if there is one and fewer
    /// than "at once" are running.
    fn next(&self) -> Option<(Task, Settings)> {
        let settings = self.settings.lock().ok()?.clone();
        let at_once =
            usize::from(settings.options.at_once.clamp(1, MAX_AT_ONCE));
        // Counting and taking happen under the queue's lock, so two
        // workers cannot both take the last free place.
        let task = {
            let mut queue = self.queue.lock().ok()?;
            if self.busy.load(Ordering::SeqCst) >= at_once {
                return None;
            }
            let task = queue.pop_front()?;
            self.busy.fetch_add(1, Ordering::SeqCst);
            drop(queue);
            task
        };
        Some((task, settings))
    }

    fn compress(&self, task: &Task, settings: &Settings) -> Outcome {
        let (id, input) = (task.id, task.input.as_path());
        let Ok(meta) = std::fs::metadata(input) else {
            return Outcome::Failed("the file is not there any more".into());
        };
        let before = meta.len();
        let output = settings.options.output(input);
        if output.exists() {
            return Outcome::Skipped(format!(
                "already compressed: {}",
                file_name(&output)
            ));
        }
        // Replacing renames the result to the original's name with the new
        // extension, which must not be a different movie.
        let result = settings.options.result(input, settings.replace);
        if settings.replace && result != input && result.exists() {
            return Outcome::Skipped(format!(
                "replacing would overwrite {}",
                file_name(&result)
            ));
        }
        if settings.options.skip_hevc
            && video_codec(input).as_deref() == Some("hevc")
        {
            return Outcome::Skipped("already HEVC".into());
        }
        let duration = duration(input);
        let args =
            settings
                .options
                .ffmpeg_args(input, &output, has_audio(input));
        self.send(JobEvent::Line {
            id,
            text: command_line("ffmpeg", &args),
        });

        let mut child = match Command::new(&task.ffmpeg)
            .args(&args)
            .env("PATH", search_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(err) => {
                return Outcome::Failed(format!(
                    "cannot run {}: {err}",
                    task.ffmpeg.display()
                ));
            }
        };
        self.pid.store(child.id(), Ordering::SeqCst);
        // A stop that landed before the pid was stored would be missed.
        if self.stopped() {
            kill_group(child.id());
        }

        // Progress and errors, on stderr. Progress is rewritten in place
        // with `\r`, so both `\r` and `\n` end a line.
        let mut tail = VecDeque::with_capacity(TAIL);
        if let Some(mut err) = child.stderr.take() {
            let mut buf = [0u8; 8192];
            let mut line = Vec::new();
            while let Ok(n) = err.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for &byte in &buf[..n] {
                    if byte == b'\r' || byte == b'\n' {
                        self.stderr_line(id, &line, duration, &mut tail);
                        line.clear();
                    } else {
                        line.push(byte);
                    }
                }
            }
            self.stderr_line(id, &line, duration, &mut tail);
        }
        let status = child.wait();
        self.pid.store(0, Ordering::SeqCst);

        if self.stopped() {
            // Only ever a file this run started: an existing output was
            // skipped above.
            let _ = std::fs::remove_file(&output);
            return Outcome::Stopped;
        }
        match status {
            Ok(status) if status.success() => {
                self.finish(task, settings, &output, &result, duration, before)
            }
            status => {
                let _ = std::fs::remove_file(&output);
                Outcome::Failed(tail.back().cloned().unwrap_or_else(|| {
                    match status {
                        Ok(status) => format!("ffmpeg ended with {status}"),
                        Err(err) => err.to_string(),
                    }
                }))
            }
        }
    }

    /// After ffmpeg: copy the dates, and replace the original if the
    /// result plays for the same length.
    fn finish(
        &self,
        task: &Task,
        settings: &Settings,
        output: &Path,
        result: &Path,
        duration: Option<f64>,
        before: u64,
    ) -> Outcome {
        let input = task.input.as_path();
        let made = std::fs::metadata(output).map_or(0, |meta| meta.len());
        let saving = u64::from(settings.options.min_saving.min(100));
        if saving > 0 && made * 100 > before * (100 - saving) {
            let _ = std::fs::remove_file(output);
            return Outcome::Skipped(format!(
                "would not shrink by {saving}%: {} → {}, original kept",
                human_bytes(before),
                human_bytes(made)
            ));
        }
        if settings.options.keep_dates
            && let Err(err) = copy_dates(input, output)
        {
            self.send(JobEvent::Line {
                id: task.id,
                text: format!("cannot copy the dates: {err}"),
            });
        }
        if settings.replace {
            let length = self::duration(output);
            let same = duration
                .zip(length)
                .is_some_and(|(was, now)| (was - now).abs() <= 1.0);
            if !same {
                return Outcome::Failed(format!(
                    "lengths differ ({} vs {}), so the original was kept; \
                     the compressed copy is {}",
                    seconds(duration),
                    seconds(length),
                    file_name(output)
                ));
            }
            if let Err(err) = std::fs::remove_file(input) {
                return Outcome::Failed(format!(
                    "cannot delete the original: {err}"
                ));
            }
            if let Err(err) = std::fs::rename(output, result) {
                return Outcome::Failed(format!(
                    "cannot rename {} to {}: {err}",
                    file_name(output),
                    file_name(result)
                ));
            }
        }
        let after = std::fs::metadata(result).map_or(0, |meta| meta.len());
        self.send(JobEvent::Line {
            id: task.id,
            text: format!("Done: {}", result.display()),
        });
        Outcome::Compressed {
            output: result.to_path_buf(),
            before,
            after,
        }
    }

    fn stderr_line(
        &self,
        id: usize,
        line: &[u8],
        duration: Option<f64>,
        tail: &mut VecDeque<String>,
    ) {
        let text = String::from_utf8_lossy(line).trim().to_string();
        if text.is_empty() {
            return;
        }
        if let Some(progress) = parse_progress(&text) {
            let fraction =
                duration.map(|total| (progress.time / total).clamp(0.0, 1.0));
            let eta = duration
                .zip(progress.rate)
                .map(|(total, rate)| ((total - progress.time) / rate).max(0.0));
            self.send(JobEvent::Progress {
                id,
                fraction,
                eta,
                speed: progress.speed,
            });
            return;
        }
        if tail.len() == TAIL {
            tail.pop_front();
        }
        tail.push_back(text);
    }
}

/// One of ffmpeg's progress lines.
#[derive(Clone, Debug, PartialEq)]
pub struct Progress {
    /// Seconds of the movie done.
    pub time: f64,
    pub speed: Option<String>,
    /// The speed as a number: 2.1 for "2.1x".
    pub rate: Option<f64>,
}

/// Read `frame= 1200 fps= 60 ... time=00:00:40.03 bitrate=... speed=2.1x`.
pub fn parse_progress(line: &str) -> Option<Progress> {
    let time = field(line, "time=")?;
    let mut parts = time.split(':').map(str::parse::<f64>);
    let (h, m, s) = (
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    );
    let speed = field(line, "speed=").filter(|s| s.ends_with('x'));
    let rate = speed
        .and_then(|s| s.trim_end_matches('x').parse::<f64>().ok())
        .filter(|rate| *rate > 0.0);
    Some(Progress {
        time: h.mul_add(3600.0, m.mul_add(60.0, s)),
        speed: speed.map(str::to_string),
        rate,
    })
}

/// The value after `key=`, which ffmpeg may pad with spaces.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let start = line.find(key)? + key.len();
    line[start..].split_whitespace().next()
}

/// The movie's length in seconds, from ffprobe.
pub fn duration(input: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .env("PATH", search_path())
        .args(["-v", "error", "-show_entries", "format=duration"])
        .args(["-of", "csv=p=0"])
        .arg(input)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()
        .filter(|seconds: &f64| *seconds > 0.0)
}

/// Whether the movie has sound, from ffprobe.
pub fn has_audio(input: &Path) -> bool {
    Command::new("ffprobe")
        .env("PATH", search_path())
        .args(["-v", "error", "-select_streams", "a:0"])
        .args(["-show_entries", "stream=index", "-of", "csv=p=0"])
        .arg(input)
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|output| !output.stdout.trim_ascii().is_empty())
}

/// The first video stream's codec, from ffprobe: "hevc", "h264".
pub fn video_codec(input: &Path) -> Option<String> {
    let output = Command::new("ffprobe")
        .env("PATH", search_path())
        .args(["-v", "error", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=codec_name", "-of", "csv=p=0"])
        .arg(input)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    let codec = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!codec.is_empty()).then_some(codec)
}

/// Give `to` the dates of `from`, as `touch -r` does, and its creation
/// date too.
fn copy_dates(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::macos::fs::FileTimesExt as _;
    let meta = std::fs::metadata(from)?;
    let mut times = std::fs::FileTimes::new()
        .set_accessed(meta.accessed()?)
        .set_modified(meta.modified()?);
    if let Ok(created) = meta.created() {
        times = times.set_created(created);
    }
    std::fs::File::options()
        .write(true)
        .open(to)?
        .set_times(times)
}

fn seconds(length: Option<f64>) -> String {
    length.map_or_else(|| "unknown".into(), |secs| format!("{secs:.1}s"))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn progress_lines_are_read() {
        let line = "frame= 1200 fps= 60 q=28.0 size=   10240kB \
                    time=00:01:30.50 bitrate=1000.0kbits/s speed=2.5x";
        assert_eq!(
            parse_progress(line),
            Some(Progress {
                time: 90.5,
                speed: Some("2.5x".into()),
                rate: Some(2.5),
            })
        );
        assert_eq!(parse_progress("time=N/A speed=N/A"), None);
        assert_eq!(parse_progress("Input #0, mov,mp4"), None);
    }

    /// A stand-in for ffmpeg: prints progress the way it does, then
    /// writes its last argument, the output. `body` runs first, with the
    /// input in `$IN`.
    fn fake_ffmpeg(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("ffmpeg");
        let script = format!(
            "#!/bin/bash\nfor a; do OUT=\"$a\"; done\n\
             for a; do [ \"$prev\" = -i ] && IN=\"$a\"; prev=\"$a\"; done\n{body}\n\
             printf 'frame=1 time=00:00:01.00 speed=2.0x\\r' >&2\n\
             echo made > \"$OUT\"\n"
        );
        std::fs::write(&path, script).expect("write ffmpeg");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod");
        path
    }

    fn task(id: usize, input: PathBuf, ffmpeg: &Path) -> Task {
        Task {
            id,
            input,
            ffmpeg: ffmpeg.to_path_buf(),
        }
    }

    fn settings(at_once: u8) -> Settings {
        Settings {
            name: "Test".into(),
            replace: false,
            options: Options {
                skip_hevc: false,
                at_once,
                ..Options::AERIAL
            },
        }
    }

    /// Drain `handle` until `done` says so.
    fn until(
        handle: &JobHandle,
        events: &mut Vec<JobEvent>,
        done: impl Fn(&[JobEvent]) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(events) {
            assert!(Instant::now() < deadline, "timed out: {events:?}");
            events.extend(handle.drain());
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn finished(events: &[JobEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, JobEvent::Finished { .. }))
            .count()
    }

    fn outcomes(events: &[JobEvent]) -> Vec<(usize, Outcome)> {
        events
            .iter()
            .filter_map(|event| match event {
                JobEvent::Finished { id, outcome } => {
                    Some((*id, outcome.clone()))
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn each_file_is_compressed_skipped_or_failed_on_its_own() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dir = temp.path();
        let ffmpeg = fake_ffmpeg(
            dir,
            r#"case "$IN" in *bad*) echo "Invalid data found" >&2; exit 1;; esac"#,
        );
        for name in ["good.mov", "bad.mov", "done.mov", "done_compressed.mp4"] {
            std::fs::write(dir.join(name), "movie data").expect("write");
        }
        let old = std::time::SystemTime::UNIX_EPOCH
            + Duration::from_secs(1_600_000_000);
        std::fs::File::options()
            .write(true)
            .open(dir.join("good.mov"))
            .and_then(|f| f.set_modified(old))
            .expect("set mtime");
        let handle = JobHandle::spawn(
            [
                task(0, dir.join("good.mov"), &ffmpeg),
                task(1, dir.join("bad.mov"), &ffmpeg),
            ],
            settings(1),
        );
        let mut events = Vec::new();
        until(&handle, &mut events, |e| finished(e) == 2);
        // The queue waits, and takes what is added after it started.
        handle.push(task(2, dir.join("done.mov"), &ffmpeg));
        handle.push(task(3, dir.join("gone.mov"), &ffmpeg));
        until(&handle, &mut events, |e| finished(e) == 4);
        handle.stop();
        until(&handle, &mut events, |e| e.last() == Some(&JobEvent::Done));
        let outcomes = outcomes(&events);
        let made = dir.join("good_compressed.mp4");
        assert!(
            matches!(&outcomes[0], (0, Outcome::Compressed { output, before: 10, .. })
                if *output == made),
            "{events:?}"
        );
        let modified = std::fs::metadata(&made)
            .and_then(|m| m.modified())
            .expect("mtime");
        assert_eq!(modified, old, "the original's dates are kept");
        assert_eq!(
            outcomes[1],
            (1, Outcome::Failed("Invalid data found".into()))
        );
        assert!(!dir.join("bad_compressed.mp4").exists(), "no half file");
        assert!(
            matches!(&outcomes[2], (2, Outcome::Skipped(why)) if why.contains("already"))
        );
        assert!(matches!(&outcomes[3], (3, Outcome::Failed(_))));
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Progress { id: 0, .. }))
        );
        assert!(
            events.iter().any(|e| matches!(e, JobEvent::Line { id: 0, text }
                if text.starts_with("ffmpeg -nostdin") && text.contains("good.mov"))),
            "the command is logged: {events:?}"
        );
    }

    #[test]
    fn replacing_never_overwrites_another_movie_or_an_unchecked_one() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dir = temp.path();
        let ffmpeg = fake_ffmpeg(dir, "");
        for (name, body) in
            [("clip.wmv", "wmv"), ("clip.mp4", "other"), ("b.mov", "b")]
        {
            std::fs::write(dir.join(name), body).expect("write");
        }
        let mut replacing = settings(1);
        replacing.replace = true;
        replacing.options.min_saving = 0;
        let handle = JobHandle::spawn(
            [
                task(0, dir.join("clip.wmv"), &ffmpeg),
                task(1, dir.join("b.mov"), &ffmpeg),
            ],
            replacing,
        );
        let mut events = Vec::new();
        until(&handle, &mut events, |e| finished(e) == 2);
        handle.stop();
        let outcomes = outcomes(&events);
        assert!(
            matches!(&outcomes[0], (0, Outcome::Skipped(why)) if why.contains("clip.mp4")),
            "{events:?}"
        );
        // A file ffprobe cannot read has no length to check, so the
        // original stays.
        assert!(
            matches!(&outcomes[1], (1, Outcome::Failed(why)) if why.contains("kept")),
            "{events:?}"
        );
        assert!(dir.join("b.mov").exists());
    }

    #[test]
    fn a_result_that_does_not_shrink_is_thrown_away() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dir = temp.path();
        let ffmpeg = fake_ffmpeg(dir, "");
        // The fake writes 5 bytes: no smaller at all.
        std::fs::write(dir.join("a.mov"), "12345").expect("write");
        std::fs::write(dir.join("b.mov"), "1234567890").expect("write");
        let handle = JobHandle::spawn(
            [
                task(0, dir.join("a.mov"), &ffmpeg),
                task(1, dir.join("b.mov"), &ffmpeg),
            ],
            settings(2),
        );
        let mut events = Vec::new();
        until(&handle, &mut events, |e| finished(e) == 2);
        handle.stop();
        until(&handle, &mut events, |e| e.last() == Some(&JobEvent::Done));
        let mut outcomes = outcomes(&events);
        outcomes.sort_by_key(|(id, _)| *id);
        assert!(
            matches!(&outcomes[0], (0, Outcome::Skipped(why)) if why.contains("shrink")),
            "{events:?}"
        );
        assert!(!dir.join("a_compressed.mp4").exists());
        assert!(matches!(&outcomes[1], (1, Outcome::Compressed { .. })));
        assert_eq!(
            events.iter().filter(|e| **e == JobEvent::Done).count(),
            1,
            "two workers, one Done"
        );
    }

    #[test]
    fn settings_are_read_as_each_movie_starts() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dir = temp.path();
        let ffmpeg = fake_ffmpeg(dir, "");
        std::fs::write(dir.join("a.mov"), "1234567890").expect("write");
        let handle = JobHandle::spawn([], settings(1));
        let mut later = settings(1);
        later.name = "Later".into();
        later.options.gpu = false;
        handle.set_settings(later.clone());
        handle.push(task(0, dir.join("a.mov"), &ffmpeg));
        let mut events = Vec::new();
        until(&handle, &mut events, |e| finished(e) == 1);
        handle.stop();
        assert!(
            events.contains(&JobEvent::Started {
                id: 0,
                settings: later
            }),
            "{events:?}"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Line { text, .. }
                if text.contains("-c:v libx265"))),
            "the CPU encoder, as changed: {events:?}"
        );
    }

    #[test]
    fn the_waiting_queue_can_be_reordered() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let ffmpeg = fake_ffmpeg(temp.path(), "");
        let handle = JobHandle::spawn([], settings(1));
        handle.stop();
        // Once the worker is gone, nothing takes from the queue.
        let mut events = Vec::new();
        until(&handle, &mut events, |e| e.last() == Some(&JobEvent::Done));
        if let Ok(mut queue) = handle.queue.lock() {
            for id in 0..4 {
                queue.push_back(task(id, temp.path().join("x"), &ffmpeg));
            }
        }
        handle.reorder(&[3, 1]);
        let order: Vec<usize> = handle
            .queue
            .lock()
            .map(|q| q.iter().map(|t| t.id).collect())
            .unwrap_or_default();
        assert_eq!(order, [3, 1, 0, 2]);
    }

    #[test]
    fn stopping_kills_ffmpeg_and_removes_its_partial_output() {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let dir = temp.path();
        let ffmpeg = fake_ffmpeg(
            dir,
            r#"echo partial > "$OUT"
sleep 30 &
wait"#,
        );
        std::fs::write(dir.join("a.mov"), "x").expect("write");
        std::fs::write(dir.join("b.mov"), "x").expect("write");
        let handle = JobHandle::spawn(
            [
                task(0, dir.join("a.mov"), &ffmpeg),
                task(1, dir.join("b.mov"), &ffmpeg),
            ],
            settings(1),
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        while !dir.join("a_compressed.mp4").exists() {
            assert!(Instant::now() < deadline, "ffmpeg never started");
            std::thread::sleep(Duration::from_millis(20));
        }
        let started = Instant::now();
        handle.stop();
        let mut events = Vec::new();
        while events.last() != Some(&JobEvent::Done) {
            assert!(started.elapsed() < Duration::from_secs(10), "{events:?}");
            events.extend(handle.drain());
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(outcomes(&events), [(0, Outcome::Stopped)]);
        assert!(!dir.join("a_compressed.mp4").exists());
        assert!(!dir.join("b_compressed.mp4").exists(), "b never started");
    }
}
