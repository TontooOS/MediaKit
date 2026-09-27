# ProRes

Native `.mov` and ProRes support without ffmpeg: ISO-BMFF box walk
(`mov`), SMPTE RDD 36 frame-header validate (`prores`) and a GPU
slice plan for the future WGPU compute decoder.

## `mov`

```rust
pub fn sniff_mov(path: &Path) -> bool
```

- Returns `true` for `ftyp`-confirmed QuickTime/ISO-BMFF files.
- Returns `false` for missing or non-MOV files, never `Err`.

```rust
pub fn read_mov_info(path: &Path) -> Result<MovInfo>
```

- Walks top-level boxes with `seek`, buffers only `moov`.
- Returns `Err(ParseError)` when `moov` is missing or truncated.
- Powers `read_metadata` natively for `.mov`/`.mp4`/`.m4v`.

### `MovInfo`

| Field | Type | Description |
|---|---|---|
| `major_brand` | `String` | `ftyp` brand, e.g. `"qt  "` |
| `duration_secs` | `f64` | `mvhd` duration / timescale |
| `width` | `u32` | Video width from `stsd` or `tkhd` |
| `height` | `u32` | Video height from `stsd` or `tkhd` |
| `video_fourcc` | `String` | Sample entry, e.g. `"apch"` |
| `audio_fourcc` | `Option<String>` | Sample entry, e.g. `"sowt"` |
| `framerate` | `f64` | `stts` estimate at video timescale |
| `has_moov` | `bool` | Always true on success |

## `prores`

```rust
pub fn profile_from_fourcc(fourcc: &str) -> ProResProfile
```

- Maps `apco`/`apcs`/`apcn`/`apch`/`ap4h`/`ap4x`, case-insensitive.
- Returns `ProResProfile::Unknown` for other codecs.

```rust
pub fn parse_frame_header(buf: &[u8]) -> Result<ProResFrameHeader>
```

- Validates `icpf` magic, version 0-1 and dimensions 1-16384.
- Returns `Err(ParseError)` on bad magic, version or truncation.

```rust
pub fn gpu_slice_plan(width: u32, height: u32, slices: u16) -> Vec<(u32, u32)>
```

- Returns 16px-aligned `(y_start, y_end)` bands, max 32 slices.
- Pure math for WGPU compute dispatch, no wgpu dependency.

### `ProResProfile`

| Variant | Meaning |
|---|---|
| `Proxy` | `apco`, 4:2:2, no alpha |
| `Lt` | `apcs`, 4:2:2, no alpha |
| `Standard` | `apcn`, 4:2:2, no alpha |
| `Hq` | `apch`, 4:2:2, no alpha |
| `FourFourFourFour` | `ap4h`, 4:4:4, alpha |
| `FourFourFourFourXq` | `ap4x`, 4:4:4, alpha |

## `transcode_prores`

```rust
pub fn transcode_prores(input: &Path, output: &Path, profile: ProResProfile) -> Result<()>
```

- Validates `input` via `probe_container` first.
- Executes via ffmpeg `prores_ks` until the native GPU encoder lands.
- Returns `Err` for unsupported inputs or failed encodes.

## Usage / Example

```rust
use mediakit::{read_mov_info, profile_from_fourcc};
use std::path::Path;

let info = read_mov_info(Path::new("clip.mov"))?;
let profile = profile_from_fourcc(&info.video_fourcc);
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Metadata.md](Metadata.md) – native MOV metadata path
- [Playback.md](Playback.md) – `.mov` opens offline
- [Editing.md](Editing.md) – ProRes transcode targets
- [Ffi.md](Ffi.md) – `tontoo_mediakit_mov_info` JSON
