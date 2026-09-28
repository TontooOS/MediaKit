# Thumbnails

Thumbnail and frame extraction (single preview image from a video, e.g.
for the Finder). Pure Rust, no external binaries.

## `extract_frame_native`

```rust
pub fn extract_frame_native(input: &Path, at_secs: f64, output: &Path) -> Result<()>
```

- Supports `.mov`/`.mp4`/`.m4v` with raw, TDC-1 (`icpf`) or MJPEG
  tracks, and `.avi` with MJPEG (`dc`) or raw RGB (`db`) chunks.
- `output` must end in `.png` (native PNG writer).
- Returns `Err(MediaError::InvalidSeek)` for negative positions,
  `Err(MediaError::UnsupportedFormat)` for foreign codecs.

## `decode_video_frame`

```rust
pub fn decode_video_frame(input: &Path, at_secs: f64) -> Result<NativeFrame>
```

- Same codecs as above, returns CPU-side RGB24 for WGPU upload.
- Backs `extract_frame_native` and `VideoPlayer::frame_at`.

### `NativeFrame`

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Frame width in pixels |
| `height` | `u32` | Frame height in pixels |
| `pts_secs` | `f64` | Presentation timestamp |
| `rgb` | `Vec<u8>` | Packed RGB24 |

## Usage / Example

```rust
mediakit::extract_frame_native(
    std::path::Path::new("clip.mov"),
    5.0,
    std::path::Path::new("thumb.png"),
)?;
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Metadata.md](Metadata.md) – pick `at_secs` inside the real duration
- [Mjpeg.md](Mjpeg.md) – native JPEG decode behind thumbnails
