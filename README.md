# MediaKit

Video framework for TontooOS: file playback (MP4, WebM, MKV, AVI), camera capture, network streaming (HTTP progressive + HLS), metadata, thumbnails and basic editing via ffmpeg.

## Made for TontooOS

Explore more at https://github.com/TontooOS/Libs

Wiki: [wiki/MAIN.md](wiki/MAIN.md)

## Adding to Your Project

Add to your `Cargo.toml`:

```toml
[dependencies]
mediakit = { path = "/Library/System/mediakit" }
# or via SDK:
# sdk = { path = "/Library/System/sdk", features = ["MediaKit"] }
```

Then at the crate root:

```rust
use mediakit::{MediaKit, VideoPlayer};
use std::path::Path;

let mut kit = MediaKit::new();
kit.player_mut().open(Path::new("clip.mp4")).unwrap();
kit.player_mut().play().unwrap();
```

## Stack

- Playback/m Metadata/Thumbs/Edit: `ffmpeg` / `ffprobe` CLI
- Streaming HTTP: `NetworkKit` (`networkkit::http`)
- Volume/Mute: `AudioKit` (`audiokit::SystemVolume`, no duplication)
- Control icons: `CoreIcon` SF Symbols (`resolve_icon_path`)
- Control text: `CoreText` SF Pro (`SF_PRO_FAMILY`, `/usr/share/fonts/OTF/`)
- Theme: Dark `#1b2022` / `#d8d9d9`, Light `#ffffff` / `#272727`

## License

TCL v26.1
