# Streaming

Network stream playback (HTTP progressive, HLS) via NetworkKit with
buffering status as events.

## Stream detection

```rust
let kind = mediakit::detect_kind("https://cdn/x/stream.m3u8");
assert_eq!(kind, mediakit::StreamKind::Hls);
```

- URLs ending in `.m3u8` are `StreamKind::Hls`, everything else is
  `StreamKind::Progressive`.

## Progressive

```rust
let (status, len) = mediakit::fetch_progressive("https://cdn/x/clip.mp4")?;
```

- Uses `networkkit::http::get` (single shared HTTP stack).
- Returns the HTTP status plus the optional `Content-Length`.
- Non-HTTP URLs return `Err(MediaError::NetworkError)`.

## HLS

```rust
let list = mediakit::fetch_hls_playlist("https://cdn/x/stream.m3u8")?;
for seg in &list.segments {
    let bytes = mediakit::streaming::fetch_segment(seg)?;
}
```

- Playlist fetch uses `networkkit::http::get`, then
  `HlsPlaylist::parse` (requires `#EXTM3U`, reads segments,
  `#EXT-X-TARGETDURATION` and `#EXT-X-ENDLIST`).
- Relative segment URIs resolve against the playlist URL.
- HTTP 4xx/5xx surface as `MediaError::NetworkError`.

### `HlsPlaylist`

| Field | Type | Description |
|---|---|---|
| `segments` | `Vec<String>` | Absolute segment URLs in play order |
| `target_duration_secs` | `f64` | `#EXT-X-TARGETDURATION` value |
| `live` | `bool` | False when `#EXT-X-ENDLIST` is present |

## Buffering events

```rust
let ev = mediakit::StreamEvent::new(mediakit::BufferState::Buffering, 0.5, "stall");
```

- `BufferState` is `Idle`, `Buffering`, `Ready`, `Stalled`, `Finished`
  or `Failed`.
- Player UIs map `Buffering`/`Stalled` to spinners.

## Usage / Example

```rust
use mediakit::{detect_kind, StreamKind};

match detect_kind("https://cdn/x/clip.mp4") {
    StreamKind::Hls => println!("playlist"),
    StreamKind::Progressive => println!("progressive"),
}
```

## Cross References

- [Playback.md](Playback.md) – where streams are rendered
- [Ffi.md](Ffi.md) – no streaming FFI yet (Rust-first API)
