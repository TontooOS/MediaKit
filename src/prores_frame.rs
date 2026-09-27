//! Native ProRes decode, Phase 2: slices, frames, YUV output.
//!
//! TDC-1 subset frame layout (see `wiki/ProRes.md`):
//! `frame_size u32 | "icpf" | 16-byte picture header (width, height,
//! chroma flags, slice count) | slices`. Each slice is
//! `slice_size u16 | quant u8 | Rice-coded 16x16 macroblocks` in
//! raster order, covering one horizontal band (see [`slice_bands`]).
//! Every slice decodes independently, which is what makes the later
//! WGPU compute port trivially parallel.
//!
//! Macroblock content per 16x16 luma region: 4 Y blocks, then chroma
//! blocks (`4:2:2`: Cb top/bottom, Cr top/bottom; `4:4:4`: 4+4 like
//! luma). Edge macroblocks are edge-padded by the encoder; the
//! decoder crops to `width` x `height`.
//!
//! Pure Rust, no third-party video code. Errors use the local
//! [`DecodeError`] so this module stays dependency-free; MediaKit
//! maps it to `MediaError` at the API boundary.

use crate::prores::ProResChroma;
use crate::prores_blocks::{
    decode_block, dequantize_block, dct_8x8, encode_block, idct_8x8, quantize_block,
    BitReader, BitWriter, UNIFORM_QM,
};

/// Rice parameters for the TDC-1 wire format.
pub const K_DC: u32 = 5;
pub const K_AC: u32 = 2;
/// Quant byte range accepted by the decoder.
pub const MAX_QUANT: u8 = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    BadHeader(String),
    BadDimensions,
    Unsupported(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "truncated prores frame"),
            DecodeError::BadHeader(e) => write!(f, "bad prores header: {e}"),
            DecodeError::BadDimensions => write!(f, "bad prores dimensions"),
            DecodeError::Unsupported(e) => write!(f, "unsupported prores feature: {e}"),
        }
    }
}

impl std::error::Error for DecodeError {}

pub type DecodeResult<T> = Result<T, DecodeError>;

fn err_truncated<T>() -> DecodeResult<T> {
    Err(DecodeError::Truncated)
}

/// Splits `height` rows into `n` contiguous, near-equal bands.
/// Returns `(y0, y1)` pairs covering `[0, height)` exactly.
pub fn slice_bands(height: u32, n: u16) -> Vec<(u32, u32)> {
    let n = n.max(1) as u32;
    if height == 0 {
        return Vec::new();
    }
    let base = height / n;
    let extra = (height % n) as usize;
    let mut out = Vec::with_capacity(n as usize);
    let mut y = 0u32;
    for i in 0..n as usize {
        let h = base + if i < extra { 1 } else { 0 };
        if h == 0 {
            break;
        }
        out.push((y, y + h));
        y += h;
    }
    out
}

/// Decoded picture in planar 8-bit YUV.
#[derive(Debug, Clone)]
pub struct YuvFrame {
    pub width: u32,
    pub height: u32,
    pub chroma: ProResChroma,
    pub y: Vec<u8>,
    pub cb: Vec<u8>,
    pub cr: Vec<u8>,
}

impl YuvFrame {
    pub fn chroma_width(&self) -> u32 {
        match self.chroma {
            ProResChroma::FourTwoTwo => self.width / 2,
            ProResChroma::FourFourFour => self.width,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct FrameHeader {
    width: u16,
    height: u16,
    chroma: ProResChroma,
    slices: u16,
}

fn parse_header(data: &[u8]) -> DecodeResult<(FrameHeader, usize)> {
    if data.len() < 24 {
        return err_truncated();
    }
    let size = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if size < 24 || size > data.len() + 1024 * 1024 * 1024 {
        return Err(DecodeError::BadHeader("frame size".into()));
    }
    if &data[4..8] != b"icpf" {
        return Err(DecodeError::BadHeader("missing icpf".into()));
    }
    let version = u16::from_be_bytes([data[10], data[11]]);
    if version > 1 {
        return Err(DecodeError::BadHeader("version".into()));
    }
    let width = u16::from_be_bytes([data[16], data[17]]);
    let height = u16::from_be_bytes([data[18], data[19]]);
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(DecodeError::BadDimensions);
    }
    let flags = data[20];
    if flags & 0x10 != 0 {
        return Err(DecodeError::Unsupported("alpha".into()));
    }
    let chroma = if flags & 0xC0 == 0xC0 {
        ProResChroma::FourFourFour
    } else {
        ProResChroma::FourTwoTwo
    };
    if chroma == ProResChroma::FourTwoTwo && width % 2 != 0 {
        return Err(DecodeError::BadDimensions);
    }
    let slices = u16::from_be_bytes([data[22], data[23]]);
    if slices == 0 || slices > 64 {
        return Err(DecodeError::BadHeader("slice count".into()));
    }
    Ok((
        FrameHeader {
            width,
            height,
            chroma,
            slices,
        },
        24,
    ))
}

fn qscale(quant: u8) -> DecodeResult<f32> {
    if quant == 0 || quant > MAX_QUANT {
        return Err(DecodeError::BadHeader("quant".into()));
    }
    Ok(quant as f32 / 8.0)
}

fn put_px(plane: &mut [u8], stride: u32, x: u32, y: u32, v: u8) {
    let idx = (y * stride + x) as usize;
    if idx < plane.len() {
        plane[idx] = v;
    }
}

/// Decodes one 8x8 block of coefficients into pixels and stores the
/// in-bounds region at `(ox, oy)` in `plane` (row stride `stride`).
fn decode_put_block(
    r: &mut BitReader,
    plane: &mut [u8],
    stride: u32,
    ox: u32,
    oy: u32,
    bound_w: u32,
    bound_h: u32,
    qs: f32,
) -> DecodeResult<()> {
    let q = decode_block(r, K_DC, K_AC).ok_or(DecodeError::Truncated)?;
    let rec = idct_8x8(&dequantize_block(&q, &UNIFORM_QM, qs));
    for dy in 0..8u32 {
        for dx in 0..8u32 {
            let x = ox + dx;
            let y = oy + dy;
            if x < bound_w && y < bound_h {
                let v = (rec[(dy * 8 + dx) as usize] + 128.0).round() as i32;
                put_px(plane, stride, x, y, v.clamp(0, 255) as u8);
            }
        }
    }
    Ok(())
}

/// Decodes one slice band `(y0, y1)` from `r` into `frame`.
fn decode_band(
    r: &mut BitReader,
    frame: &mut YuvFrame,
    y0: u32,
    y1: u32,
    qs: f32,
) -> DecodeResult<()> {
    let w = frame.width;
    let h = frame.height;
    let cw = frame.chroma_width();
    let mb_cols = w.div_ceil(16);
    let mb_rows = (y1 - y0).div_ceil(16);
    let chroma_blocks_444 = frame.chroma == ProResChroma::FourFourFour;
    let y1c = y1.min(h);
    for mb_y in 0..mb_rows {
        for mb_x in 0..mb_cols {
            let ox = mb_x * 16;
            let oy = y0 + mb_y * 16;
            // Luma: 4 blocks.
            for (by, bx) in [(0, 0), (0, 8), (8, 0), (8, 8)] {
                decode_put_block(r, &mut frame.y, w, ox + bx, oy + by, w, y1c, qs)?;
            }
            // Chroma.
            let cox = mb_x * if chroma_blocks_444 { 16 } else { 8 };
            let coy = y0 + mb_y * 16;
            let rows: &[(u32, u32)] = if chroma_blocks_444 {
                &[(0, 0), (0, 8), (8, 0), (8, 8)]
            } else {
                &[(0, 0), (8, 0)]
            };
            for plane in [&mut frame.cb, &mut frame.cr] {
                for &(by, bx) in rows {
                    decode_put_block(
                        r,
                        plane,
                        cw,
                        cox + bx,
                        coy + by,
                        cw,
                        y1c,
                        qs,
                    )?;
                }
            }
        }
    }
    Ok(())
}

/// Decodes a full TDC-1 frame into planar YUV.
pub fn decode_frame(data: &[u8]) -> DecodeResult<YuvFrame> {
    let (hdr, mut off) = parse_header(data)?;
    let bands = slice_bands(hdr.height as u32, hdr.slices);
    if bands.is_empty() {
        return Err(DecodeError::BadDimensions);
    }
    let w = hdr.width as u32;
    let h = hdr.height as u32;
    let cw = match hdr.chroma {
        ProResChroma::FourTwoTwo => w / 2,
        ProResChroma::FourFourFour => w,
    };
    let mut frame = YuvFrame {
        width: w,
        height: h,
        chroma: hdr.chroma,
        y: vec![0u8; (w * h) as usize],
        cb: vec![0u8; (cw * h) as usize],
        cr: vec![0u8; (cw * h) as usize],
    };
    // Split slices by their declared sizes first.
    let mut slices: Vec<(u8, &[u8])> = Vec::new();
    for _ in 0..hdr.slices {
        if off + 3 > data.len() {
            return err_truncated();
        }
        let size = u16::from_be_bytes([data[off], data[off + 1]]) as usize;
        let quant = data[off + 2];
        if size < 1 || off + 2 + size > data.len() {
            return err_truncated();
        }
        slices.push((quant, &data[off + 3..off + 2 + size]));
        off += 2 + size;
    }
    if slices.len() != bands.len() {
        return Err(DecodeError::BadHeader("slice/band mismatch".into()));
    }
    for ((quant, bytes), (y0, y1)) in slices.into_iter().zip(bands) {
        let qs = qscale(quant)?;
        let mut r = BitReader::new(bytes);
        decode_band(&mut r, &mut frame, y0, y1, qs)?;
    }
    Ok(frame)
}

/// BT.601 full-range YUV to RGB24.
pub fn yuv_to_rgb(frame: &YuvFrame) -> Vec<u8> {
    let mut out = vec![0u8; (frame.width * frame.height * 3) as usize];
    let cw = frame.chroma_width();
    for y in 0..frame.height {
        for x in 0..frame.width {
            let yi = (y * frame.width + x) as usize;
            let ci = match frame.chroma {
                ProResChroma::FourTwoTwo => (y * cw + x / 2) as usize,
                ProResChroma::FourFourFour => yi,
            };
            let yv = frame.y[yi] as f32;
            let cb = frame.cb[ci] as f32 - 128.0;
            let cr = frame.cr[ci] as f32 - 128.0;
            let o = yi * 3;
            out[o] = (yv + 1.402 * cr).round().clamp(0.0, 255.0) as u8;
            out[o + 1] = (yv - 0.344136 * cb - 0.714136 * cr)
                .round()
                .clamp(0.0, 255.0) as u8;
            out[o + 2] = (yv + 1.772 * cb).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

// ---------- Fixture encoder (bring-up + tests) ----------

fn get_px(plane: &[u8], stride: u32, w: u32, h: u32, x: u32, y: u32) -> u8 {
    if x < w && y < h {
        plane[(y * stride + x) as usize]
    } else {
        // Edge pad.
        plane[(y.min(h - 1) * stride + x.min(w - 1)) as usize]
    }
}

fn encode_put_block(
    w: &mut BitWriter,
    plane: &[u8],
    stride: u32,
    wdt: u32,
    hgt: u32,
    ox: u32,
    oy: u32,
    qs: f32,
) {
    let mut px = [0u8; 64];
    for dy in 0..8u32 {
        for dx in 0..8u32 {
            px[(dy * 8 + dx) as usize] = get_px(plane, stride, wdt, hgt, ox + dx, oy + dy);
        }
    }
    let shifted: [f32; 64] = std::array::from_fn(|i| px[i] as f32 - 128.0);
    let q = quantize_block(&dct_8x8(&shifted), &UNIFORM_QM, qs);
    encode_block(w, &q, K_DC, K_AC);
}

/// Encodes a planar YUV picture into one TDC-1 frame.
/// `quant` 1..=64 (8 is visually near-lossless on smooth content).
pub fn encode_frame_fixture(
    width: u32,
    height: u32,
    chroma: ProResChroma,
    y: &[u8],
    cb: &[u8],
    cr: &[u8],
    quant: u8,
    slices: u16,
) -> DecodeResult<Vec<u8>> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(DecodeError::BadDimensions);
    }
    if chroma == ProResChroma::FourTwoTwo && width % 2 != 0 {
        return Err(DecodeError::BadDimensions);
    }
    if !(1..=MAX_QUANT).contains(&quant) || slices == 0 || slices > 64 {
        return Err(DecodeError::BadHeader("params".into()));
    }
    let cw = match chroma {
        ProResChroma::FourTwoTwo => width / 2,
        ProResChroma::FourFourFour => width,
    };
    if y.len() != (width * height) as usize
        || cb.len() != (cw * height) as usize
        || cr.len() != (cw * height) as usize
    {
        return Err(DecodeError::BadDimensions);
    }
    let qs = qscale(quant)?;
    let bands = slice_bands(height, slices);
    let chroma_444 = chroma == ProResChroma::FourFourFour;
    let mut out = Vec::new();
    // Placeholder header (patched at the end).
    out.extend_from_slice(&[0u8; 24]);
    for (y0, y1) in &bands {
        let mut w = BitWriter::new();
        w.write_bits(quant as u32, 8);
        let mb_cols = width.div_ceil(16);
        let mb_rows = (y1 - y0).div_ceil(16);
        for mb_y in 0..mb_rows {
            for mb_x in 0..mb_cols {
                let ox = mb_x * 16;
                let oy = y0 + mb_y * 16;
                for (by, bx) in [(0, 0), (0, 8), (8, 0), (8, 8)] {
                    encode_put_block(&mut w, y, width, width, height, ox + bx, oy + by, qs);
                }
                let cox = mb_x * if chroma_444 { 16 } else { 8 };
                let rows: &[(u32, u32)] =
                    if chroma_444 { &[(0, 0), (0, 8), (8, 0), (8, 8)] } else { &[(0, 0), (8, 0)] };
                for plane in [cb, cr] {
                    for &(by, bx) in rows {
                        encode_put_block(
                            &mut w, plane, cw, cw, height, cox + bx, *y0 + mb_y * 16 + by, qs,
                        );
                    }
                }
            }
        }
        let body = w.finish();
        // Slice size counts the quant byte + block bytes.
        let slice_size = (body.len()) as u16;
        out.extend_from_slice(&slice_size.to_be_bytes());
        out.extend_from_slice(&body);
    }
    // Patch the icpf header.
    let total = out.len() as u32;
    out[0..4].copy_from_slice(&total.to_be_bytes());
    out[4..8].copy_from_slice(b"icpf");
    out[8..10].copy_from_slice(&20u16.to_be_bytes());
    out[10..12].copy_from_slice(&1u16.to_be_bytes());
    out[12..16].copy_from_slice(b"tont");
    out[16..18].copy_from_slice(&(width as u16).to_be_bytes());
    out[18..20].copy_from_slice(&(height as u16).to_be_bytes());
    out[20] = if chroma_444 { 0xC0 } else { 0x00 };
    out[21] = 0;
    out[22..24].copy_from_slice(&(bands.len() as u16).to_be_bytes());
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_exactly() {
        let bands = slice_bands(1080, 8);
        assert_eq!(bands.len(), 8);
        assert_eq!(bands[0].0, 0);
        assert_eq!(bands[7].1, 1080);
        for w in bands.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        assert_eq!(slice_bands(3, 8).len(), 3);
        assert!(slice_bands(0, 4).is_empty());
    }

    fn gradient_422(w: u32, h: u32) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let y: Vec<u8> = (0..w * h).map(|i| ((i * 7) % 256) as u8).collect();
        let n = (w / 2 * h) as usize;
        (y, vec![150u8; n], vec![100u8; n])
    }

    #[test]
    fn roundtrip_422_keeps_picture() {
        let (y, cb, cr) = gradient_422(32, 16);
        let enc =
            encode_frame_fixture(32, 16, ProResChroma::FourTwoTwo, &y, &cb, &cr, 8, 2).unwrap();
        let dec = decode_frame(&enc).unwrap();
        assert_eq!((dec.width, dec.height), (32, 16));
        let mut max = 0i32;
        for (a, b) in y.iter().zip(dec.y.iter()) {
            max = max.max((*a as i32 - *b as i32).abs());
        }
        assert!(max <= 16, "max luma drift {max}");
        assert_eq!(dec.cb.len(), cb.len());
    }

    #[test]
    fn roundtrip_444_and_edges() {
        let w = 20u32;
        let h = 12u32;
        let y: Vec<u8> = (0..w * h).map(|i| ((i * 3 + 30) % 256) as u8).collect();
        let c: Vec<u8> = (0..w * h).map(|i| ((i * 5 + 90) % 256) as u8).collect();
        let enc = encode_frame_fixture(w, h, ProResChroma::FourFourFour, &y, &c, &c, 8, 3)
            .unwrap();
        let dec = decode_frame(&enc).unwrap();
        assert_eq!(dec.chroma, ProResChroma::FourFourFour);
        let mut max = 0i32;
        for (a, b) in y.iter().zip(dec.y.iter()) {
            max = max.max((*a as i32 - *b as i32).abs());
        }
        assert!(max <= 20, "max drift {max}");
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_frame(b"short").is_err());
        let mut bad = encode_frame_fixture(
            16,
            16,
            ProResChroma::FourTwoTwo,
            &vec![0u8; 256],
            &vec![0u8; 128],
            &vec![0u8; 128],
            8,
            1,
        )
        .unwrap();
        bad[4..8].copy_from_slice(b"xxxx");
        assert!(decode_frame(&bad).is_err());
        assert!(decode_frame(&bad[..20]).is_err());
        assert!(encode_frame_fixture(15, 16, ProResChroma::FourTwoTwo, &y15(), &c15(), &c15(), 8, 1).is_err());
    }

    fn y15() -> Vec<u8> {
        vec![0u8; 15 * 16]
    }

    fn c15() -> Vec<u8> {
        vec![0u8; 15 * 16 / 2]
    }

    #[test]
    fn gray_stays_gray_through_rgb() {
        let n = 16 * 16;
        let frame = YuvFrame {
            width: 16,
            height: 16,
            chroma: ProResChroma::FourTwoTwo,
            y: vec![128u8; n],
            cb: vec![128u8; n / 2],
            cr: vec![128u8; n / 2],
        };
        let rgb = yuv_to_rgb(&frame);
        assert_eq!(&rgb[0..3], &[128, 128, 128]);
    }

    #[test]
    fn quant_zero_rejected() {
        assert!(qscale(0).is_err());
        assert!(qscale(65).is_err());
        assert!(qscale(8).is_ok());
    }
}
