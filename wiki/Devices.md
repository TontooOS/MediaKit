# Devices

Available cameras/capture devices and video outputs.

## Cameras

```rust
let cameras = mediakit::available_cameras()?;
```

- Delegates to `capture::list_cameras` (sysfs plus direct
  `/dev/video` scan, no tools).
- See [Capture.md](Capture.md) for the `CameraDevice` shape.

## Outputs

```rust
let outputs = mediakit::available_outputs();
for out in &outputs {
    println!("{} primary={}", out.id, out.primary);
}
```

- Enumerates DRM/KMS connectors on Linux (`/dev/dri/card*`,
  no tools); empty elsewhere.
- Picks the preferred mode, else the largest, per connector.
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

- [Capture.md](Capture.md) – stills on these cameras
- [Playback.md](Playback.md) – fullscreen target outputs
