# Editing

Lossless native editing of `.mov` files: trim/cut (start/end export)
and multi-clip concat. Pure Rust, no external binaries.
Re-encoding (transcoding) is intentionally absent until the native
encoder milestone lands.

## `trim_mov_native`

```rust
pub fn trim_mov_native(input: &Path, start_secs: f64, end_secs: f64, output: &Path) -> Result<()>
```

- Returns `Err(MediaError::InvalidSeek)` when `end_secs <= start_secs`.
- Every video/audio track is cut in its own timescale; sample
  tables and durations are rewritten, one chunk per track.
- `output` must end in `.mov`, `.mp4` or `.m4v`.

## `concat_mov_native`

```rust
pub fn concat_mov_native(inputs: &[&Path], output: &Path) -> Result<()>
```

- Appends clips with matching track layout, codecs, dimensions and
  timescales; samples concatenate per track in input order.
- Returns `Err` when `inputs` is empty or tracks mismatch.

## Usage / Example

```rust
use mediakit::{trim_mov_native, concat_mov_native};
use std::path::Path;

trim_mov_native(Path::new("in.mov"), 5.0, 15.0, Path::new("out.mov"))?;
concat_mov_native(&[Path::new("a.mov"), Path::new("b.mov")], Path::new("joined.mov"))?;
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Capture.md](Capture.md) – native stills feed edits here
- [Metadata.md](Metadata.md) – verify durations before cutting
- [ProRes.md](ProRes.md) – native MOV parser details
