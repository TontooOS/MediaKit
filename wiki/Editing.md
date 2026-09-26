# Editing

Basic editing: trim/cut (start/end export), multi-clip concat and
compression/transcoding between formats via ffmpeg.

## `trim`

```rust
pub fn trim(input: &Path, start_secs: f64, end_secs: f64, output: &Path, preset: ExportPreset) -> Result<()>
```

- Returns `Err(MediaError::InvalidSeek)` when `end_secs <= start_secs`.
- `ExportPreset::Copy` is fast (`-c copy`); `H264Fast` and `WebM`
  re-encode.

## `concat`

```rust
pub fn concat(inputs: &[&Path], output: &Path) -> Result<()>
```

- Uses the ffmpeg concat demuxer with a temp file list.
- Returns `Err` when `inputs` is empty.
- Clips must share codecs (stream copy, no re-encode).

## `transcode`

```rust
pub fn transcode(input: &Path, output: &Path, preset: ExportPreset) -> Result<()>
```

- `ExportPreset::H264Fast` writes H264 + AAC, `WebM` writes VP9 + Opus.
- Output container follows the file extension.

### `ExportPreset`

| Variant | Meaning |
|---|---|
| `Copy` | Stream copy, no re-encode |
| `H264Fast` | `libx264 fast` + `aac` |
| `WebM` | `libvpx-vp9` + `libopus` |

## Usage / Example

```rust
use mediakit::{trim, ExportPreset};
use std::path::Path;

trim(Path::new("in.mp4"), 5.0, 15.0, Path::new("out.mp4"), ExportPreset::Copy)?;
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Capture.md](Capture.md) – takes that get trimmed here
- [Metadata.md](Metadata.md) – verify durations before cutting
