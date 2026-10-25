//! Everything about how a movie is encoded, and the ffmpeg command that
//! does it. clipcraft is the script: each movie's command is built here
//! from its preset, and ffmpeg runs it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The CRF range for the CPU encoders: 51 is their limit, and below 10 the
/// files grow without visible gain. Lower is better quality.
pub const MIN_CRF: u8 = 10;
pub const MAX_CRF: u8 = 51;

/// The GPU encoder's quality range. Higher is better quality.
pub const MIN_GPU_QUALITY: u8 = 1;
pub const MAX_GPU_QUALITY: u8 = 100;

/// The video format.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub enum Codec {
    /// Plays everywhere.
    H264,
    /// About half the size of H.264 at the same quality.
    #[default]
    Hevc,
}

impl Codec {
    pub const ALL: [Self; 2] = [Self::H264, Self::Hevc];

    pub const fn label(self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::Hevc => "HEVC",
        }
    }

    const fn encoder(self, gpu: bool) -> &'static str {
        match (self, gpu) {
            (Self::H264, false) => "libx264",
            (Self::Hevc, false) => "libx265",
            (Self::H264, true) => "h264_videotoolbox",
            (Self::Hevc, true) => "hevc_videotoolbox",
        }
    }
}

/// How hard the CPU encoder works: slower is smaller at the same quality.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub enum Speed {
    Veryfast,
    Fast,
    Medium,
    #[default]
    Slow,
    Slower,
    Veryslow,
}

impl Speed {
    pub const ALL: [Self; 6] = [
        Self::Veryfast,
        Self::Fast,
        Self::Medium,
        Self::Slow,
        Self::Slower,
        Self::Veryslow,
    ];

    /// ffmpeg's own name for it, which is also the label.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Veryfast => "veryfast",
            Self::Fast => "fast",
            Self::Medium => "medium",
            Self::Slow => "slow",
            Self::Slower => "slower",
            Self::Veryslow => "veryslow",
        }
    }
}

/// What happens to the sound.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Audio {
    /// AAC at this many kbit/s.
    Aac(u16),
    /// The original sound, untouched.
    Copy,
    /// No sound.
    Remove,
}

impl Default for Audio {
    fn default() -> Self {
        Self::Aac(128)
    }
}

impl Audio {
    pub const ALL: [Self; 6] = [
        Self::Aac(96),
        Self::Aac(128),
        Self::Aac(192),
        Self::Aac(256),
        Self::Copy,
        Self::Remove,
    ];

    pub fn label(self) -> String {
        match self {
            Self::Aac(kbps) => format!("AAC {kbps}k"),
            Self::Copy => "Keep original".into(),
            Self::Remove => "None".into(),
        }
    }
}

/// Smoothing out sensor noise before encoding. Noise costs a lot of bits
/// and nobody misses it; too much smears fine detail like grass.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub enum Denoise {
    #[default]
    Off,
    Light,
    Medium,
}

impl Denoise {
    pub const ALL: [Self; 3] = [Self::Off, Self::Light, Self::Medium];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Light => "Light",
            Self::Medium => "Medium",
        }
    }

    /// ffmpeg's hqdn3d filter: half its default strength, and its default.
    const fn filter(self) -> Option<&'static str> {
        match self {
            Self::Off => None,
            Self::Light => Some("hqdn3d=2:1.5:3:2.25"),
            Self::Medium => Some("hqdn3d=4:3:6:4.5"),
        }
    }
}

/// The largest picture kept, by its short side, so portrait phone movies
/// shrink the same as landscape ones. Never enlarges.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub enum MaxSize {
    #[default]
    Original,
    P2160,
    P1440,
    P1080,
    P720,
}

impl MaxSize {
    pub const ALL: [Self; 5] = [
        Self::Original,
        Self::P2160,
        Self::P1440,
        Self::P1080,
        Self::P720,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Original => "Original",
            Self::P2160 => "4K",
            Self::P1440 => "1440p",
            Self::P1080 => "1080p",
            Self::P720 => "720p",
        }
    }

    const fn short_side(self) -> Option<u32> {
        match self {
            Self::Original => None,
            Self::P2160 => Some(2160),
            Self::P1440 => Some(1440),
            Self::P1080 => Some(1080),
            Self::P720 => Some(720),
        }
    }

    /// The scale filter: the short side at most `n`, the long side in
    /// proportion and even, as the encoders need.
    fn filter(self) -> Option<String> {
        self.short_side().map(|n| {
            format!(
                "scale=w='if(gte(iw,ih),-2,min(iw,{n}))':\
                 h='if(gte(iw,ih),min(ih,{n}),-2)'"
            )
        })
    }
}

/// How much smaller a result must be to be kept, in percent. Less, and
/// the original stays as it is.
pub const MIN_SAVINGS: [u8; 3] = [0, 10, 25];

/// The most movies compressed at once.
pub const MAX_AT_ONCE: u8 = 3;

/// The file the movie is written into.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize,
)]
pub enum Container {
    #[default]
    Mp4,
    Mov,
    /// Whatever the original was.
    Same,
}

impl Container {
    pub const ALL: [Self; 3] = [Self::Mp4, Self::Mov, Self::Same];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Mp4 => ".mp4",
            Self::Mov => ".mov",
            Self::Same => "Same as original",
        }
    }

    /// The extension for a movie made from `input`.
    pub fn extension(self, input: &Path) -> String {
        match self {
            Self::Mp4 => "mp4".into(),
            Self::Mov => "mov".into(),
            Self::Same => input
                .extension()
                .map(|ext| ext.to_string_lossy().to_lowercase())
                .unwrap_or_default(),
        }
    }
}

/// Everything about how a movie is encoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub codec: Codec,
    /// Encode on Apple's media engine (VideoToolbox): many times faster,
    /// somewhat bigger files than the CPU at the same quality.
    pub gpu: bool,
    /// 10-bit color: no banding in skies. HEVC only, since 10-bit H.264
    /// does not play on Macs.
    pub ten_bit: bool,
    /// The CPU encoders' quality: lower is better and bigger.
    pub crf: u8,
    /// The GPU encoder's quality, 1 to 100: higher is better and bigger.
    pub gpu_quality: u8,
    /// The CPU encoders' effort.
    pub speed: Speed,
    pub audio: Audio,
    pub container: Container,
    /// Give the result the original's dates.
    pub keep_dates: bool,
    /// Leave movies already in HEVC alone.
    pub skip_hevc: bool,
    pub denoise: Denoise,
    pub max_size: MaxSize,
    /// Keep a result only if it is at least this many percent smaller.
    pub min_saving: u8,
    /// Movies compressed side by side. The CPU encoders cannot use every
    /// core on one smaller movie, so two at once finish a batch sooner.
    pub at_once: u8,
}

impl Default for Options {
    fn default() -> Self {
        Self::AERIAL
    }
}

impl Options {
    /// Screen recordings and general video: H.264 that plays anywhere, in
    /// the original's file type.
    pub const VIDEO: Self = Self {
        codec: Codec::H264,
        gpu: false,
        ten_bit: false,
        crf: 28,
        gpu_quality: 60,
        speed: Speed::Slow,
        audio: Audio::Aac(128),
        container: Container::Same,
        keep_dates: false,
        skip_hevc: false,
        denoise: Denoise::Off,
        max_size: MaxSize::Original,
        min_saving: 10,
        at_once: 1,
    };

    /// Drone footage: HEVC 10-bit on the GPU, keeping the original's dates.
    pub const AERIAL: Self = Self {
        codec: Codec::Hevc,
        gpu: true,
        ten_bit: true,
        crf: 26,
        gpu_quality: 65,
        speed: Speed::Slow,
        audio: Audio::Aac(128),
        container: Container::Mp4,
        keep_dates: true,
        skip_hevc: true,
        denoise: Denoise::Off,
        max_size: MaxSize::Original,
        min_saving: 10,
        at_once: 1,
    };

    /// Whether the movie is written in 10-bit.
    pub fn ten_bit(self) -> bool {
        self.ten_bit && self.codec == Codec::Hevc
    }

    /// The quality in its encoder's own terms: "CRF 26", "Quality 65".
    pub fn quality_label(self) -> String {
        if self.gpu {
            format!("Quality {}", self.gpu_quality)
        } else {
            format!("CRF {}", self.crf)
        }
    }

    /// What the quality means, in a word or two.
    pub const fn quality_word(self) -> &'static str {
        if self.gpu {
            match self.gpu_quality {
                80.. => "near original",
                70..=79 => "high quality",
                60..=69 => "balanced",
                50..=59 => "small",
                _ => "smallest",
            }
        } else {
            match self.crf {
                0..=18 => "near original",
                19..=23 => "high quality",
                24..=28 => "balanced",
                29..=33 => "small",
                _ => "smallest",
            }
        }
    }

    /// Whether the quality can go a step better, or a step smaller.
    pub const fn can_nudge(self, better: bool) -> bool {
        match (self.gpu, better) {
            (true, true) => self.gpu_quality < MAX_GPU_QUALITY,
            (true, false) => self.gpu_quality > MIN_GPU_QUALITY,
            (false, true) => self.crf > MIN_CRF,
            (false, false) => self.crf < MAX_CRF,
        }
    }

    /// One step better quality and a bigger file, or the other way. The
    /// two encoders count in opposite directions; this does not.
    pub const fn nudge_quality(&mut self, better: bool) {
        if !self.can_nudge(better) {
            return;
        }
        match (self.gpu, better) {
            (true, true) => self.gpu_quality += 1,
            (true, false) => self.gpu_quality -= 1,
            (false, true) => self.crf -= 1,
            (false, false) => self.crf += 1,
        }
    }

    /// Where the compressed copy of `input` is written.
    pub fn output(self, input: &Path) -> PathBuf {
        let stem = input.file_stem().unwrap_or_default().to_string_lossy();
        let ext = self.container.extension(input);
        input.with_file_name(format!("{stem}_compressed.{ext}"))
    }

    /// Where the result ends up: the output, or, when replacing, the
    /// original's name with the new extension.
    pub fn result(self, input: &Path, replace: bool) -> PathBuf {
        if replace {
            input.with_extension(self.container.extension(input))
        } else {
            self.output(input)
        }
    }

    /// ffmpeg's arguments for encoding `input` into `output`. Only the
    /// main video and sound are kept, so phone and drone data streams
    /// cannot break the file; `has_audio` says whether there is sound.
    pub fn ffmpeg_args(
        self,
        input: &Path,
        output: &Path,
        has_audio: bool,
    ) -> Vec<String> {
        let mut args = Args::default();
        // -n never overwrites. -stats keeps the progress line the queue
        // reads, while -loglevel error keeps only real problems.
        args.push(&["-nostdin", "-n", "-hide_banner", "-loglevel", "error"]);
        args.push(&["-stats", "-hwaccel", "videotoolbox", "-i"]);
        args.path(input);
        args.push(&["-map", "0:v:0"]);
        let sound = has_audio && self.audio != Audio::Remove;
        if sound {
            args.push(&["-map", "0:a:0"]);
        }
        // Where and when it was filmed: the file's tags, which Photos reads
        // for the map and the date, and each stream's own, like rotation.
        args.push(&["-map_metadata", "0", "-map_metadata:s:v", "0:s:v"]);
        if sound {
            args.push(&["-map_metadata:s:a", "0:s:a"]);
        }
        // Smaller first, so the denoiser has fewer pixels to work on.
        let filters: Vec<String> = [
            self.max_size.filter(),
            self.denoise.filter().map(String::from),
        ]
        .into_iter()
        .flatten()
        .collect();
        if !filters.is_empty() {
            args.push(&["-vf", &filters.join(",")]);
        }
        args.push(&["-c:v", self.codec.encoder(self.gpu)]);
        if self.gpu {
            args.push(&["-q:v", &self.gpu_quality.to_string()]);
            if self.ten_bit() {
                args.push(&["-profile:v", "main10", "-pix_fmt", "p010le"]);
            }
            // Quality over speed; the media engine is fast either way.
            args.push(&["-prio_speed", "0"]);
        } else {
            args.push(&["-crf", &self.crf.to_string()]);
            args.push(&["-preset", self.speed.name()]);
            if self.ten_bit() {
                args.push(&["-pix_fmt", "yuv420p10le"]);
            }
            if self.codec == Codec::Hevc {
                args.push(&["-x265-params", "aq-mode=3:log-level=error"]);
            }
        }
        if self.codec == Codec::Hevc {
            // hvc1 is the tag QuickTime and Finder need to play HEVC.
            args.push(&["-tag:v", "hvc1"]);
        }
        match self.audio {
            Audio::Aac(kbps) if sound => {
                args.push(&["-c:a", "aac", "-b:a", &format!("{kbps}k")]);
            }
            Audio::Copy if sound => args.push(&["-c:a", "copy"]),
            _ => args.push(&["-an"]),
        }
        let ext = output
            .extension()
            .map(|ext| ext.to_string_lossy().to_lowercase());
        if matches!(ext.as_deref(), Some("mp4" | "mov" | "m4v")) {
            args.push(&["-movflags", "+faststart+use_metadata_tags"]);
        }
        args.path(output);
        args.0
    }

    /// ffmpeg's arguments for encoding only `seconds` of `input`, from
    /// `start`: a sample to judge the settings by.
    pub fn sample_args(
        self,
        input: &Path,
        output: &Path,
        has_audio: bool,
        start: f64,
        seconds: f64,
    ) -> Vec<String> {
        let mut args = self.ffmpeg_args(input, output, has_audio);
        let at = args.iter().position(|arg| arg == "-i").unwrap_or(0);
        args.splice(
            at..at,
            [
                "-ss".to_string(),
                format!("{start:.2}"),
                "-t".to_string(),
                format!("{seconds:.2}"),
            ],
        );
        args
    }

    /// The same options a few quality steps better (`steps` > 0) or
    /// smaller. GPU steps are bigger: its scale is finer.
    #[must_use]
    pub fn stepped(self, steps: i8) -> Self {
        let mut options = self;
        let unit: i8 = if self.gpu { 5 } else { 3 };
        for _ in 0..(steps.unsigned_abs() * unit.unsigned_abs()) {
            options.nudge_quality(steps > 0);
        }
        options
    }
}

#[derive(Default)]
struct Args(Vec<String>);

impl Args {
    fn push(&mut self, items: &[&str]) {
        self.0.extend(items.iter().map(|item| (*item).to_string()));
    }

    fn path(&mut self, path: &Path) {
        self.0.push(path.to_string_lossy().into_owned());
    }
}

/// A command as it would be typed into a shell.
pub fn command_line(program: &str, args: &[String]) -> String {
    let mut line = program.to_string();
    for arg in args {
        line.push(' ');
        let plain = !arg.is_empty()
            && arg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.,:/+=@%".contains(c));
        if plain {
            line.push_str(arg);
        } else {
            line.push('\'');
            line.push_str(&arg.replace('\'', r"'\''"));
            line.push('\'');
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(options: Options, input: &str, has_audio: bool) -> String {
        let input = Path::new(input);
        let output = options.output(input);
        command_line("ffmpeg", &options.ffmpeg_args(input, &output, has_audio))
    }

    #[test]
    fn the_gpu_command_matches_compress_aerial() {
        assert_eq!(
            line(Options::AERIAL, "/m/DJI 0001.MOV", true),
            "ffmpeg -nostdin -n -hide_banner -loglevel error -stats \
             -hwaccel videotoolbox -i '/m/DJI 0001.MOV' -map 0:v:0 \
             -map 0:a:0 -map_metadata 0 -map_metadata:s:v 0:s:v \
             -map_metadata:s:a 0:s:a -c:v hevc_videotoolbox -q:v 65 \
             -profile:v main10 -pix_fmt p010le -prio_speed 0 -tag:v hvc1 \
             -c:a aac -b:a 128k -movflags +faststart+use_metadata_tags \
             '/m/DJI 0001_compressed.mp4'"
        );
    }

    #[test]
    fn the_cpu_command_uses_the_crf() {
        let cpu = Options {
            gpu: false,
            ..Options::AERIAL
        };
        let args = line(cpu, "/m/a.mov", false);
        assert!(
            args.contains(
                "-c:v libx265 -crf 26 -preset slow -pix_fmt yuv420p10le \
                 -x265-params aq-mode=3:log-level=error -tag:v hvc1 -an"
            ),
            "{args}"
        );
        let video = line(Options::VIDEO, "/m/b.WMV", true);
        assert!(
            video.contains("-c:v libx264 -crf 28 -preset slow -c:a aac"),
            "{video}"
        );
        assert!(video.ends_with("/m/b_compressed.wmv"), "{video}");
    }

    #[test]
    fn better_quality_goes_the_right_way_on_both_encoders() {
        let mut gpu = Options::AERIAL;
        gpu.nudge_quality(true);
        assert_eq!(gpu.gpu_quality, 66);
        let mut cpu = Options::VIDEO;
        cpu.nudge_quality(true);
        assert_eq!(cpu.crf, 27);
        cpu.crf = MIN_CRF;
        cpu.nudge_quality(true);
        assert_eq!(cpu.crf, MIN_CRF, "stops at the end of the range");
    }

    #[test]
    fn outputs_follow_the_container() {
        let input = Path::new("/m/Clip One.WMV");
        assert_eq!(
            Options::VIDEO.output(input),
            Path::new("/m/Clip One_compressed.wmv")
        );
        assert_eq!(
            Options::AERIAL.result(input, true),
            Path::new("/m/Clip One.mp4")
        );
        let h264 = Options {
            codec: Codec::H264,
            ..Options::AERIAL
        };
        let args = line(h264, "/m/a.mov", true);
        assert!(!args.contains("p010le"), "10-bit is HEVC only: {args}");
        assert!(!args.contains("hvc1"), "{args}");
    }

    #[test]
    fn filters_shrink_then_denoise() {
        let options = Options {
            gpu: false,
            denoise: Denoise::Light,
            max_size: MaxSize::P1080,
            ..Options::AERIAL
        };
        let args = line(options, "/m/a.mov", true);
        assert!(
            args.contains(
                "-vf 'scale=w='\\''if(gte(iw,ih),-2,min(iw,1080))'\\''"
            ),
            "{args}"
        );
        assert!(args.contains(",hqdn3d=2:1.5:3:2.25'"), "{args}");
        let sample = options.sample_args(
            Path::new("/m/a.mov"),
            Path::new("/s/a.mp4"),
            true,
            12.5,
            10.0,
        );
        let at = sample.iter().position(|a| a == "-i").expect("-i");
        assert_eq!(sample[at - 4..at], ["-ss", "12.50", "-t", "10.00"]);
        assert_eq!(Options::VIDEO.stepped(-1).crf, 31);
        assert_eq!(Options::AERIAL.stepped(1).gpu_quality, 70);
    }
}
