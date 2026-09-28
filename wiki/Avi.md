# Avi

Native AVI (RIFF) support without ffmpeg: header walk (`avi`),
metadata and chapters fallback.

## `sniff_avi`

```rust
pub fn sniff_avi(path: &Path) -> bool
```

- Returns `true` for `RIFF....AVI ` magic files.
- Returns `false` for missing or non-AVI files, never `Err`.

## `read_avi_info`

```rust
pub fn read_avi_info(path: &Path) -> Result<AviInfo>
```

- Buffers `hdrl` (`avih` + one `strl` per stream), skips `movi`
  payloads via seeking; files over 512 MB return `ParseError`.
- Video fourcc prefers the `strf` `BITMAPINFOHEADER` entry over
  `strh`; audio names come from the `WAVEFORMATEX` tag map
  (`PCM`, `MP3`, `AAC`, ...) with hex fallback.
- OpenDML spanned files (`AVIX`) return `ParseError` (v1 scope is
  classic AVI 1.0).

### `AviInfo`

| Field | Type | Description |
|---|---|---|
| `duration_secs` | `f64` | `avih` frames, else stream length |
| `width` | `u32` | `avih` width, else video `strf` |
| `height` | `u32` | Absolute `strf` height (bottom-up safe) |
| `video_fourcc` | `String` | Codec, e.g. `"MJPG"` |
| `audio_codec` | `Option<String>` | Format name, e.g. `"PCM"` |
| `framerate` | `f64` | `avih` rate, else stream rate |
| `stream_count` | `u32` | `avih` stream count |
| `has_movi` | `bool` | `movi` list seen on disk |

## `read_avi_metadata`

```rust
pub fn read_avi_metadata(path: &Path) -> Result<VideoMetadata>
```

- Native-first path used by `read_metadata` for `.avi`.
- Falls back to ffprobe for damaged files (see
  [Metadata.md](Metadata.md)).

## Usage / Example

```rust
use mediakit::read_avi_info;
use std::path::Path;

let info = read_avi_info(Path::new("clip.avi"))?;
assert_eq!(info.container_name(), "avi");
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Metadata.md](Metadata.md) – native AVI metadata path
- [Playback.md](Playback.md) – `.avi` opens offline
- [Mkv.md](Mkv.md) – native Matroska parser details
- [Ffi.md](Ffi.md) – `tontoo_mediakit_avi_info` JSON
