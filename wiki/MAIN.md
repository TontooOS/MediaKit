# MediaKit – Wiki

MediaKit is the video framework for TontooOS. It covers native file
parsing, decode, camera capture, network streaming, metadata,
thumbnails and lossless editing with pure Rust and no external
binaries, without any cloud dependency.

- Repository: https://github.com/TontooOS/Libs
- License: TCL
- Version: 27.0.0

## Feature Index

| Feature | File | Description |
|---|---|---|
| Main index | [MAIN.md](MAIN.md) | This page |
| Rules | [RULE.md](RULE.md) | Development and usage rules |
| Playback | [Playback.md](Playback.md) | `VideoPlayer` file playback, speed, fullscreen, subtitles, chapters |
| Capture | [Capture.md](Capture.md) | Camera access, recording, quality selection, snapshots |
| Streaming | [Streaming.md](Streaming.md) | HTTP progressive and HLS via NetworkKit, buffering events |
| Metadata | [Metadata.md](Metadata.md) | Duration, resolution, codec, framerate from native parsers |
| Thumbnails | [Thumbnails.md](Thumbnails.md) | Native frame extraction for Finder previews |
| Editing | [Editing.md](Editing.md) | Native lossless trim and concat |
| Mkv | [Mkv.md](Mkv.md) | Native `.mkv`/`.webm` parser, metadata and chapters fallback |
| Avi | [Avi.md](Avi.md) | Native `.avi` parser, metadata and chapters fallback |
| Mjpeg | [Mjpeg.md](Mjpeg.md) | Native baseline MJPEG decoder and fixture encoder |
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

- 2026-09-27: serde/serde_json replaced by Foundation (lang, FFI JSON, derives dropped).
- 2026-09-27: Native DRM outputs (`/dev/dri`, no swaymsg/xrandr), direct `/dev/video` scan.
- 2026-09-27: ffmpeg removed completely (native trim/concat, stills, frames, metadata).
- 2026-09-27: ffmpeg-free playback + capture (`decode_video_frame`, `frame_at`, native V4L2).
- 2026-09-27: Native pixel pipeline (`mjpeg`, `png_mini`, `extract_frame_native`, mov/avi samples).
- 2026-09-27: Audio track support (PCM `sowt` writer, multi-track native trim).
- 2026-09-27: Native AVI support (`avi`, metadata, chapters, FFI).
- 2026-09-27: WebM proven native (VP9/Opus fixture, same `mkv` path).
- 2026-09-27: Native Matroska/WebM support (`mkv`, metadata, chapters, FFI).
- 2026-09-27: Native ProRes decode TDC-1 subset (`prores_blocks`, `prores_frame`, 14 tests).
- 2026-09-27: Native lossless `.mov` trim (`trim_mov_native`), raw `.mov` writer (`build_raw_mov`).
- 2026-09-27: Native `.mov`/ProRes support (`mov`, `prores`, `transcode_prores`, FFI).
- 2026-09-26: Initial wiki with Playback, Capture, Streaming, Metadata, Thumbnails, Editing, Devices, Controls, Ffi and Localization pages.
