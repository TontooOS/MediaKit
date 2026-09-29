# HwAccel

System-GPU H.264 decode via VA-API (Linux). The module loads
`libva.so.2` plus `libva-drm.so.2` at runtime, opens a DRM render
node, and decodes through the OS fixed-function video engine instead
of an external decoder binary or a hand-written codec. No new
dependencies: `dlopen`/`dlsym` and every VA-API symbol are declared
locally, following `capture_v4l2` and `devices`.

Phase 1 (capability query) and Phase 2 for I-slices are implemented.
P/B slices return `UnsupportedFormat` until DPB management lands.
Everything degrades gracefully: no hardware means an empty list or an
error, never a panic.

## Capability Query

```rust
let decoders = mediakit::hw_decoder_info();
for d in &decoders {
    println!("{} {} {}", d.api, d.codec, d.profile);
}
```

- Opens `/dev/dri/renderD128` and up, runs `vaInitialize`, then
  `vaQueryConfigProfiles` plus `vaQueryConfigEntrypoints`.
- Reports one entry per H.264 profile with a `VLD` entrypoint.
- Returns an empty vector without hardware, without libva, or when any
  VA call fails. Never panics, never errors.

### `HwDecoder`

```rust
pub struct HwDecoder {
    pub api: String,
    pub codec: String,
    pub profile: String,
    pub profile_id: i32,
    pub entrypoint: String,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}
```

| Field | Type | Description |
|---|---|---|
| `api` | `String` | Always `"va-api"` |
| `codec` | `String` | Always `"h264"` (HEVC/VP9 later) |
| `profile` | `String` | e.g. `"VAProfileH264High"` |
| `profile_id` | `i32` | Numeric VA profile, e.g. `7` |
| `entrypoint` | `String` | Always `"VAEntrypointVLD"` |
| `max_width` | `Option<u32>` | Always `None` (not portably queryable) |
| `max_height` | `Option<u32>` | Always `None` (not portably queryable) |

### `hw_decoder_info`

```rust
pub fn hw_decoder_info() -> Vec<HwDecoder>
```

- Probes the default render nodes. Empty when unsupported.
- Linux only; the module is not wired in on other targets.

### `hw_decoder_info_from_nodes`

```rust
pub fn hw_decoder_info_from_nodes(nodes: &[&str]) -> Vec<HwDecoder>
```

- Probes exactly `nodes`, e.g. `&["/dev/dri/renderD128"]`.
- Used by tests with nonexistent paths (expect empty) and by machines
  with unusual device paths.

## Decode Session

```rust
let mut session = mediakit::VaapiSession::open(1280, 720)?;
let frame = session.decode_frame_vaapi(&annexb)?;
```

- `VaapiSession::open(width, height)` picks the first render node with
  H.264 VLD, preferring High, then Main, then Constrained Baseline.
  It creates a VA config, 4 YUV420 surfaces, and a progressive
  context. All resources release in `Drop`, in reverse order.
- `VaapiSession::open_on_nodes(nodes, width, height)` tries exactly
  `nodes` in order and reports the last error when all fail.
- Missing libraries or nodes yield `UnsupportedFormat`, never a panic.

### `decode_frame_vaapi`

```rust
pub fn decode_frame_vaapi(&mut self, annexb: &[u8]) -> Result<HwFrame>
```

- Expects Annex-B with SPS plus PPS plus a single I-slice (IDR or
  non-IDR). Submits the real `vaBeginPicture` / `vaRenderPicture`
  (picture params, slice params, bitstream) / `vaEndPicture` /
  `vaSyncSurface` / `vaDeriveImage` plus `vaMapBuffer` path and returns
  planar YUV.
- Returns `UnsupportedFormat("va-api slice decode: ...")` for P/B
  slices (DPB management is the remaining milestone), multi-slice
  pictures, and resolution changes (reopen the session instead).
- Returns `ParseError` for unparseable SPS/PPS/slice headers.

### `decode_annexb_to_yuv`

```rust
pub fn decode_annexb_to_yuv(data: &[u8]) -> Result<HwFrame>
```

- One-shot helper: sizes the session from the SPS, decodes, closes.
- Same graceful behavior as `decode_frame_vaapi`.

### `annexb_to_rgb`

```rust
pub fn annexb_to_rgb(data: &[u8]) -> Result<(u32, u32, Vec<u8>)>
```

- GPU decode plus software NV12/I420 to RGB24 (BT.601 full-range, same
  as the crate converters). Returns `(width, height, rgb)`.

### `HwFrame`

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Coded width |
| `height` | `u32` | Coded height |
| `y` | `Vec<u8>` | Luma plane (`width * height`) |
| `u` | `Vec<u8>` | Chroma U plane (`width/2 * height/2`) |
| `v` | `Vec<u8>` | Chroma V plane (`width/2 * height/2`) |

## Pure Helpers

Fully unit-tested without hardware; also used by the session.

### `split_annexb_nals`

```rust
pub fn split_annexb_nals(data: &[u8]) -> Vec<NalUnit>
```

- Scans `0x000001` / `0x00000001` start codes. Skips leading garbage,
  empty units, and trailing zeros. Payload emulation-prevention bytes
  pass through untouched. Input without a start code yields empty.

### `NalUnit`

| Field | Type | Description |
|---|---|---|
| `offset` | `usize` | Offset of the NAL header byte |
| `len` | `usize` | Length from the header to the next start code |
| `nal_type` | `u8` | H.264 NAL unit type (`header & 0x1F`) |
| `nal_ref_idc` | `u8` | H.264 `nal_ref_idc` (`(header >> 5) & 0x03`) |

### `parse_sps` / `parse_sps_dimensions`

```rust
pub fn parse_sps(nal: &[u8]) -> Option<SpsInfo>
pub fn parse_sps_dimensions(nal: &[u8]) -> Option<(u32, u32)>
```

- Parse an SPS NAL (header byte included) via Exp-Golomb, including
  cropping and high-profile chroma fields. `None` for non-SPS or
  truncated input.

### `parse_pps`

```rust
pub fn parse_pps(nal: &[u8]) -> Option<PpsInfo>
```

- Parses the PPS subset the submit path needs (ids, entropy coding
  mode, QP init, deblocking flag). `None` for non-PPS input.

### `parse_slice_header`

```rust
pub fn parse_slice_header(nal: &[u8], sps: &SpsInfo, pps: &PpsInfo) -> Option<SliceInfo>
```

- Parses I-slice headers end to end (frame number, POC, QP delta,
  deblocking offsets, `header_bits` for `slice_data_bit_offset`).
- Returns `None` for P/B/SP/SI slices, field pictures, and truncated
  input.

### Converters

```rust
pub fn nv12_to_rgb(y_plane: &[u8], uv_plane: &[u8], width: u32, height: u32) -> Result<Vec<u8>>
pub fn i420_to_rgb(y_plane: &[u8], u_plane: &[u8], v_plane: &[u8], width: u32, height: u32) -> Result<Vec<u8>>
```

- NV12 and planar I420/YV12 to RGB24, BT.601 full-range.
- `Err(ParseError)` on odd/zero/huge dimensions or short buffers.

## Future Wiring

Once DPB management for P/B slices lands, `decode_video_frame` in
`thumbnails.rs` should prefer this path for H.264 content and keep the
native software decoders as fallback. Existing decode behavior is
unchanged until then.

## Usage / Example

```rust
// Phase 1: list GPU decoders (empty without hardware, never panics).
let decoders = mediakit::hw_decoder_info();

// Phase 2: decode one Annex-B I-frame picture to RGB.
match mediakit::annexb_to_rgb(&annexb_bytes) {
    Ok((w, h, rgb)) => println!("gpu frame {w}x{h}, {} bytes", rgb.len()),
    Err(e) => println!("software fallback: {e}"),
}
```

## Cross References

- [Thumbnails.md](Thumbnails.md) – `decode_video_frame` will prefer
  this path once inter-frame DPB management lands
- [Devices.md](Devices.md) – same graceful style for DRM enumeration
- [Capture.md](Capture.md) – other Linux-only hardware path (V4L2)
