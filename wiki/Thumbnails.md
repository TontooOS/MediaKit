# Thumbnails

Thumbnail and frame extraction (single preview image from a video, e.g.
for the Finder) via ffmpeg, plus a pure-Rust path for native codecs.

## `extract_frame`

```rust
pub fn extract_frame(input: &Path, at_secs: f64, output: &Path) -> Result<()>
```

- Runs `ffmpeg -y -ss <at> -i <input> -vframes 1 <output>`.
- Works for MP4, WebM, MKV and AVI without extra codecs.
- Output format follows the extension (PNG/JPG).
- Returns `Err(MediaError::InvalidSeek)` for negative positions.

## `extract_frame_native`

```rust
pub fn extract_frame_native(input: &Path, at_secs: f64, output: &Path) -> Result<()>
```

- Pure Rust, no ffmpeg. Supports `.mov`/`.mp4`/`.m4v` with raw,
  TDC-1 (`icpf`) or MJPEG tracks, and `.avi` with MJPEG (`dc`)
  or raw RGB (`db`) chunks.
- `output` must end in `.png` (written by the native PNG writer).
- Returns `Err(MediaError::UnsupportedFormat)` for foreign codecs.

## `extract_args`

```rust
pub fn extract_args(input: &Path, at_secs: f64, output: &Path) -> Vec<String>
```

- Returns the ffmpeg argument vector without spawning anything.
- Used by tests and by apps that manage their own ffmpeg process.

## Usage / Example

```rust
mediakit::extract_frame(
    std::path::Path::new("clip.mp4"),
    5.0,
    std::path::Path::new("thumb.png"),
)?;
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Metadata.md](Metadata.md) – pick `at_secs` inside the real duration
- [Editing.md](Editing.md) – same ffmpeg backend
- [Mjpeg.md](Mjpeg.md) – native JPEG decode behind thumbnails
