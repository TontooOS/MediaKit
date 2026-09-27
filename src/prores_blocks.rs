//! Native ProRes decode, Phase 1: bitstream, block codec, IDCT.
//!
//! TDC-1 subset (documented in `wiki/ProRes.md`): 8-bit 4:2:2/4:4:4,
//! 8x8 DCT-II blocks in zigzag order, uniform quant matrix, Rice
//! entropy coding with per-stream `k`. Slices and frames live in
//! `crate::prores_decode_frame` (Phase 2); the `icpf` container and
//! profiles stay in `crate::prores`.
//!
//! Pure Rust, no third-party video code. The block layout mirrors the
//! ProRes slice architecture so a WGPU compute port can reuse the
//! slice plan from `crate::prores::gpu_slice_plan`.

/// Standard JPEG zigzag order: index = raster position of the i-th
/// zigzag coefficient.
pub const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48,
    41, 34, 27, 20, 13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22,
    15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47,
    55, 62, 63,
];

/// Uniform base quant matrix (v1). Real ProRes uses perceptual
/// matrices per profile; the table is a parameter so they can drop
/// in without API churn.
pub const UNIFORM_QM: [u16; 64] = [16; 64];

/// MSB-first bit reader over a byte slice.
#[derive(Debug)]
pub struct BitReader<'a> {
    data: &'a [u8],
    byte: usize,
    /// Next bit index within the current byte (7 = MSB .. 0 = LSB).
    bit: i8,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte: 0,
            bit: 7,
        }
    }

    /// Bits consumed so far (for slice boundary checks).
    pub fn consumed_bits(&self) -> usize {
        self.byte * 8 + (7 - self.bit as usize)
    }

    pub fn read_bit(&mut self) -> Option<u32> {
        if self.byte >= self.data.len() {
            return None;
        }
        let v = (self.data[self.byte] >> self.bit) & 1;
        if self.bit == 0 {
            self.bit = 7;
            self.byte += 1;
        } else {
            self.bit -= 1;
        }
        Some(v as u32)
    }

    pub fn read_bits(&mut self, n: u32) -> Option<u32> {
        if n > 32 {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Some(v)
    }

    /// Unsigned Rice code with parameter `k`: `q` zeros, a one,
    /// then the `k`-bit remainder.
    pub fn read_rice(&mut self, k: u32) -> Option<u32> {
        if k > 24 {
            return None;
        }
        let mut q = 0u32;
        while self.read_bit()? == 0 {
            q += 1;
            if q > 1 << 20 {
                return None;
            }
        }
        let r = if k > 0 { self.read_bits(k)? } else { 0 };
        Some((q << k) | r)
    }

    /// Signed Rice code (zigzag-mapped unsigned).
    pub fn read_rice_signed(&mut self, k: u32) -> Option<i32> {
        let u = self.read_rice(k)?;
        Some(rice_unsigned_to_signed(u))
    }
}

/// MSB-first bit writer (fixture encoder + future native encoder).
#[derive(Debug, Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    /// Bits already stored in `cur` (0..8), MSB-aligned.
    filled: u8,
    cur: u8,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_bit(&mut self, b: u32) {
        self.cur = (self.cur << 1) | ((b & 1) as u8);
        self.filled += 1;
        if self.filled == 8 {
            self.buf.push(self.cur);
            self.cur = 0;
            self.filled = 0;
        }
    }

    pub fn write_bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bit((value >> i) & 1);
        }
    }

    pub fn write_rice(&mut self, value: u32, k: u32) {
        let q = value >> k;
        for _ in 0..q {
            self.write_bit(0);
        }
        self.write_bit(1);
        if k > 0 {
            self.write_bits(value & ((1 << k) - 1), k);
        }
    }

    pub fn write_rice_signed(&mut self, value: i32, k: u32) {
        self.write_rice(rice_signed_to_unsigned(value), k);
    }

    /// Pads with zero bits to the next byte and returns the bytes.
    pub fn finish(mut self) -> Vec<u8> {
        if self.filled > 0 {
            self.cur <<= 8 - self.filled;
            self.buf.push(self.cur);
        }
        self.buf
    }

    pub fn len_bits(&self) -> usize {
        self.buf.len() * 8 + self.filled as usize
    }
}

fn rice_signed_to_unsigned(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

fn rice_unsigned_to_signed(u: u32) -> i32 {
    ((u >> 1) as i32) ^ -((u & 1) as i32)
}

fn cos_table() -> [[f32; 8]; 8] {
    let mut t = [[0.0f32; 8]; 8];
    for x in 0..8 {
        for u in 0..8 {
            t[x][u] = (((2 * x + 1) as f32 * u as f32 * std::f32::consts::PI) / 16.0).cos();
        }
    }
    t
}

fn ortho(u: usize) -> f32 {
    if u == 0 {
        std::f32::consts::FRAC_1_SQRT_2
    } else {
        1.0
    }
}

/// Forward 8x8 DCT-II on level-shifted input. `f` is row-major.
pub fn dct_8x8(f: &[f32; 64]) -> [f32; 64] {
    let cos = cos_table();
    let mut out = [0.0f32; 64];
    for v in 0..8 {
        for u in 0..8 {
            let mut s = 0.0f32;
            for y in 0..8 {
                for x in 0..8 {
                    s += f[y * 8 + x] * cos[x][u] * cos[y][v];
                }
            }
            out[v * 8 + u] = 0.25 * ortho(u) * ortho(v) * s;
        }
    }
    out
}

/// Inverse 8x8 DCT-III on dequantized coefficients. `f` is row-major.
pub fn idct_8x8(f: &[f32; 64]) -> [f32; 64] {
    let cos = cos_table();
    let mut out = [0.0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0.0f32;
            for v in 0..8 {
                for u in 0..8 {
                    s += ortho(u) * ortho(v) * f[v * 8 + u] * cos[x][u] * cos[y][v];
                }
            }
            out[y * 8 + x] = 0.25 * s;
        }
    }
    out
}

/// Quantizes one 8x8 coefficient block (level-shifted pixels in,
/// zigzag-ordered quantized coefficients out).
pub fn quantize_block(coeffs: &[f32; 64], qm: &[u16; 64], qscale: f32) -> [i32; 64] {
    let mut out = [0i32; 64];
    for (i, &zz) in ZIGZAG.iter().enumerate() {
        let step = qm[i] as f32 * qscale;
        out[zz] = (coeffs[zz] / step).round() as i32;
    }
    out
}

/// Dequantizes a zigzag-ordered block back to raster-order floats.
pub fn dequantize_block(q: &[i32; 64], qm: &[u16; 64], qscale: f32) -> [f32; 64] {
    let mut out = [0.0f32; 64];
    for (i, &zz) in ZIGZAG.iter().enumerate() {
        out[zz] = q[zz] as f32 * qm[i] as f32 * qscale;
    }
    out
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    v.clamp(lo, hi)
}

/// Decodes one 8x8 block of signed coefficients from `r`.
///
/// Wire layout (TDC-1): DC as signed Rice `k_dc`, then the 63 AC
/// coefficients in zigzag order, each prefixed by a nonzero flag
/// bit; nonzero AC values are signed Rice `k_ac`.
pub fn decode_block(r: &mut BitReader, k_dc: u32, k_ac: u32) -> Option<[i32; 64]> {
    let mut q = [0i32; 64];
    q[0] = r.read_rice_signed(k_dc)?;
    for i in 1..64 {
        if r.read_bit()? == 0 {
            continue;
        }
        q[ZIGZAG[i]] = r.read_rice_signed(k_ac)?;
    }
    Some(q)
}

/// Encodes one 8x8 block (fixture encoder mirror of [`decode_block`]).
pub fn encode_block(w: &mut BitWriter, q: &[i32; 64], k_dc: u32, k_ac: u32) {
    w.write_rice_signed(q[0], k_dc);
    for i in 1..64 {
        let v = q[ZIGZAG[i]];
        if v == 0 {
            w.write_bit(0);
        } else {
            w.write_bit(1);
            w.write_rice_signed(v, k_ac);
        }
    }
}

/// Full pixel-domain roundtrip of one block: level shift, DCT,
/// quantize, dequantize, IDCT, unshift. Returns the reconstructed
/// bytes (used by tests and the fixture encoder).
pub fn roundtrip_block(pixels: &[u8; 64], qm: &[u16; 64], qscale: f32) -> [u8; 64] {
    let shifted: [f32; 64] = std::array::from_fn(|i| pixels[i] as f32 - 128.0);
    let coeffs = dct_8x8(&shifted);
    let q = quantize_block(&coeffs, qm, qscale);
    let deq = dequantize_block(&q, qm, qscale);
    let rec = idct_8x8(&deq);
    std::array::from_fn(|i| clamp_i32((rec[i] + 128.0).round() as i32, 0, 255) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rice_roundtrip() {
        let values = [0u32, 1, 5, 31, 32, 100, 1000, 65535];
        for k in [0, 1, 2, 5] {
            let mut w = BitWriter::new();
            for &v in &values {
                w.write_rice(v, k);
            }
            let bytes = w.finish();
            let mut r = BitReader::new(&bytes);
            for &v in &values {
                assert_eq!(r.read_rice(k).unwrap(), v, "k={k} v={v}");
            }
        }
    }

    #[test]
    fn rice_signed_roundtrip() {
        let values = [0i32, -1, 1, -127, 128, -1024, 1024];
        let mut w = BitWriter::new();
        for &v in &values {
            w.write_rice_signed(v, 3);
        }
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        for &v in &values {
            assert_eq!(r.read_rice_signed(3).unwrap(), v);
        }
    }

    #[test]
    fn bitstream_is_msb_first() {
        let mut w = BitWriter::new();
        w.write_bits(0b101, 3);
        let bytes = w.finish();
        assert_eq!(bytes, vec![0b1010_0000]);
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
    }

    #[test]
    fn dct_idct_roundtrip() {
        // Gradient block exercises all frequencies.
        let px: [f32; 64] = std::array::from_fn(|i| ((i * 7) % 256) as f32 - 128.0);
        let rec = idct_8x8(&dct_8x8(&px));
        for (a, b) in px.iter().zip(rec.iter()) {
            assert!((a - b).abs() < 0.01, "{a} vs {b}");
        }
    }

    #[test]
    fn flat_block_quantizes_to_dc_only() {
        let px = [200u8; 64];
        let shifted: [f32; 64] = std::array::from_fn(|_| 200.0 - 128.0);
        let q = quantize_block(&dct_8x8(&shifted), &UNIFORM_QM, 0.5);
        assert_eq!(q[0], 72); // 8*72 / (16*0.5)
        assert!(q[1..].iter().all(|&v| v == 0));
        let _ = px;
    }

    #[test]
    fn block_bitstream_roundtrip() {
        let q: [i32; 64] = std::array::from_fn(|i| {
            if i == 0 {
                -300
            } else if i == 5 || i == 40 {
                17
            } else {
                0
            }
        });
        let mut w = BitWriter::new();
        encode_block(&mut w, &q, 5, 2);
        let bits = w.len_bits();
        let bytes = w.finish();
        let mut r = BitReader::new(&bytes);
        assert_eq!(decode_block(&mut r, 5, 2).unwrap(), q);
        assert_eq!(r.consumed_bits(), bits);
    }

    #[test]
    fn pixel_roundtrip_near_lossless_at_fine_quant() {
        let px: [u8; 64] = std::array::from_fn(|i| ((i * 13 + 40) % 256) as u8);
        let rec = roundtrip_block(&px, &UNIFORM_QM, 0.25);
        for (a, b) in px.iter().zip(rec.iter()) {
            assert!((*a as i32 - *b as i32).abs() <= 3, "{a} vs {b}");
        }
    }

    #[test]
    fn truncated_stream_returns_none() {
        let mut r = BitReader::new(&[0x00]);
        assert!(r.read_rice(2).is_none());
        let mut r = BitReader::new(&[]);
        assert!(r.read_bit().is_none());
        assert!(decode_block(&mut r, 5, 2).is_none());
    }
}
