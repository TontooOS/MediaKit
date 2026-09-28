//! Baseline MJPEG codec, pure Rust.
//!
//! Decoder: 8-bit sequential DCT frames, 1 or 3 components, sampling
//! factors 1-2, 8-bit quant tables, single scans (interleaved or
//! not). No arithmetic coding, no progressive scans, no restart
//! intervals (clear errors). Reuses the exact IDCT from
//! `crate::prores_blocks`.
//!
//! The fixture encoder (`encode_jpeg_fixture`) mirrors the decoder
//! with canonical Huffman tables so roundtrips are self-checking.
//! Real-camera MJPEG (AVI `MJPG`, MOV `mjpa`) decodes through the
//! same path; exotic files surface `Unsupported`, never garbage.

use crate::prores_blocks::{dct_8x8, idct_8x8, ZIGZAG};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpegError {
    Truncated,
    BadMarker(String),
    Unsupported(String),
}

impl std::fmt::Display for JpegError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JpegError::Truncated => write!(f, "truncated jpeg"),
            JpegError::BadMarker(e) => write!(f, "bad jpeg marker: {e}"),
            JpegError::Unsupported(e) => write!(f, "unsupported jpeg feature: {e}"),
        }
    }
}

impl std::error::Error for JpegError {}

pub type JpegResult<T> = Result<T, JpegError>;

fn unsupported(what: &str) -> JpegError {
    JpegError::Unsupported(what.into())
}

#[derive(Debug, Clone)]
pub struct JpegImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Canonical Huffman decode table built from DHT counts.
#[derive(Debug, Clone, Default)]
struct HuffTable {
    /// symbols in canonical order, grouped by length 1..16.
    counts: [u8; 16],
    symbols: Vec<u8>,
    /// codes[li][k] = code of k-th symbol with length li+1.
    codes: Vec<Vec<u16>>,
}

impl HuffTable {
    fn build(counts: [u8; 16], symbols: Vec<u8>) -> JpegResult<Self> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total != symbols.len() || total == 0 || total > 256 {
            return Err(JpegError::BadMarker("dht counts/symbols".into()));
        }
        let mut codes: Vec<Vec<u16>> = vec![Vec::new(); 16];
        let mut code = 0u16;
        let mut idx = 0usize;
        for (li, &n) in counts.iter().enumerate() {
            for _ in 0..n {
                if idx >= symbols.len() {
                    return Err(JpegError::BadMarker("dht overrun".into()));
                }
                codes[li].push(code);
                code += 1;
                idx += 1;
            }
            code <<= 1;
            if code as u32 > (1 << 16) {
                return Err(JpegError::BadMarker("dht over-subscribed".into()));
            }
        }
        Ok(Self {
            counts,
            symbols,
            codes,
        })
    }

    /// Symbol offsets per length for canonical lookup.
    fn offsets(&self) -> [usize; 16] {
        let mut off = [0usize; 16];
        let mut acc = 0usize;
        for (i, &n) in self.counts.iter().enumerate() {
            off[i] = acc;
            acc += n as usize;
        }
        off
    }
}

struct BitPump<'a> {
    data: &'a [u8],
    pos: usize,
    acc: u32,
    nbits: u8,
}

impl<'a> BitPump<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            acc: 0,
            nbits: 0,
        }
    }

    fn fill(&mut self) -> bool {
        while self.nbits <= 24 && self.pos < self.data.len() {
            // Byte stuffing: 0xFF 0x00 means data 0xFF.
            if self.data[self.pos] == 0xFF {
                if self.pos + 1 >= self.data.len() {
                    return false;
                }
                let next = self.data[self.pos + 1];
                if next == 0x00 {
                    self.acc = (self.acc << 8) | 0xFF;
                    self.nbits += 8;
                    self.pos += 2;
                    continue;
                }
                // Real marker: stop before it.
                return self.nbits > 0;
            }
            self.acc = (self.acc << 8) | self.data[self.pos] as u32;
            self.nbits += 8;
            self.pos += 1;
        }
        true
    }

    fn get_bits(&mut self, n: u8) -> Option<u32> {
        if n > 16 || !self.fill() || self.nbits < n {
            return None;
        }
        self.nbits -= n;
        Some((self.acc >> self.nbits) & ((1 << n) - 1))
    }

    fn decode_symbol(&mut self, table: &HuffTable, offsets: &[usize; 16]) -> Option<u8> {
        let mut code = 0u32;
        for li in 0..16 {
            let bit = self.get_bits(1)?;
            code = (code << 1) | bit;
            for (k, &c) in table.codes[li].iter().enumerate() {
                if c as u32 == code {
                    return Some(table.symbols[offsets[li] + k]);
                }
            }
        }
        None
    }
}

fn receive(pump: &mut BitPump, s: u8) -> Option<i32> {
    if s == 0 {
        return Some(0);
    }
    if s > 11 {
        return None;
    }
    let v = pump.get_bits(s)? as i32;
    if v < (1 << (s - 1)) {
        Some(v - ((1 << s) - 1))
    } else {
        Some(v)
    }
}

#[derive(Debug, Clone, Copy)]
struct CompInfo {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
}

struct Frame {
    width: u32,
    height: u32,
    comps: Vec<CompInfo>,
    quant: Vec<[u16; 64]>,
    dc_tables: Vec<Option<HuffTable>>,
    ac_tables: Vec<Option<HuffTable>>,
}

impl Frame {
    fn max_hv(&self) -> (u8, u8) {
        let mh = self.comps.iter().map(|c| c.h).max().unwrap_or(1);
        let mv = self.comps.iter().map(|c| c.v).max().unwrap_or(1);
        (mh, mv)
    }
}

fn parse_dqt(frame: &mut Frame, body: &[u8]) -> JpegResult<()> {
    let mut off = 0usize;
    while off < body.len() {
        if off + 65 > body.len() {
            return Err(JpegError::BadMarker("dqt size".into()));
        }
        let info = body[off];
        let pq = info >> 4;
        let tq = (info & 15) as usize;
        if pq != 0 {
            return Err(unsupported("16-bit quant tables"));
        }
        if tq > 3 {
            return Err(JpegError::BadMarker("dqt id".into()));
        }
        while frame.quant.len() <= tq {
            frame.quant.push([16u16; 64]);
        }
        for i in 0..64 {
            frame.quant[tq][ZIGZAG[i]] = body[off + 1 + i] as u16;
        }
        off += 65;
    }
    Ok(())
}

fn parse_dht(frame: &mut Frame, body: &[u8]) -> JpegResult<()> {
    let mut off = 0usize;
    while off + 17 <= body.len() {
        let info = body[off];
        let tc = (info >> 4) as usize;
        let th = (info & 15) as usize;
        if tc > 1 || th > 3 {
            return Err(JpegError::BadMarker("dht id".into()));
        }
        let mut counts = [0u8; 16];
        counts.copy_from_slice(&body[off + 1..off + 17]);
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if off + 17 + total > body.len() {
            return Err(JpegError::BadMarker("dht symbols".into()));
        }
        let symbols = body[off + 17..off + 17 + total].to_vec();
        let table = HuffTable::build(counts, symbols)?;
        if tc == 0 {
            while frame.dc_tables.len() <= th {
                frame.dc_tables.push(None);
            }
            frame.dc_tables[th] = Some(table);
        } else {
            while frame.ac_tables.len() <= th {
                frame.ac_tables.push(None);
            }
            frame.ac_tables[th] = Some(table);
        }
        off += 17 + total;
    }
    Ok(())
}

fn parse_sof(frame: &mut Frame, body: &[u8]) -> JpegResult<()> {
    if body.len() < 6 {
        return Err(JpegError::BadMarker("sof size".into()));
    }
    if body[0] != 8 {
        return Err(unsupported("12-bit precision"));
    }
    let height = u16::from_be_bytes([body[1], body[2]]) as u32;
    let width = u16::from_be_bytes([body[3], body[4]]) as u32;
    let ncomp = body[5] as usize;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(JpegError::BadMarker("sof dims".into()));
    }
    if ncomp != 1 && ncomp != 3 {
        return Err(unsupported("component count"));
    }
    if body.len() < 6 + ncomp * 3 {
        return Err(JpegError::BadMarker("sof comps".into()));
    }
    let mut comps = Vec::new();
    for i in 0..ncomp {
        let h = body[6 + i * 3 + 1] >> 4;
        let v = body[6 + i * 3 + 1] & 15;
        if h == 0 || v == 0 || h > 2 || v > 2 {
            return Err(unsupported("sampling factors"));
        }
        comps.push(CompInfo {
            id: body[6 + i * 3],
            h,
            v,
            tq: body[6 + i * 3 + 2],
        });
    }
    frame.width = width;
    frame.height = height;
    frame.comps = comps;
    Ok(())
}

fn decode_block(
    pump: &mut BitPump,
    dc_table: &HuffTable,
    dc_off: &[usize; 16],
    ac_table: &HuffTable,
    ac_off: &[usize; 16],
    dc_pred: &mut i32,
    out: &mut [f32; 64],
) -> Option<()> {
    let s = pump.decode_symbol(dc_table, dc_off)? as u8;
    if s > 11 {
        return None;
    }
    let diff = receive(pump, s)?;
    *dc_pred = dc_pred.wrapping_add(diff);
    let mut q = [0i32; 64];
    q[0] = *dc_pred;
    let mut k = 1usize;
    while k < 64 {
        let rs = pump.decode_symbol(ac_table, ac_off)?;
        if rs == 0x00 {
            break;
        }
        if rs == 0xF0 {
            k += 16;
            continue;
        }
        let run = (rs >> 4) as usize;
        let size = (rs & 15) as u8;
        if size > 10 || k + run >= 64 {
            return None;
        }
        k += run;
        q[ZIGZAG[k]] = receive(pump, size)?;
        k += 1;
    }
    for i in 0..64 {
        out[i] = q[i] as f32;
    }
    Some(())
}

struct ScanComp {
    comp_idx: usize,
    td: usize,
    ta: usize,
}

/// Per-component MCU cursor state so blocks land in raster order.
struct Cursor {
    mcu_col: u32,
    mcu_row: u32,
    bx: u32,
    by: u32,
}

/// Decodes a full baseline JPEG into RGB.
pub fn decode_jpeg(data: &[u8]) -> JpegResult<JpegImage> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(JpegError::BadMarker("soi".into()));
    }
    let mut frame = Frame {
        width: 0,
        height: 0,
        comps: Vec::new(),
        quant: Vec::new(),
        dc_tables: Vec::new(),
        ac_tables: Vec::new(),
    };
    let mut off = 2usize;
    // Component planes (float, level-shifted later) + MCU cursors.
    let mut planes: Vec<Vec<f32>> = Vec::new();
    let mut comp_dims: Vec<(u32, u32)> = Vec::new();
    let mut cursors: Vec<Cursor> = Vec::new();
    let mut seen_sof = false;
    loop {
        if off + 2 > data.len() {
            return Err(JpegError::Truncated);
        }
        if data[off] != 0xFF {
            return Err(JpegError::BadMarker("marker prefix".into()));
        }
        let marker = data[off + 1];
        off += 2;
        match marker {
            0xD8 => continue, // fill / stray SOI
            0xD9 => break,    // EOI
            0x01 | 0xD0..=0xD9 => continue, // TEM / RSTn
            _ => {}
        }
        if marker == 0xDA {
            // SOS.
            if !seen_sof {
                return Err(JpegError::BadMarker("sos before sof".into()));
            }
            if off + 2 > data.len() {
                return Err(JpegError::Truncated);
            }
            let seg_len = u16::from_be_bytes([data[off], data[off + 1]]) as usize;
            if seg_len < 6 || off + seg_len > data.len() {
                return Err(JpegError::BadMarker("sos size".into()));
            }
            let body = &data[off + 2..off + seg_len];
            let ns = body[0] as usize;
            if ns == 0 || ns > frame.comps.len() || ns > 4 {
                return Err(JpegError::BadMarker("sos ns".into()));
            }
            if body.len() < 1 + ns * 2 + 3 {
                return Err(JpegError::BadMarker("sos comps".into()));
            }
            let mut scan = Vec::new();
            for i in 0..ns {
                let cs = body[1 + i * 2];
                let td = (body[1 + i * 2 + 1] >> 4) as usize;
                let ta = (body[1 + i * 2 + 1] & 15) as usize;
                let comp_idx = frame
                    .comps
                    .iter()
                    .position(|c| c.id == cs)
                    .ok_or_else(|| JpegError::BadMarker("sos comp ref".into()))?;
                scan.push(ScanComp { comp_idx, td, ta });
            }
            let ss = body[1 + ns * 2];
            let se = body[1 + ns * 2 + 1];
            let ah_al = body[1 + ns * 2 + 2];
            if ss != 0 || se != 63 || ah_al != 0 {
                return Err(unsupported("progressive scans"));
            }
            // Scan data runs to the next marker (handle stuffing in pump).
            let scan_start = off + seg_len;
            let mut scan_end = scan_start;
            while scan_end + 1 < data.len() {
                if data[scan_end] == 0xFF && data[scan_end + 1] != 0x00 {
                    break;
                }
                scan_end += 1;
            }
            let dc_offs: Vec<Option<[usize; 16]>> = frame
                .dc_tables
                .iter()
                .map(|t| t.as_ref().map(|x| {
                    let o = x.offsets();
                    o
                }))
                .collect();
            let ac_offs: Vec<Option<[usize; 16]>> = frame
                .ac_tables
                .iter()
                .map(|t| t.as_ref().map(|x| x.offsets()))
                .collect();
            let mut pump = BitPump::new(&data[scan_start..scan_end]);
            decode_scan_mcu(
                &frame, &mut planes, &comp_dims, &mut cursors, &mut pump, &scan, &dc_offs,
                &ac_offs,
            )?;
            off = scan_end;
            continue;
        }
        // Length-prefixed segment.
        if off + 2 > data.len() {
            return Err(JpegError::Truncated);
        }
        let seg_len = u16::from_be_bytes([data[off], data[off + 1]]) as usize;
        if seg_len < 2 || off + seg_len > data.len() {
            return Err(JpegError::BadMarker("segment size".into()));
        }
        let body = &data[off + 2..off + seg_len];
        match marker {
            0xC0 => {
                parse_sof(&mut frame, body)?;
                // Allocate planes + cursors.
                let (mh, mv) = frame.max_hv();
                planes.clear();
                comp_dims.clear();
                cursors.clear();
                for c in &frame.comps {
                    // True picture-relative dims (not MCU-padded):
                    // the encoder clamps edge reads, the decoder
                    // drops out-of-bounds writes.
                    let fx = mh / c.h;
                    let fy = mv / c.v;
                    let pw = frame.width.div_ceil(fx as u32);
                    let ph = frame.height.div_ceil(fy as u32);
                    planes.push(vec![0.0f32; (pw * ph) as usize]);
                    comp_dims.push((pw, ph));
                    cursors.push(Cursor {
                        mcu_col: 0,
                        mcu_row: 0,
                        bx: 0,
                        by: 0,
                    });
                }
                seen_sof = true;
            }
            0xDB => parse_dqt(&mut frame, body)?,
            0xC4 => parse_dht(&mut frame, body)?,
            0xDD => return Err(unsupported("restart intervals")),
            0xE0..=0xEF | 0xFE => {} // APPn / COM: skip
            0xC1 | 0xC2 | 0xC3 | 0xC9..=0xCB => {
                return Err(unsupported("non-baseline sof"));
            }
            _ => {}
        }
        off += seg_len;
    }
    if !seen_sof || planes.is_empty() {
        return Err(JpegError::BadMarker("no image".into()));
    }
    Ok(assemble_rgb(&frame, &planes, &comp_dims))
}

/// MCU-major scan decode with per-component cursors.
#[allow(clippy::too_many_arguments)]
fn decode_scan_mcu(
    frame: &Frame,
    planes: &mut [Vec<f32>],
    comp_dims: &[(u32, u32)],
    cursors: &mut [Cursor],
    pump: &mut BitPump,
    scan: &[ScanComp],
    dc_offs: &[Option<[usize; 16]>],
    ac_offs: &[Option<[usize; 16]>],
) -> JpegResult<()> {
    let (mh, mv) = frame.max_hv();
    let mcu_cols = frame.width.div_ceil(8 * mh as u32);
    let mcu_rows = frame.height.div_ceil(8 * mv as u32);
    let mut blocks = 0u32;
    for s in scan {
        let c = &frame.comps[s.comp_idx];
        blocks += c.h as u32 * c.v as u32;
    }
    if blocks == 0 || blocks > 10 {
        return Err(unsupported("mcu block count"));
    }
    // Reset cursors + DC predictors for this scan.
    for cur in cursors.iter_mut() {
        *cur = Cursor {
            mcu_col: 0,
            mcu_row: 0,
            bx: 0,
            by: 0,
        };
    }
    let mut dc_preds = vec![0i32; frame.comps.len()];
    for _ in 0..mcu_rows {
        for _ in 0..mcu_cols {
            for s in scan {
                let c = &frame.comps[s.comp_idx];
                let dc_t = frame.dc_tables.get(s.td).and_then(|t| t.as_ref()).ok_or_else(|| {
                    JpegError::BadMarker("dc table ref".into())
                })?;
                let ac_t = frame.ac_tables.get(s.ta).and_then(|t| t.as_ref()).ok_or_else(|| {
                    JpegError::BadMarker("ac table ref".into())
                })?;
                let dc_off = dc_offs[s.td].ok_or_else(|| JpegError::BadMarker("dc".into()))?;
                let ac_off = ac_offs[s.ta].ok_or_else(|| JpegError::BadMarker("ac".into()))?;
                let qt = frame.quant.get(c.tq as usize).ok_or_else(|| {
                    JpegError::BadMarker("quant ref".into())
                })?;
                for _ in 0..(c.v as u32 * c.h as u32) {
                    let mut coeff = [0f32; 64];
                    decode_block(
                        pump,
                        dc_t,
                        &dc_off,
                        ac_t,
                        &ac_off,
                        &mut dc_preds[s.comp_idx],
                        &mut coeff,
                    )
                    .ok_or(JpegError::Truncated)?;
                    for i in 0..64 {
                        coeff[i] *= qt[i] as f32;
                    }
                    let px = idct_8x8(&coeff);
                    let cur = &mut cursors[s.comp_idx];
                    let (pw, ph) = comp_dims[s.comp_idx];
                    let ox = (cur.mcu_col * c.h as u32 + cur.bx) * 8;
                    let oy = (cur.mcu_row * c.v as u32 + cur.by) * 8;
                    for dy in 0..8u32 {
                        for dx in 0..8u32 {
                            let x = ox + dx;
                            let y = oy + dy;
                            if x < pw && y < ph {
                                planes[s.comp_idx][(y * pw + x) as usize] =
                                    px[(dy * 8 + dx) as usize] + 128.0;
                            }
                        }
                    }
                    cur.bx += 1;
                    if cur.bx >= c.h as u32 {
                        cur.bx = 0;
                        cur.by += 1;
                        if cur.by >= c.v as u32 {
                            cur.by = 0;
                        }
                    }
                }
            }
            // Advance MCU cursors for comps in this scan.
            for s in scan {
                let cur = &mut cursors[s.comp_idx];
                cur.mcu_col += 1;
                if cur.mcu_col >= mcu_cols {
                    cur.mcu_col = 0;
                    cur.mcu_row += 1;
                }
                cur.bx = 0;
                cur.by = 0;
            }
        }
    }
    Ok(())
}

fn sample_plane(plane: &[f32], pw: u32, ph: u32, x: u32, y: u32) -> f32 {
    plane[(y.min(ph - 1) * pw + x.min(pw - 1)) as usize]
}

fn assemble_rgb(frame: &Frame, planes: &[Vec<f32>], comp_dims: &[(u32, u32)]) -> JpegImage {
    let w = frame.width;
    let h = frame.height;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    let get = |ci: usize, x: u32, y: u32| -> f32 {
        let (pw, ph) = comp_dims[ci];
        if frame.comps[ci].h == 1 && frame.comps[ci].v == 1 && pw == w && ph == h {
            planes[ci][(y * pw + x) as usize]
        } else {
            // Nearest-neighbor chroma upsampling.
            let sx = x * pw / w;
            let sy = y * ph / h;
            sample_plane(&planes[ci], pw, ph, sx, sy)
        }
    };
    for y in 0..h {
        for x in 0..w {
            let (yy, cb, cr) = if frame.comps.len() == 1 {
                let g = get(0, x, y);
                (g, 128.0, 128.0)
            } else {
                (get(0, x, y), get(1, x, y), get(2, x, y))
            };
            let o = ((y * w + x) * 3) as usize;
            rgb[o] = (yy + 1.402 * (cr - 128.0)).round().clamp(0.0, 255.0) as u8;
            rgb[o + 1] = (yy - 0.344136 * (cb - 128.0) - 0.714136 * (cr - 128.0))
                .round()
                .clamp(0.0, 255.0) as u8;
            rgb[o + 2] = (yy + 1.772 * (cb - 128.0)).round().clamp(0.0, 255.0) as u8;
        }
    }
    JpegImage { width: w, height: h, rgb }
}

// ---------- Fixture encoder ----------

struct JpegWriter {
    buf: Vec<u8>,
    acc: u32,
    nbits: u8,
}

impl JpegWriter {
    fn new() -> Self {
        Self {
            buf: Vec::new(),
            acc: 0,
            nbits: 0,
        }
    }

    fn bits(&mut self, value: u32, n: u8) {
        self.acc = (self.acc << n) | (value & ((1 << n) - 1));
        self.nbits += n;
        while self.nbits >= 8 {
            self.nbits -= 8;
            let b = (self.acc >> self.nbits) as u8;
            self.buf.push(b);
            if b == 0xFF {
                self.buf.push(0x00);
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.nbits > 0 {
            let b = (self.acc << (8 - self.nbits)) as u8 | ((1 << (8 - self.nbits)) - 1);
            self.buf.push(b);
            if b == 0xFF {
                self.buf.push(0x00);
            }
        }
        self.buf
    }
}

/// Canonical codes from per-symbol lengths `[(symbol, length)]`.
fn canonical(lengths: &[(u16, u8)]) -> Vec<(u16, u8, u16)> {
    // (symbol, length, code)
    let mut by_len: Vec<Vec<u16>> = vec![Vec::new(); 17];
    for &(sym, len) in lengths {
        by_len[len as usize].push(sym);
    }
    for bucket in by_len.iter_mut() {
        bucket.sort_unstable();
    }
    let mut out = Vec::new();
    let mut code = 0u16;
    for len in 1..=16 {
        for &sym in &by_len[len] {
            out.push((sym, len as u8, code));
            code += 1;
        }
        code <<= 1;
    }
    out
}

fn dht_segment(tc_th: u8, lengths: &[(u16, u8)]) -> Vec<u8> {
    let mut counts = [0u8; 16];
    for &(_, len) in lengths {
        counts[len as usize - 1] += 1;
    }
    let table = canonical(lengths);
    let mut syms: Vec<Vec<u8>> = vec![Vec::new(); 16];
    for (sym, len, _) in &table {
        syms[*len as usize - 1].push(*sym as u8);
    }
    let mut seg = vec![tc_th];
    seg.extend_from_slice(&counts);
    for bucket in &syms {
        seg.extend_from_slice(bucket);
    }
    seg
}

fn find_code(table: &[(u16, u8, u16)], sym: u16) -> (u16, u8) {
    table
        .iter()
        .find(|(s, _, _)| *s == sym)
        .map(|(_, len, code)| (*code, *len))
        .expect("fixture symbol missing")
}

/// Fixture sampling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JpegSampling {
    Gray,
    Y444,
    Y422,
    Y420,
}

/// Encodes RGB into baseline JPEG (fixture/bring-up quality).
/// `qstep` scales the uniform quant table (8 is near-lossless on
/// smooth content).
pub fn encode_jpeg_fixture(
    rgb: &[u8],
    width: u32,
    height: u32,
    sampling: JpegSampling,
    qstep: f32,
) -> JpegResult<Vec<u8>> {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return Err(JpegError::BadMarker("dims".into()));
    }
    if rgb.len() != (width * height * 3) as usize {
        return Err(JpegError::BadMarker("rgb size".into()));
    }
    if qstep <= 0.0 || qstep > 64.0 {
        return Err(JpegError::BadMarker("qstep".into()));
    }
    // RGB -> YCbCr full-range.
    let n = (width * height) as usize;
    let mut y = vec![0f32; n];
    let mut cb = vec![0f32; n];
    let mut cr = vec![0f32; n];
    for i in 0..n {
        let r = rgb[i * 3] as f32;
        let g = rgb[i * 3 + 1] as f32;
        let b = rgb[i * 3 + 2] as f32;
        y[i] = 0.299 * r + 0.587 * g + 0.114 * b;
        cb[i] = 128.0 - 0.168736 * r - 0.331264 * g + 0.5 * b;
        cr[i] = 128.0 + 0.5 * r - 0.418688 * g - 0.081312 * b;
    }
    let (comps, plane_data): (Vec<(u8, u8, u8)>, Vec<Vec<f32>>) = match sampling {
        JpegSampling::Gray => (vec![(1, 1, 1)], vec![y]),
        JpegSampling::Y444 => (
            vec![(1, 1, 1), (2, 1, 1), (3, 1, 1)],
            vec![y, cb, cr],
        ),
        JpegSampling::Y422 => (
            vec![(1, 2, 1), (2, 1, 1), (3, 1, 1)],
            vec![y, downsample(&cb, width, height, 2, 1), downsample(&cr, width, height, 2, 1)],
        ),
        JpegSampling::Y420 => (
            vec![(1, 2, 2), (2, 1, 1), (3, 1, 1)],
            vec![y, downsample(&cb, width, height, 2, 2), downsample(&cr, width, height, 2, 2)],
        ),
    };
    // Fixed canonical tables: DC cats 0..11, AC run/size + EOB/ZRL.
    let dc_lengths: Vec<(u16, u8)> = [2u8, 3, 3, 3, 3, 4, 5, 5, 5, 5, 5, 5]
        .into_iter()
        .enumerate()
        .map(|(cat, len)| (cat as u16, len))
        .collect();
    let mut ac_lengths: Vec<(u16, u8)> = vec![(0x00, 2), (0xF0, 8)];
    for run in 0..16u16 {
        for size in 1..=10u16 {
            ac_lengths.push(((run << 4) | size, 8));
        }
    }
    let dc_table = canonical(&dc_lengths);
    let ac_table = canonical(&ac_lengths);
    // Effective quant table (baked into DQT, like real encoders).
    let qm: [u16; 64] = std::array::from_fn(|_| {
        (8.0 * qstep).round().clamp(1.0, 255.0) as u16
    });
    let mut out = vec![0xFF, 0xD8];
    // APP0 (JFIF stub, skipped by decoder).
    let mut app0 = vec![0xFF, 0xE0, 0x00, 0x10];
    app0.extend_from_slice(b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00");
    out.extend_from_slice(&app0);
    // DQT.
    let mut dqt = vec![0u8];
    for i in 0..64 {
        dqt.push(qm[ZIGZAG[i]] as u8);
    }
    out.extend_from_slice(&marker_seg(0xDB, &dqt));
    // SOF0.
    let mut sof = vec![8];
    sof.extend_from_slice(&(height as u16).to_be_bytes());
    sof.extend_from_slice(&(width as u16).to_be_bytes());
    sof.push(comps.len() as u8);
    for (id, h, v) in &comps {
        sof.push(*id);
        sof.push(h << 4 | v);
        sof.push(0);
    }
    out.extend_from_slice(&marker_seg(0xC0, &sof));
    // DHT DC + AC.
    out.extend_from_slice(&marker_seg(0xC4, &dht_segment(0x00, &dc_lengths)));
    out.extend_from_slice(&marker_seg(0xC4, &dht_segment(0x10, &ac_lengths)));
    // SOS single interleaved scan.
    let mut sos = vec![comps.len() as u8];
    for (id, _, _) in &comps {
        sos.push(*id);
        sos.push(0x00);
    }
    sos.extend_from_slice(&[0, 63, 0]);
    out.extend_from_slice(&marker_seg(0xDA, &sos));
    // MCU data.
    let mh = comps.iter().map(|c| c.1).max().unwrap();
    let mv = comps.iter().map(|c| c.2).max().unwrap();
    // Packed plane views: luma is full-res, chroma subsampled by
    // (max_h / h, max_v / v).
    let views: Vec<(u32, u32, &[f32])> = comps
        .iter()
        .enumerate()
        .map(|(ci, (_, h, v))| {
            let fx = mh as u32 / *h as u32;
            let fy = mv as u32 / *v as u32;
            (
                width.div_ceil(fx),
                height.div_ceil(fy),
                &plane_data[ci][..],
            )
        })
        .collect();
    let mcu_cols = width.div_ceil(8 * mh as u32);
    let mcu_rows = height.div_ceil(8 * mv as u32);
    let mut w = JpegWriter::new();
    let mut dc_preds = vec![0i32; comps.len()];
    for mcu_row in 0..mcu_rows {
        for mcu_col in 0..mcu_cols {
            for (ci, (_, h, v)) in comps.iter().enumerate() {
                let (pw, ph, plane) = views[ci];
                for by in 0..*v as u32 {
                    for bx in 0..*h as u32 {
                        let ox = (mcu_col * *h as u32 + bx) * 8;
                        let oy = (mcu_row * *v as u32 + by) * 8;
                        let mut px = [0f32; 64];
                        for dy in 0..8u32 {
                            for dx in 0..8u32 {
                                // Edge-replicate past the picture.
                                let x = (ox + dx).min(pw - 1) as usize;
                                let y = (oy + dy).min(ph - 1) as usize;
                                px[(dy * 8 + dx) as usize] =
                                    plane[y * pw as usize + x] - 128.0;
                            }
                        }
                        let coeff = dct_8x8(&px);
                        let mut q = [0i32; 64];
                        for (i, &zz) in ZIGZAG.iter().enumerate() {
                            q[zz] = (coeff[zz] / qm[i] as f32).round() as i32;
                        }
                        // DC.
                        let diff = q[0] - dc_preds[ci];
                        dc_preds[ci] = q[0];
                        let (cat, mag) = dc_category(diff);
                        let (code, len) = find_code(&dc_table, cat as u16);
                        w.bits(code as u32, len);
                        if cat > 0 {
                            w.bits(mag, cat);
                        }
                        // AC.
                        let mut zero_run = 0u32;
                        for i in 1..64 {
                            let v = q[ZIGZAG[i]];
                            if v == 0 {
                                zero_run += 1;
                                continue;
                            }
                            while zero_run >= 16 {
                                let (code, len) = find_code(&ac_table, 0xF0);
                                w.bits(code as u32, len);
                                zero_run -= 16;
                            }
                            let (size, mag) = ac_mag(v);
                            let sym = (zero_run << 4) | size as u32;
                            let (code, len) = find_code(&ac_table, sym as u16);
                            w.bits(code as u32, len);
                            w.bits(mag, size);
                            zero_run = 0;
                        }
                        let (code, len) = find_code(&ac_table, 0x00);
                        w.bits(code as u32, len);
                    }
                }
            }
        }
    }
    out.extend_from_slice(&w.finish());
    out.extend_from_slice(&[0xFF, 0xD9]);
    Ok(out)
}

fn marker_seg(marker: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![0xFF, marker];
    out.extend_from_slice(&((body.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(body);
    out
}

fn dc_category(diff: i32) -> (u8, u32) {
    if diff == 0 {
        return (0, 0);
    }
    let mag = diff.abs() as u32;
    let cat = 32 - mag.leading_zeros() as u8;
    let bits = if diff < 0 {
        (diff - 1 + (1 << cat)) as u32 & ((1 << cat) - 1)
    } else {
        mag
    };
    (cat, bits)
}

fn ac_mag(v: i32) -> (u8, u32) {
    let mag = v.abs() as u32;
    let size = 32 - mag.leading_zeros() as u8;
    let bits = if v < 0 {
        (v - 1 + (1 << size)) as u32 & ((1 << size) - 1)
    } else {
        mag
    };
    (size, bits)
}

fn downsample(plane: &[f32], w: u32, h: u32, fx: u32, fy: u32) -> Vec<f32> {
    let nw = w.div_ceil(fx);
    let nh = h.div_ceil(fy);
    let mut out = vec![0f32; (nw * nh) as usize];
    for y in 0..nh {
        for x in 0..nw {
            let mut s = 0f32;
            let mut n = 0u32;
            for dy in 0..fy {
                for dx in 0..fx {
                    let sx = x * fx + dx;
                    let sy = y * fy + dy;
                    if sx < w && sy < h {
                        s += plane[(sy * w + sx) as usize];
                        n += 1;
                    }
                }
            }
            out[(y * nw + x) as usize] = s / n.max(1) as f32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_rgb(w: u32, h: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                out.push(((x * 255 / w.max(1)) % 256) as u8);
                out.push(((y * 255 / h.max(1)) % 256) as u8);
                out.push((((x + y) * 255 / (w + h).max(1)) % 256) as u8);
            }
        }
        out
    }

    fn max_drift(a: &[u8], b: &[u8]) -> i32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (*x as i32 - *y as i32).abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn roundtrip_gray() {
        let rgb: Vec<u8> = (0..(16 * 16)).flat_map(|i| [i as u8, i as u8, i as u8]).collect();
        let enc = encode_jpeg_fixture(&rgb, 16, 16, JpegSampling::Gray, 4.0).unwrap();
        let dec = decode_jpeg(&enc).unwrap();
        assert_eq!((dec.width, dec.height), (16, 16));
        assert!(max_drift(&rgb, &dec.rgb) <= 12);
    }

    #[test]
    fn roundtrip_444() {
        let rgb = gradient_rgb(24, 16);
        let enc = encode_jpeg_fixture(&rgb, 24, 16, JpegSampling::Y444, 4.0).unwrap();
        let dec = decode_jpeg(&enc).unwrap();
        assert!(max_drift(&rgb, &dec.rgb) <= 16);
    }

    #[test]
    fn roundtrip_420() {
        let rgb = gradient_rgb(32, 24);
        let enc = encode_jpeg_fixture(&rgb, 32, 24, JpegSampling::Y420, 8.0).unwrap();
        let dec = decode_jpeg(&enc).unwrap();
        assert!(max_drift(&rgb, &dec.rgb) <= 32);
    }

    #[test]
    fn roundtrip_422_odd_dims() {
        let rgb = gradient_rgb(20, 12);
        let enc = encode_jpeg_fixture(&rgb, 20, 12, JpegSampling::Y422, 8.0).unwrap();
        let dec = decode_jpeg(&enc).unwrap();
        assert_eq!((dec.width, dec.height), (20, 12));
        assert!(max_drift(&rgb, &dec.rgb) <= 32);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_jpeg(b"not a jpeg").is_err());
        assert!(decode_jpeg(&[0xFF, 0xD8, 0xFF, 0xD9]).is_err());
        let rgb = gradient_rgb(8, 8);
        let mut enc = encode_jpeg_fixture(&rgb, 8, 8, JpegSampling::Gray, 4.0).unwrap();
        enc[0] = 0x00;
        assert!(decode_jpeg(&enc).is_err());
        assert!(encode_jpeg_fixture(&rgb, 0, 8, JpegSampling::Gray, 4.0).is_err());
    }
}
