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

## `transcode_prores`

```rust
pub fn transcode_prores(input: &Path, output: &Path, profile: ProResProfile) -> Result<()>
```

- Validates `input` via `probe_container` first.
- Executes via ffmpeg `prores_ks` until the native GPU encoder lands.

## `trim_mov_native`

```rust
pub fn trim_mov_native(input: &Path, start_secs: f64, end_secs: f64, output: &Path) -> Result<()>
```

- Pure Rust, no ffmpeg: every video/audio track is cut in its own
  timescale; sample tables (`stts`, `stsc`, `stsz`, `stco`/`co64`,
  `stss`, `ctts`) and durations (`mvhd`, `mdhd`, `tkhd`) are
  rewritten, frame bytes land in one chunk per track.
- Works on multi-track `.mov` files of any codec (including ProRes
  and raw video plus PCM `sowt` audio); output is faststart
  (`ftyp` + `moov` + `mdat`).
- Returns `Err(MediaError::InvalidSeek)` for bad ranges or empty
  selections, `Err(MediaError::ParseError)` for edit lists, compact
  sample tables or non-A/V tracks.
- Returns `Err(MediaError::UnsupportedFormat)` unless `output` ends
  in `.mov`, `.mp4` or `.m4v`.

## `build_raw_mov`

```rust
pub fn build_raw_mov(params: &RawMovParams, frames: &[Vec<u8>]) -> Result<Vec<u8>>
```

- Builds a minimal playable raw-RGB24 `.mov` (codec `raw `).
- Each frame must be `width * height * 3` bytes; `fps` must divide 600.

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
- [ProRes.md](ProRes.md) – native MOV parser details
