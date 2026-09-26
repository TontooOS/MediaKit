# Metadata

Video metadata (duration, resolution, codec, framerate) via ffprobe.

## `read_metadata`

```rust
pub fn read_metadata(path: &Path) -> Result<VideoMetadata>
```

- Runs `ffprobe -v quiet -print_format json -show_format -show_streams`.
- Returns `Err(MediaError::FfmpegMissing)` when ffprobe is not on PATH
  (surfaced as `IoError` NotFound mapping).
- Returns `Err(MediaError::ParseError)` on unparsable JSON.

### `VideoMetadata`

| Field | Type | Description |
|---|---|---|
| `duration_secs` | `f64` | Container duration in seconds |
| `width` | `u32` | Video width in pixels |
| `height` | `u32` | Video height in pixels |
| `video_codec` | `String` | Codec name, e.g. `"h264"` |
| `audio_codec` | `Option<String>` | Audio codec, e.g. `"aac"` |
| `framerate` | `f64` | Parsed `avg_frame_rate` (`30000/1001` to 29.97) |
| `container` | `String` | ffprobe `format_name` |
| `size_bytes` | `Option<u64>` | File size when reported |

## Usage / Example

```rust
let meta = mediakit::read_metadata(std::path::Path::new("clip.mp4"))?;
println!("{:.1}s {}x{}", meta.duration_secs, meta.width, meta.height);
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Playback.md](Playback.md) – duration feeds the player clock
- [Thumbnails.md](Thumbnails.md) – frame extraction for the same files
