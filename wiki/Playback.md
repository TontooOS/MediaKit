# Playback

File playback (MP4, WebM, MKV, AVI) with Play, Pause, Stop, Seek, speed
control, fullscreen switching, subtitles and chapters. Volume and mute
are delegated to AudioKit instead of being reimplemented.

Two output paths exist: the native frame provider (`frame_at`,
no binaries) and the legacy external player (`spawn_external`
with `mpv`/`ffplay`).

## Native frames

```rust
pub fn frame_at(&self, position_secs: f64) -> Result<NativeFrame>
```

- Decodes the frame at `position_secs` to CPU-side RGB24 via
  `decode_video_frame` (raw, TDC-1, MJPEG for MOV; MJPEG/RGB
  for AVI).
- Seeks are exact (sample PTS) and independent of `seek`.
- Apps upload `NativeFrame.rgb` to a WGPU texture.
- Returns `Err(MediaError::NotAvailable)` with no open file.

### `NativeFrame`

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Frame width in pixels |
| `height` | `u32` | Frame height in pixels |
| `pts_secs` | `f64` | Presentation timestamp |
| `rgb` | `Vec<u8>` | Packed RGB24 |

## VideoPlayer

Core transport state. Real output spawns `mpv` (preferred) or `ffplay`
via `spawn_external`; all other transitions are pure Rust.

```rust
let mut player = mediakit::VideoPlayer::new();
player.open(std::path::Path::new("clip.mp4"))?;
player.play()?;
player.set_speed(1.5)?;
player.seek(10.0)?;
```

### `VideoPlayer::open`

```rust
pub fn open(&mut self, path: &Path) -> Result<()>
```

- Validates the container extension (MP4, WebM, MKV, AVI, MOV, M4V).
- Returns `Err` when the file is missing or the format is unsupported.
- Resets position and loads duration via ffprobe when available.

### `VideoPlayer::play`

```rust
pub fn play(&mut self) -> Result<()>
```

- Returns `Err(MediaError::NotAvailable)` with no open file.
- Sets state to `Playing`.

### `VideoPlayer::pause`

```rust
pub fn pause(&mut self) -> Result<()>
```

- No-op unless currently `Playing`.
- Sets state to `Paused`.

### `VideoPlayer::stop`

```rust
pub fn stop(&mut self) -> Result<()>
```

- Resets position to 0 and sets state to `Stopped`.

### `VideoPlayer::seek`

```rust
pub fn seek(&mut self, position_secs: f64) -> Result<()>
```

- Returns `Err(MediaError::InvalidSeek)` for negative, non-finite or
  beyond-duration positions.
- Seeking to exactly the duration sets state to `Finished`.

### `VideoPlayer::set_speed`

```rust
pub fn set_speed(&mut self, speed: f32) -> Result<()>
```

- Valid range is 0.5x-2.0x (`MIN_SPEED` to `MAX_SPEED`).
- Returns `Err(MediaError::InvalidSpeed)` outside the range.

### Volume and mute

```rust
player.set_volume(0.5);
player.set_muted(true);
```

- Stored on the player for UI state only.
- System mixer calls go through `mediakit::audio` which forwards to
  `audiokit::SystemVolume` (PipeWire via pactl/wpctl).

### Fullscreen

```rust
let mode = player.toggle_fullscreen();
```

- Toggles between `DisplayMode::Windowed` and `DisplayMode::Fullscreen`.
- Passed to `mpv` as `--fs` / `--no-fs` in `spawn_external`.

## Usage / Example

```rust
use mediakit::MediaKit;
use std::path::Path;

let mut kit = MediaKit::new();
kit.player_mut().open(Path::new("clip.mkv"))?;
kit.player_mut().play()?;
kit.player_mut().tick(2.0);
kit.player_mut().pause()?;
```

## Cross References

- [Metadata.md](Metadata.md) – duration source for the player clock
- [Controls.md](Controls.md) – transport bar icons and labels
- [Ffi.md](Ffi.md) – C transport functions
