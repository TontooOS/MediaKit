# Mjpeg

Native baseline MJPEG codec without ffmpeg: decoder, fixture
encoder and RGB output.

## `decode_jpeg`

```rust
pub fn decode_jpeg(data: &[u8]) -> Result<JpegImage, JpegError>
```

- 8-bit sequential DCT, 1 (gray) or 3 (YCbCr) components,
  sampling factors 1-2, 8-bit quant tables, interleaved or
  sequential scans.
- Returns `Err(JpegError::Unsupported)` for arithmetic coding,
  progressive scans or restart intervals; `Err(Truncated)` on
  short input, never garbage.

### `JpegImage`

| Field | Type | Description |
|---|---|---|
| `width` | `u32` | Picture width in pixels |
| `height` | `u32` | Picture height in pixels |
| `rgb` | `Vec<u8>` | Packed RGB24, BT.601 full-range |

## `encode_jpeg_fixture`

```rust
pub fn encode_jpeg_fixture(rgb: &[u8], width: u32, height: u32, sampling: JpegSampling, qstep: f32) -> Result<Vec<u8>, JpegError>
```

- Bring-up encoder with canonical Huffman tables; `sampling` is
  `Gray`, `Y444`, `Y422` or `Y420`, `qstep` scales the uniform
  quant table.
- Roundtrips against `decode_jpeg` (see tests).

## Usage / Example

```rust
use mediakit::{decode_jpeg, JpegSampling, encode_jpeg_fixture};

let enc = encode_jpeg_fixture(&vec![128u8; 16*16*3], 16, 16, JpegSampling::Y444, 4.0)?;
let img = decode_jpeg(&enc)?;
# Ok::<(), mediakit::JpegError>(())
```

## Cross References

- [Thumbnails.md](Thumbnails.md) – native MJPEG thumbnails
- [ProRes.md](ProRes.md) – shared IDCT design notes
