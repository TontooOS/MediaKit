# Mkv

Native Matroska / WebM (`.mkv`, `.webm`) support without ffmpeg:
EBML walk (`mkv`), metadata and chapters fallback.

## `sniff_mkv`

```rust
pub fn sniff_mkv(path: &Path) -> bool
```

- Returns `true` for EBML-magic (`0x1A45DFA3`) files.
- Returns `false` for missing or non-MKV files, never `Err`.

## `read_mkv_info`

```rust
pub fn read_mkv_info(path: &Path) -> Result<MkvInfo>
```

- Parses the `EBML` header (`matroska` / `webm` doctypes only),
  `Info` (timescale, duration) and `Tracks` (codec IDs, resolution,
  `DefaultDuration`).
- Falls back to a cluster-timestamp scan when `Info` duration is
  missing; files over 512 MB return `ParseError`.
- Returns `Err(ParseError)` without a `Segment` or with a foreign
  doctype.

### `MkvInfo`

| Field | Type | Description |
|---|---|---|
| `doctype` | `String` | `"matroska"` or `"webm"` |
| `duration_secs` | `f64` | `Info` duration, else cluster scan |
| `width` | `u32` | First video track pixel width |
| `height` | `u32` | First video track pixel height |
| `video_codec` | `String` | Codec ID, e.g. `"V_MPEG4/ISO/AVC"` |
| `audio_codec` | `Option<String>` | Codec ID, e.g. `"A_AAC"` |
| `framerate` | `f64` | From `DefaultDuration`, 0 when absent |
| `has_segment` | `bool` | Always true on success |

## `read_mkv_metadata`

```rust
pub fn read_mkv_metadata(path: &Path) -> Result<VideoMetadata>
```

- Native-first path used by `read_metadata` for `.mkv`/`.webm`.
- Falls back to ffprobe for damaged files (see
  [Metadata.md](Metadata.md)).

## Usage / Example

```rust
use mediakit::read_mkv_info;
use std::path::Path;

let info = read_mkv_info(Path::new("clip.mkv"))?;
assert_eq!(info.doctype, "matroska");
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Metadata.md](Metadata.md) – native MKV metadata path
- [Playback.md](Playback.md) – `.mkv` opens offline
- [ProRes.md](ProRes.md) – native MOV parser details
- [Ffi.md](Ffi.md) – `tontoo_mediakit_mkv_info` JSON
