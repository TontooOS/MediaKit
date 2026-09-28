# Capture

Camera access, live-preview device info, recording start/stop,
format/quality selection, camera selection and still snapshots from the
live stream via V4L2 and ffmpeg.

## Camera listing

```rust
let cameras = mediakit::list_cameras()?;
for cam in &cameras {
    println!("{} {}", cam.id, cam.label);
}
```

- Reads `/sys/class/video4linux` first (no binary needed).
- Falls back to `v4l2-ctl --list-devices`.
- Returns an empty list on headless machines, never an error for that
  case.

### `CameraDevice`

| Field | Type | Description |
|---|---|---|
| `id` | `String` | Node name, e.g. `"video0"` |
| `label` | `String` | Human label from sysfs or v4l2-ctl |
| `node` | `PathBuf` | Device node, e.g. `/dev/video0` |
| `kind` | `CameraKind` | `BuiltIn`, `Usb`, `Virtual` or `Unknown` |

## Recording

```rust
let out = std::path::Path::new("take.mp4");
let pid = mediakit::start_capture(
    &cameras[0],
    out,
    mediakit::CaptureQuality::Medium,
    mediakit::CaptureFormat::Mp4,
)?;
```

- Shells to `ffmpeg -f v4l2 -video_size WxH -i /dev/videoN <output>`.
- Returns the ffmpeg PID; apps stop it with SIGTERM/SIGINT.
- `CaptureQuality` maps to 640x480 (Low), 1280x720 (Medium) and
  1920x1080 (High, FullHd).
- `CaptureFormat` selects the output extension (Mp4, WebM, Mkv).

## Native stills (no ffmpeg, Linux only)

```rust
let frame = mediakit::capture_still_native(&cameras[0].node)?;
```

- Drives `/dev/videoN` with raw ioctls (no libc crate):
  `QUERYCAP`, `S_FMT`, `STREAMON`, blocking `read()`, `STREAMOFF`.
- Prefers MJPEG 1280x720 (decoded by `Mjpeg.md`), falls back to
  YUYV 640x480 (converted in-crate).
- `capture_frame_native(node, width, height, fourcc)` takes
  `FOURCC_MJPG` or `FOURCC_YUYV`; the driver may adjust the size
  (reported back in `CapturedFrame`).
- Needs real hardware; without a camera it returns `Err`.

### `CapturedFrame`

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Negotiated width in pixels |
| `height` | `u32` | Negotiated height in pixels |
| `rgb` | `Vec<u8>` | Packed RGB24 |

## Snapshots

```rust
mediakit::snapshot(&cameras[0], std::path::Path::new("still.png"))?;
```

- Grabs one frame with `ffmpeg -vframes 1`.
- Output format follows the file extension (PNG/JPG).

## Usage / Example

```rust
let cameras = mediakit::available_cameras().unwrap_or_default();
if let Some(cam) = cameras.first() {
    mediakit::snapshot(cam, std::path::Path::new("/tmp/still.png"))?;
}
# Ok::<(), mediakit::MediaError>(())
```

## Cross References

- [Devices.md](Devices.md) – unified camera + output listing
- [Editing.md](Editing.md) – what happens to takes after capture
