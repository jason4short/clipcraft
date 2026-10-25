# clipcraft

A small macOS app for compressing movies. Drop movies or folders on the
window, pick a preset, and press Start. clipcraft builds the ffmpeg command
for each movie from the preset and runs them one after another, showing
progress, speed, time left and how much space each one saved.

## What it does

- **Presets** hold every encoding option: GPU (VideoToolbox) or CPU
  (x264/x265), H.264 or HEVC, 8- or 10-bit, quality, speed, sound, file
  type, denoise, max resolution. Settings shows the exact command each
  movie gets.
- **Safe replacing**: with *Replace originals* on, an original is deleted
  only after the new file plays for the same length, and never if it would
  overwrite a different movie.
- **Keeps what matters**: file dates, creation date and GPS location tags,
  so Photos still maps and sorts the clips.
- **Skips what would not help**: movies already in HEVC, and results that
  are not smaller than the original by the preset's margin.
- **Try settings**: encodes ten seconds at the preset's quality and a step
  either side, with the projected size of the whole movie, to watch side by
  side in QuickTime.
- **Overnight runs**: keeps the Mac awake while it works, sends a
  notification when the queue is done, and can sleep the Mac after. An
  estimate of the final size and finish time appears after a minute.
- **The queue**: add movies while it runs, drag to reorder, sort by size,
  find movies in a folder by name, select and copy paths, Delete to remove.
  The list is kept between launches.

## Building

Needs Rust and ffmpeg (`brew install ffmpeg`).

```sh
make app        # builds target/clipcraft.app
make install    # copies it to ~/Applications
cargo xtask ci  # format check, clippy, tests
```

## License

MIT
