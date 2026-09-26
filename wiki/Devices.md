# Devices

Available cameras/capture devices and video outputs.

## Cameras

```rust
let cameras = mediakit::available_cameras()?;
```

- Delegates to `capture::list_cameras` (`/sys/class/video4linux` with
  `v4l2-ctl` fallback).
- See [Capture.md](Capture.md) for the `CameraDevice` shape.

## Outputs

```rust
let outputs = mediakit::available_outputs();
for out in &outputs {
    println!("{} primary={}", out.id, out.primary);
}
```

- Probes `swaymsg -t get_outputs` (TontooCompositor/Wayland) first.
- Falls back to `xrandr --query`.
- Returns an empty list on headless machines instead of an error.

### `VideoOutput`

| Field | Type | Description |
|---|---|---|
| `id` | `String` | Output name, e.g. `"eDP-1"` |
| `label` | `String` | Display label (same as id) |
| `primary` | `bool` | True for the first/active output |
| `width` | `Option<u32>` | Mode width when reported |
| `height` | `Option<u32>` | Mode height when reported |

## Usage / Example

```rust
let cameras = mediakit::available_cameras().unwrap_or_default();
let outputs = mediakit::available_outputs();
println!("{} cameras, {} outputs", cameras.len(), outputs.len());
```

## Cross References

- [Capture.md](Capture.md) – recording on these cameras
- [Playback.md](Playback.md) – fullscreen target outputs
