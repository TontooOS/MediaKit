# MediaKit – Wiki

MediaKit is the video framework for TontooOS. It covers file playback,
camera capture, network streaming, metadata, thumbnails and basic editing
on top of ffmpeg, V4L2, NetworkKit and AudioKit, without any cloud
dependency.

- Repository: https://github.com/TontooOS/Libs
- License: TCL
- Version: 26.1.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| Playback | [Playback.md](Playback.md) | `VideoPlayer` file playback, speed, fullscreen, subtitles, chapters |
| Capture | [Capture.md](Capture.md) | Camera access, recording, quality selection, snapshots |
| Streaming | [Streaming.md](Streaming.md) | HTTP progressive and HLS via NetworkKit, buffering events |
| Metadata | [Metadata.md](Metadata.md) | Duration, resolution, codec, framerate natively for MOV plus ffprobe |
| Thumbnails | [Thumbnails.md](Thumbnails.md) | Frame extraction for Finder previews |
| Editing | [Editing.md](Editing.md) | Trim, concat and transcode via ffmpeg |
| ProRes | [ProRes.md](ProRes.md) | Native `.mov` parser and ProRes profiles plus GPU slice plan |
| Devices | [Devices.md](Devices.md) | Camera and video-output listing |
| Controls | [Controls.md](Controls.md) | SF Symbols via CoreIcon, SF Pro via CoreText, theme colors |
| Ffi | [Ffi.md](Ffi.md) | C ABI (`tontoo_mediakit_*`) and JSON shapes |
| Localization | [Localization.md](Localization.md) | Error message localization via `lang/en_us.json` and `lang/de_de.json` |

## Quick Start

```rust
use mediakit::MediaKit;
use std::path::Path;

let mut kit = MediaKit::new();
kit.player_mut().open(Path::new("clip.mp4")).unwrap();
kit.player_mut().play().unwrap();
kit.player_mut().set_speed(1.5).unwrap();
```

See [Playback.md](Playback.md) for details.

## Changelog

- 2026-09-27: Native `.mov`/ProRes support (`mov`, `prores`, `transcode_prores`, FFI).
- 2026-09-26: Initial wiki with Playback, Capture, Streaming, Metadata, Thumbnails, Editing, Devices, Controls, Ffi and Localization pages.
