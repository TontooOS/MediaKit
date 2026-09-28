//! Native QuickTime / ISO-BMFF (`.mov`, `.mp4`, `.m4v`) box parser.
//!
//! Pure Rust, no ffmpeg/ffprobe dependency. Streaming-friendly: top-level
//! boxes are walked with `seek`, only `moov` is buffered fully so large
//! `mdat` payloads are never loaded into memory.
//!
//! Coverage is the metadata subset MediaKit needs 1:1 for `.mov`:
//! duration, resolution, video/audio fourcc and framerate estimate.
//! Full sample decode (ProRes coefficients) lives in `crate::prores`
//! with the GPU slice plan; pixel output still uses the external
//! ffmpeg path until the WGPU decoder lands.

use crate::error::{MediaError, Result};
use crate::metadata::VideoMetadata;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions handled by the native MOV parser.
pub const MOV_EXTENSIONS: &[&str] = &["mov", "mp4", "m4v"];

/// Known top-level brands. Anything else is still accepted when the
/// `ftyp` box itself is well-formed.
pub const MOV_BRANDS: &[&str] = &[
    "qt  ", "mov ", "mp41", "mp42", "isom", "iso2", "iso4", "mmp4", "m4v ", "avc1",
];

const MAX_TOP_BOXES: usize = 256;
const MAX_BOX_SIZE: u64 = 8 * 1024 * 1024 * 1024;
const MAX_MOOV_SIZE: usize = 256 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct MovInfo {
    pub major_brand: String,
    pub compatible: Vec<String>,
    pub duration_secs: f64,
    pub timescale: u32,
    pub width: u32,
    pub height: u32,
    pub video_fourcc: String,
    pub audio_fourcc: Option<String>,
    pub framerate: f64,
    pub has_moov: bool,
}

impl MovInfo {
    pub fn container_name(&self) -> String {
        if self.major_brand.trim().is_empty() {
            "mov".to_string()
        } else {
            self.major_brand.trim().to_string()
        }
    }
}

/// True when the extension is handled natively.
pub fn is_mov_extension(ext: &str) -> bool {
    MOV_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

/// Byte-level sniff: `size + b"ftyp"` at offset 0 with a sane size.
pub fn is_mov_bytes(buf: &[u8]) -> bool {
    if buf.len() < 12 {
        return false;
    }
    if &buf[4..8] != b"ftyp" {
        return false;
    }
    let size = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    size >= 12 && (size <= buf.len() || size == 1)
}

/// Sniffs the first 32 bytes of `path` for an `ftyp` box.
/// Returns false (not an error) for missing/unreadable files.
pub fn sniff_mov(path: &Path) -> bool {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut head = [0u8; 32];
    match f.read_exact(&mut head) {
        Ok(()) => is_mov_bytes(&head),
        Err(_) => false,
    }
}

fn fourcc(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect()
}

fn read_box_header(f: &mut File) -> Result<Option<(u64, [u8; 4])>> {
    let mut hdr = [0u8; 8];
    match f.read_exact(&mut hdr) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(MediaError::from_io(e)),
    }
    let mut size = u32::from_be_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64;
    let typ = [hdr[4], hdr[5], hdr[6], hdr[7]];
    let mut header_len = 8u64;
    if size == 1 {
        let mut ext = [0u8; 8];
        f.read_exact(&mut ext).map_err(MediaError::from_io)?;
        size = u64::from_be_bytes(ext);
        header_len = 16;
    } else if size == 0 {
        let end = f.seek(SeekFrom::End(0)).map_err(MediaError::from_io)?;
        let pos = f.seek(SeekFrom::Current(0)).map_err(MediaError::from_io)?;
        size = end.saturating_sub(pos).saturating_add(header_len);
    }
    if size < header_len || size > MAX_BOX_SIZE {
        return Err(MediaError::ParseError("invalid box size".into()));
    }
    Ok(Some((size, typ)))
}

fn skip_box(f: &mut File, size: u64, header_len: u64) -> Result<()> {
    let rest = size.saturating_sub(header_len);
    f.seek(SeekFrom::Current(rest as i64))
        .map_err(MediaError::from_io)?;
    Ok(())
}

fn read_box_payload(f: &mut File, size: u64, header_len: u64, cap: usize) -> Result<Vec<u8>> {
    let rest = size.saturating_sub(header_len) as usize;
    if rest > cap {
        return Err(MediaError::ParseError("box too large".into()));
    }
    let mut buf = vec![0u8; rest];
    f.read_exact(&mut buf).map_err(MediaError::from_io)?;
    Ok(buf)
}

fn parse_ftyp(info: &mut MovInfo, payload: &[u8]) -> Result<()> {
    if payload.len() < 8 {
        return Err(MediaError::ParseError("truncated ftyp".into()));
    }
    info.major_brand = fourcc(&payload[0..4]);
    for chunk in payload[8..].chunks(4) {
        if chunk.len() == 4 {
            info.compatible.push(fourcc(chunk));
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
struct TrackCtx {
    handler: String,
    timescale: u32,
    width: u32,
    height: u32,
    video_fourcc: Option<String>,
    audio_fourcc: Option<String>,
    framerate: f64,
}

fn be_u16(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
}

fn be_u32(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn be_u64(b: &[u8], off: usize) -> Option<u64> {
    b.get(off..off + 8).map(|s| {
        u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
    })
}

fn walk_boxes(data: &[u8], mut f: impl FnMut(&[u8; 4], &[u8]) -> Result<()>) -> Result<()> {
    let mut off = 0usize;
    let mut count = 0usize;
    while off + 8 <= data.len() && count < MAX_TOP_BOXES {
        count += 1;
        let mut size = be_u32(data, off).unwrap_or(0) as usize;
        let typ = [data[off + 4], data[off + 5], data[off + 6], data[off + 7]];
        let mut hdr = 8usize;
        if size == 1 {
            size = be_u64(data, off + 8).unwrap_or(0) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - off;
        }
        if size < hdr || off + size > data.len() {
            return Err(MediaError::ParseError("invalid child box".into()));
        }
        f(&typ, &data[off + hdr..off + size])?;
        off += size;
    }
    Ok(())
}

fn parse_mvhd(info: &mut MovInfo, payload: &[u8]) -> Result<()> {
    if payload.len() < 20 {
        return Err(MediaError::ParseError("truncated mvhd".into()));
    }
    let version = payload[0];
    let (timescale, duration) = if version == 1 {
        if payload.len() < 32 {
            return Err(MediaError::ParseError("truncated mvhd v1".into()));
        }
        (
            be_u32(payload, 20).unwrap_or(0),
            be_u64(payload, 24).unwrap_or(0) as f64,
        )
    } else {
        (
            be_u32(payload, 12).unwrap_or(0),
            be_u32(payload, 16).unwrap_or(0) as f64,
        )
    };
    if timescale == 0 {
        return Ok(());
    }
    info.timescale = timescale;
    info.duration_secs = duration / timescale as f64;
    Ok(())
}

fn parse_tkhd_dimensions(payload: &[u8]) -> (u32, u32) {
    if payload.len() < 8 {
        return (0, 0);
    }
    let version = payload[0];
    // Tail after the fixed matrix: width/height as 16.16.
    // v0 content is 84 bytes, v1 content is 96 bytes.
    let tail = if version == 1 {
        if payload.len() < 96 {
            return (0, 0);
        }
        &payload[payload.len() - 8..]
    } else {
        if payload.len() < 84 {
            return (0, 0);
        }
        &payload[payload.len() - 8..]
    };
    let w = u32::from_be_bytes([tail[0], tail[1], tail[2], tail[3]]) >> 16;
    let h = u32::from_be_bytes([tail[4], tail[5], tail[6], tail[7]]) >> 16;
    (w, h)
}

fn parse_mdhd(payload: &[u8]) -> (u32, f64) {
    if payload.len() < 16 {
        return (0, 0.0);
    }
    let version = payload[0];
    if version == 1 {
        if payload.len() < 32 {
            return (0, 0.0);
        }
        let ts = be_u32(payload, 20).unwrap_or(0);
        let dur = be_u64(payload, 24).unwrap_or(0) as f64;
        (ts, if ts == 0 { 0.0 } else { dur / ts as f64 })
    } else {
        let ts = be_u32(payload, 12).unwrap_or(0);
        let dur = be_u32(payload, 16).unwrap_or(0) as f64;
        (ts, if ts == 0 { 0.0 } else { dur / ts as f64 })
    }
}

fn parse_hdlr(payload: &[u8]) -> String {
    if payload.len() < 12 {
        return String::new();
    }
    fourcc(&payload[8..12]).trim().to_string()
}

fn parse_stsd(track: &mut TrackCtx, payload: &[u8]) -> Result<()> {
    if payload.len() < 8 {
        return Ok(());
    }
    let body = &payload[8..];
    let mut off = 0usize;
    while off + 8 <= body.len() {
        let size = be_u32(body, off).unwrap_or(0) as usize;
        if size < 8 || off + size > body.len() {
            break;
        }
        let entry_fourcc = fourcc(&body[off + 4..off + 8]);
        let entry = &body[off..off + size];
        if track.handler == "vide" && track.video_fourcc.is_none() {
            track.video_fourcc = Some(entry_fourcc.clone());
            // VisualSampleEntry: width/height u16 BE at +32/+34
            // (8 entry header + 6 reserved + 2 dataref + 16 predefined).
            if entry.len() >= 36 {
                if let (Some(w), Some(h)) = (be_u16(entry, 32), be_u16(entry, 34)) {
                    if w > 0 && h > 0 {
                        track.width = w as u32;
                        track.height = h as u32;
                    }
                }
            }
        } else if track.handler == "soun" && track.audio_fourcc.is_none() {
            track.audio_fourcc = Some(entry_fourcc.clone());
        }
        if size == 0 {
            break;
        }
        off += size;
        if track.video_fourcc.is_some() && track.audio_fourcc.is_some() {
            break;
        }
    }
    Ok(())
}

fn parse_stts(track: &mut TrackCtx, payload: &[u8]) -> Result<()> {
    if payload.len() < 8 {
        return Ok(());
    }
    let count = be_u32(payload, 4).unwrap_or(0) as usize;
    let mut off = 8usize;
    let mut total_samples = 0u64;
    let mut total_delta = 0u64;
    for _ in 0..count.min(256) {
        if off + 8 > payload.len() {
            break;
        }
        let n = be_u32(payload, off).unwrap_or(0) as u64;
        let delta = be_u32(payload, off + 4).unwrap_or(0) as u64;
        off += 8;
        if delta == 0 || n == 0 {
            continue;
        }
        total_samples += n;
        total_delta += n * delta;
    }
    if track.handler == "vide" && total_samples > 0 && total_delta > 0 && track.timescale > 0 {
        track.framerate = track.timescale as f64 / (total_delta as f64 / total_samples as f64);
    }
    Ok(())
}

fn parse_stbl(track: &mut TrackCtx, payload: &[u8]) -> Result<()> {
    walk_boxes(payload, |typ, body| {
        match typ {
            b"stsd" => parse_stsd(track, body)?,
            b"stts" => parse_stts(track, body)?,
            _ => {}
        }
        Ok(())
    })
}

fn parse_minf(track: &mut TrackCtx, payload: &[u8]) -> Result<()> {
    walk_boxes(payload, |typ, body| {
        if typ == b"stbl" {
            parse_stbl(track, body)?;
        }
        Ok(())
    })
}

fn parse_mdia(track: &mut TrackCtx, payload: &[u8]) -> Result<()> {
    let mut duration = 0.0f64;
    walk_boxes(payload, |typ, body| {
        match typ {
            b"mdhd" => {
                let (ts, dur) = parse_mdhd(body);
                if ts != 0 {
                    track.timescale = ts;
                }
                duration = dur;
            }
            b"hdlr" => track.handler = parse_hdlr(body),
            b"minf" => parse_minf(track, body)?,
            _ => {}
        }
        let _ = duration;
        Ok(())
    })?;
    Ok(())
}

fn parse_trak(info: &mut MovInfo, payload: &[u8]) -> Result<()> {
    let mut track = TrackCtx::default();
    let mut tkhd_w = 0u32;
    let mut tkhd_h = 0u32;
    walk_boxes(payload, |typ, body| {
        match typ {
            b"tkhd" => {
                let (w, h) = parse_tkhd_dimensions(body);
                tkhd_w = w;
                tkhd_h = h;
            }
            b"mdia" => parse_mdia(&mut track, body)?,
            _ => {}
        }
        Ok(())
    })?;
    if track.handler == "vide" {
        if track.width == 0 {
            track.width = tkhd_w;
        }
        if track.height == 0 {
            track.height = tkhd_h;
        }
        if info.video_fourcc.is_empty() {
            info.video_fourcc = track.video_fourcc.unwrap_or_default();
            info.width = track.width;
            info.height = track.height;
            info.framerate = track.framerate;
        }
    } else if track.handler == "soun" && info.audio_fourcc.is_none() {
        info.audio_fourcc = track.audio_fourcc;
    }
    Ok(())
}

fn parse_moov(info: &mut MovInfo, payload: &[u8]) -> Result<()> {
    info.has_moov = true;
    walk_boxes(payload, |typ, body| {
        match typ {
            b"mvhd" => parse_mvhd(info, body)?,
            b"trak" => parse_trak(info, body)?,
            _ => {}
        }
        Ok(())
    })
}

/// Reads the top-level structure of `path` without loading `mdat`.
pub fn read_mov_info(path: &Path) -> Result<MovInfo> {
    let mut f = File::open(path).map_err(MediaError::from_io)?;
    let mut info = MovInfo::default();
    let mut boxes = 0usize;
    loop {
        if boxes >= MAX_TOP_BOXES {
            break;
        }
        boxes += 1;
        let pos = f.seek(SeekFrom::Current(0)).map_err(MediaError::from_io)?;
        let header = read_box_header(&mut f)?;
        let Some((size, typ)) = header else { break };
        let header_len = if size >= 16 && looks_like_large_size(&mut f, pos)? {
            16
        } else {
            8
        };
        match &typ {
            b"ftyp" => {
                let payload = read_box_payload(&mut f, size, header_len, 4096)?;
                parse_ftyp(&mut info, &payload)?;
            }
            b"moov" => {
                let payload = read_box_payload(&mut f, size, header_len, MAX_MOOV_SIZE)?;
                parse_moov(&mut info, &payload)?;
            }
            _ => skip_box(&mut f, size, header_len)?,
        }
        if size == 0 {
            break;
        }
    }
    if !info.has_moov {
        return Err(MediaError::ParseError("missing moov box".into()));
    }
    Ok(info)
}

fn looks_like_large_size(f: &mut File, _pos: u64) -> Result<bool> {
    let cur = f.seek(SeekFrom::Current(0)).map_err(MediaError::from_io)?;
    // read_box_header already consumed 8 bytes; peek whether the real
    // header was 16 bytes by checking alignment is impossible without
    // re-reading, so conservatively report false: 8-byte headers are
    // the common case and payload readers tolerate trailing bytes.
    f.seek(SeekFrom::Start(cur)).map_err(MediaError::from_io)?;
    Ok(false)
}

/// Converts native `MovInfo` into the shared `VideoMetadata` shape
/// so `.mov` behaves 1:1 with the ffprobe path.
pub fn mov_to_metadata(info: &MovInfo, path: &Path) -> VideoMetadata {
    let size_bytes = std::fs::metadata(path).ok().map(|m| m.len());
    VideoMetadata {
        duration_secs: info.duration_secs,
        width: info.width,
        height: info.height,
        video_codec: if info.video_fourcc.is_empty() {
            info.container_name()
        } else {
            info.video_fourcc.clone()
        },
        audio_codec: info.audio_fourcc.clone(),
        framerate: info.framerate,
        container: info.container_name(),
        size_bytes,
    }
}

/// Native metadata for `.mov`/`.mp4`/`.m4v` without ffprobe.
pub fn read_mov_metadata(path: &Path) -> Result<VideoMetadata> {
    let info = read_mov_info(path)?;
    Ok(mov_to_metadata(&info, path))
}

// ---------- Native sample tables, raw writer and lossless trim ----------

/// Hard cap for whole-file reads in the native trim path.
pub const MAX_MOV_FILE_SIZE: usize = 512 * 1024 * 1024;
/// Sanity cap for expanded sample counts.
const MAX_SAMPLES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default)]
struct Span {
    start: usize,
    end: usize,
    header: usize,
}

impl Span {
    fn content_range(&self) -> (usize, usize) {
        (self.start + self.header, self.end)
    }
}

fn parse_error(msg: &str) -> MediaError {
    MediaError::ParseError(msg.into())
}

fn be32(data: &[u8], off: usize) -> Result<u32> {
    data.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| parse_error("truncated box"))
}

fn be64(data: &[u8], off: usize) -> Result<u64> {
    data.get(off..off + 8)
        .map(|s| {
            u64::from_be_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]])
        })
        .ok_or_else(|| parse_error("truncated box"))
}

fn scan_children(data: &[u8]) -> Result<Vec<([u8; 4], Span)>> {
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 8 <= data.len() {
        if out.len() > 4096 {
            return Err(parse_error("too many boxes"));
        }
        let s32 = be32(data, off)? as u64;
        let typ = [data[off + 4], data[off + 5], data[off + 6], data[off + 7]];
        let (size, header) = if s32 == 1 {
            (be64(data, off + 8)?, 16usize)
        } else if s32 == 0 {
            ((data.len() - off) as u64, 8usize)
        } else {
            (s32, 8usize)
        };
        if size < header as u64 || off + size as usize > data.len() {
            return Err(parse_error("invalid child box"));
        }
        out.push((
            typ,
            Span {
                start: off,
                end: off + size as usize,
                header,
            },
        ));
        off += size as usize;
    }
    Ok(out)
}

fn find_child(kids: &[([u8; 4], Span)], want: &[u8; 4]) -> Option<Span> {
    kids.iter().find(|(t, _)| t == want).map(|(_, s)| *s)
}

fn make_box(typ: &[u8; 4], payload: &[u8]) -> Result<Vec<u8>> {
    let size = payload.len() as u64 + 8;
    if size > u32::MAX as u64 {
        return Err(parse_error("box too large"));
    }
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&(size as u32).to_be_bytes());
    out.extend_from_slice(typ);
    out.extend_from_slice(payload);
    Ok(out)
}

/// Rebuilds a box payload, replacing listed children and copying the
/// rest verbatim (sizes re-serialized bottom-up by the caller).
fn splice(payload: &[u8], replacements: &[([u8; 4], Vec<u8>)]) -> Result<Vec<u8>> {
    let kids = scan_children(payload)?;
    let mut out = Vec::new();
    for (typ, span) in kids {
        match replacements.iter().find(|(t, _)| t == &typ) {
            Some((_, replacement)) => out.extend_from_slice(replacement),
            None => out.extend_from_slice(&payload[span.start..span.end]),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Default)]
struct SttsRun {
    count: u32,
    delta: u32,
}

#[derive(Debug, Clone, Default)]
struct ParsedTrak {
    handler: String,
    tkhd: Span,
    tkhd_version: u8,
    mdhd: Span,
    mdhd_version: u8,
    media_timescale: u32,
    stbl: Span,
    stsd_box: Vec<u8>,
    stts: Vec<SttsRun>,
    stsc: Vec<(u32, u32, u32)>,
    stsz_uniform: u32,
    stsz_table: Vec<u32>,
    chunk_offsets_64: bool,
    chunk_offsets: Vec<u64>,
    stss: Option<Vec<u32>>,
    ctts_version: Option<u8>,
    ctts: Vec<(u32, i64)>,
    video_fourcc: String,
    stbl_kids: Vec<([u8; 4], Span)>,
}

fn parse_stts_body(body: &[u8]) -> Result<Vec<SttsRun>> {
    if body.len() < 8 {
        return Err(parse_error("truncated stts"));
    }
    let count = be32(body, 4)? as usize;
    if count > 65536 {
        return Err(parse_error("stts run count insane"));
    }
    let mut runs = Vec::with_capacity(count.min(1024));
    let mut off = 8usize;
    for _ in 0..count {
        if off + 8 > body.len() {
            return Err(parse_error("truncated stts run"));
        }
        runs.push(SttsRun {
            count: be32(body, off)?,
            delta: be32(body, off + 4)?,
        });
        off += 8;
    }
    Ok(runs)
}

fn parse_stsc_body(body: &[u8]) -> Result<Vec<(u32, u32, u32)>> {
    if body.len() < 8 {
        return Err(parse_error("truncated stsc"));
    }
    let count = be32(body, 4)? as usize;
    if count > 65536 {
        return Err(parse_error("stsc entry count insane"));
    }
    let mut out = Vec::with_capacity(count.min(1024));
    let mut off = 8usize;
    for _ in 0..count {
        if off + 12 > body.len() {
            return Err(parse_error("truncated stsc entry"));
        }
        out.push((be32(body, off)?, be32(body, off + 4)?, be32(body, off + 8)?));
        off += 12;
    }
    if out.is_empty() || out[0].0 != 1 {
        return Err(parse_error("stsc must start at chunk 1"));
    }
    Ok(out)
}

fn parse_stsz_body(body: &[u8]) -> Result<(u32, Vec<u32>)> {
    if body.len() < 12 {
        return Err(parse_error("truncated stsz"));
    }
    let uniform = be32(body, 4)?;
    let count = be32(body, 8)? as usize;
    if count > MAX_SAMPLES {
        return Err(parse_error("sample count insane"));
    }
    if uniform != 0 {
        return Ok((uniform, Vec::new()));
    }
    if body.len() < 12 + count * 4 {
        return Err(parse_error("truncated stsz table"));
    }
    let mut table = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        table.push(be32(body, 12 + i * 4)?);
    }
    Ok((0, table))
}

fn parse_offset_table(body: &[u8], wide: bool) -> Result<Vec<u64>> {
    if body.len() < 8 {
        return Err(parse_error("truncated chunk offset table"));
    }
    let count = be32(body, 4)? as usize;
    if count == 0 || count > MAX_SAMPLES {
        return Err(parse_error("chunk count insane"));
    }
    let stride = if wide { 8 } else { 4 };
    if body.len() < 8 + count * stride {
        return Err(parse_error("truncated chunk offsets"));
    }
    let mut out = Vec::with_capacity(count.min(1 << 20));
    for i in 0..count {
        out.push(if wide {
            be64(body, 8 + i * 8)?
        } else {
            be32(body, 8 + i * 4)? as u64
        });
    }
    Ok(out)
}

fn parse_stss_body(body: &[u8]) -> Result<Vec<u32>> {
    if body.len() < 8 {
        return Err(parse_error("truncated stss"));
    }
    let count = be32(body, 4)? as usize;
    if count > MAX_SAMPLES {
        return Err(parse_error("sync sample count insane"));
    }
    if body.len() < 8 + count * 4 {
        return Err(parse_error("truncated stss table"));
    }
    (0..count)
        .map(|i| be32(body, 8 + i * 4))
        .collect()
}

fn parse_ctts_body(body: &[u8]) -> Result<(u8, Vec<(u32, i64)>)> {
    if body.len() < 8 {
        return Err(parse_error("truncated ctts"));
    }
    let version = body[0];
    if version > 1 {
        return Err(parse_error("unsupported ctts version"));
    }
    let count = be32(body, 4)? as usize;
    if count > 65536 {
        return Err(parse_error("ctts run count insane"));
    }
    let mut runs = Vec::with_capacity(count.min(1024));
    let mut off = 8usize;
    for _ in 0..count {
        if off + 8 > body.len() {
            return Err(parse_error("truncated ctts run"));
        }
        let n = be32(body, off)?;
        let offset = if version == 1 {
            i32::from_be_bytes([body[off + 4], body[off + 5], body[off + 6], body[off + 7]])
                as i64
        } else {
            be32(body, off + 4)? as i64
        };
        runs.push((n, offset));
        off += 8;
    }
    Ok((version, runs))
}

fn expand_runs_u32(runs: &[SttsRun], total: usize) -> Result<Vec<u32>> {
    let mut out = Vec::with_capacity(total);
    for run in runs {
        if run.count as usize > MAX_SAMPLES || run.delta == 0 {
            return Err(parse_error("bad stts run"));
        }
        for _ in 0..run.count {
            out.push(run.delta);
            if out.len() > total {
                return Err(parse_error("stts overruns sample count"));
            }
        }
    }
    if out.len() != total {
        return Err(parse_error("stts/sample count mismatch"));
    }
    Ok(out)
}

fn repack_runs_u32(values: &[u32]) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &v in values {
        if let Some(last) = runs.last_mut() {
            if last.1 == v {
                last.0 += 1;
                continue;
            }
        }
        runs.push((1u32, v));
    }
    runs
}

fn repack_runs_i64(values: &[i64]) -> Vec<(u32, i64)> {
    let mut runs: Vec<(u32, i64)> = Vec::new();
    for &v in values {
        if let Some(last) = runs.last_mut() {
            if last.1 == v {
                last.0 += 1;
                continue;
            }
        }
        runs.push((1, v));
    }
    runs
}

fn expand_ctts(runs: &[(u32, i64)], total: usize) -> Result<Vec<i64>> {
    let mut out = Vec::with_capacity(total);
    for (n, v) in runs {
        for _ in 0..*n {
            out.push(*v);
            if out.len() > total {
                return Err(parse_error("ctts overruns sample count"));
            }
        }
    }
    if out.len() != total {
        return Err(parse_error("ctts/sample count mismatch"));
    }
    Ok(out)
}

/// Shifts a span found inside `parent` (content range) to `moov` space.
fn abs_span(parent_content_start: usize, rel: Span) -> Span {
    Span {
        start: parent_content_start + rel.start,
        end: parent_content_start + rel.end,
        header: rel.header,
    }
}

fn parse_trak_tables(moov: &[u8], trak_span: Span) -> Result<ParsedTrak> {
    // All spans stored in ParsedTrak are moov-absolute.
    let (cs, ce) = trak_span.content_range();
    let trak = &moov[cs..ce];
    let kids = scan_children(trak)?;
    if kids.iter().any(|(t, _)| t == b"edts") {
        return Err(parse_error("edit lists unsupported in native trim"));
    }
    let tkhd = abs_span(
        cs,
        find_child(&kids, b"tkhd").ok_or_else(|| parse_error("trak missing tkhd"))?,
    );
    let mdia_abs = abs_span(
        cs,
        find_child(&kids, b"mdia").ok_or_else(|| parse_error("trak missing mdia"))?,
    );
    let (ms, me) = mdia_abs.content_range();
    let mdia = &moov[ms..me];
    let mdia_kids = scan_children(mdia)?;
    let mdhd = abs_span(
        ms,
        find_child(&mdia_kids, b"mdhd").ok_or_else(|| parse_error("mdia missing mdhd"))?,
    );
    let hdlr = find_child(&mdia_kids, b"hdlr").ok_or_else(|| parse_error("mdia missing hdlr"))?;
    let (hs, he) = hdlr.content_range();
    if he - hs < 12 {
        return Err(parse_error("truncated hdlr"));
    }
    let handler = fourcc(&mdia[hs + 8..hs + 12]).trim().to_string();
    if handler != "vide" && handler != "soun" {
        return Err(parse_error("native trim supports video+audio only"));
    }
    let minf_abs = abs_span(
        ms,
        find_child(&mdia_kids, b"minf").ok_or_else(|| parse_error("mdia missing minf"))?,
    );
    let (ns, ne) = minf_abs.content_range();
    let minf = &moov[ns..ne];
    let minf_kids = scan_children(minf)?;
    let stbl_abs = abs_span(
        ns,
        find_child(&minf_kids, b"stbl").ok_or_else(|| parse_error("minf missing stbl"))?,
    );
    let (ss, se) = stbl_abs.content_range();
    let stbl = &moov[ss..se];
    let stbl_kids = scan_children(stbl)?;
    if stbl_kids.iter().any(|(t, _)| t == b"stz2") {
        return Err(parse_error("compact sample sizes unsupported"));
    }
    let get = |name: &[u8; 4]| -> Result<Span> {
        find_child(&stbl_kids, name)
            .map(|s| abs_span(ss, s))
            .ok_or_else(|| parse_error("stbl missing box"))
    };
    let stsd_span = get(b"stsd")?;
    let (ts, te) = stsd_span.content_range();
    let stsd_box = moov[ts..te].to_vec();
    // Video fourcc = first sample entry.
    let mut video_fourcc = String::new();
    if stsd_box.len() >= 16 {
        video_fourcc = fourcc(&stsd_box[12..16]);
    }
    let stbl_kids_abs: Vec<([u8; 4], Span)> = stbl_kids
        .iter()
        .map(|(t, s)| (*t, abs_span(ss, *s)))
        .collect();
    let body_of = |name: &[u8; 4]| -> Result<Vec<u8>> {
        let s = stbl_kids_abs
            .iter()
            .find(|(t, _)| t == name)
            .map(|(_, s)| *s)
            .ok_or_else(|| parse_error("stbl missing box"))?;
        let (a, b) = s.content_range();
        Ok(moov[a..b].to_vec())
    };
    let stts = parse_stts_body(&body_of(b"stts")?)?;
    let stsc = parse_stsc_body(&body_of(b"stsc")?)?;
    let (stsz_uniform, stsz_table) = parse_stsz_body(&body_of(b"stsz")?)?;
    let has_stco = stbl_kids_abs.iter().any(|(t, _)| t == b"stco");
    let has_co64 = stbl_kids_abs.iter().any(|(t, _)| t == b"co64");
    let (wide, off_body) = match (has_stco, has_co64) {
        (true, false) => (false, body_of(b"stco")?),
        (false, true) => (true, body_of(b"co64")?),
        _ => return Err(parse_error("need exactly one stco/co64")),
    };
    let chunk_offsets = parse_offset_table(&off_body, wide)?;
    let stss = match stbl_kids_abs.iter().find(|(t, _)| t == b"stss") {
        Some((_, s)) => {
            let (a, b) = s.content_range();
            Some(parse_stss_body(&moov[a..b])?)
        }
        None => None,
    };
    let (ctts_version, ctts) = match stbl_kids_abs.iter().find(|(t, _)| t == b"ctts") {
        Some((_, s)) => {
            let (a, b) = s.content_range();
            let (v, runs) = parse_ctts_body(&moov[a..b])?;
            (Some(v), runs)
        }
        None => (None, Vec::new()),
    };
    let (ts, te) = tkhd.content_range();
    let tkhd_body = &moov[ts..te];
    if tkhd_body.is_empty() {
        return Err(parse_error("truncated tkhd"));
    }
    let (ts, te) = mdhd.content_range();
    let mdhd_body = &moov[ts..te];
    if mdhd_body.len() < 16 {
        return Err(parse_error("truncated mdhd"));
    }
    let mdhd_version = mdhd_body[0];
    let media_timescale = if mdhd_version == 1 {
        be32(mdhd_body, 20)?
    } else {
        be32(mdhd_body, 12)?
    };
    if media_timescale == 0 {
        return Err(parse_error("null media timescale"));
    }
    Ok(ParsedTrak {
        handler,
        tkhd,
        tkhd_version: tkhd_body[0],
        mdhd,
        mdhd_version,
        media_timescale,
        stbl: stbl_abs,
        stsd_box,
        stts,
        stsc,
        stsz_uniform,
        stsz_table,
        chunk_offsets_64: wide,
        chunk_offsets,
        stss,
        ctts_version,
        ctts,
        video_fourcc,
        stbl_kids: stbl_kids_abs,
    })
}

#[derive(Debug, Clone)]
struct ExpandedSamples {
    offsets: Vec<u64>,
    sizes: Vec<u32>,
    deltas: Vec<u32>,
}

fn expand_samples(trak: &ParsedTrak) -> Result<ExpandedSamples> {
    let chunks = trak.chunk_offsets.len();
    // Map each chunk to its stsc entry.
    let mut total = 0usize;
    let mut per_chunk = Vec::with_capacity(chunks);
    for c in 1..=chunks as u32 {
        let mut entry = &trak.stsc[0];
        for e in &trak.stsc {
            if e.0 <= c {
                entry = e;
            } else {
                break;
            }
        }
        if entry.1 == 0 || entry.1 as usize > MAX_SAMPLES {
            return Err(parse_error("bad stsc samples-per-chunk"));
        }
        per_chunk.push(entry.1 as usize);
        total = total
            .checked_add(entry.1 as usize)
            .ok_or_else(|| parse_error("sample count overflow"))?;
    }
    if total == 0 || total > MAX_SAMPLES {
        return Err(parse_error("sample count insane"));
    }
    let sizes: Vec<u32> = if trak.stsz_uniform != 0 {
        vec![trak.stsz_uniform; total]
    } else {
        if trak.stsz_table.len() != total {
            return Err(parse_error("stsz/sample count mismatch"));
        }
        trak.stsz_table.clone()
    };
    let deltas = expand_runs_u32(&trak.stts, total)?;
    let mut offsets = Vec::with_capacity(total);
    let mut sizes_out = Vec::with_capacity(total);
    let mut global = 0usize;
    for (c, &n) in per_chunk.iter().enumerate() {
        let mut pos = trak.chunk_offsets[c];
        for _ in 0..n {
            if global >= total {
                return Err(parse_error("stsc overruns samples"));
            }
            offsets.push(pos);
            sizes_out.push(sizes[global]);
            pos += sizes[global] as u64;
            global += 1;
        }
    }
    if global != total {
        return Err(parse_error("chunk/sample count mismatch"));
    }
    Ok(ExpandedSamples {
        offsets,
        sizes: sizes_out,
        deltas,
    })
}

fn patch_u32(content: &mut [u8], off: usize, value: u32) -> Result<()> {
    if content.len() < off + 4 {
        return Err(parse_error("duration patch out of range"));
    }
    content[off..off + 4].copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn patch_u64(content: &mut [u8], off: usize, value: u64) -> Result<()> {
    if content.len() < off + 8 {
        return Err(parse_error("duration patch out of range"));
    }
    content[off..off + 8].copy_from_slice(&value.to_be_bytes());
    Ok(())
}

fn checked_u32(value: u64, what: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| parse_error(what))
}

/// Lossless native trim of a single-track `.mov`: selects samples in
/// `[start_secs, end_secs)`, rewrites sample tables and durations, and
/// returns the new file bytes (ftyp + moov + single mdat, faststart
/// order). No ffmpeg involved.
pub(crate) fn trim_mov_bytes(data: &[u8], start_secs: f64, end_secs: f64) -> Result<Vec<u8>> {
    if data.len() > MAX_MOV_FILE_SIZE {
        return Err(parse_error("file too large for native trim"));
    }
    let top = scan_children(data)?;
    let ftyp = top
        .iter()
        .find(|(t, _)| t == b"ftyp")
        .map(|(_, s)| *s)
        .ok_or_else(|| parse_error("missing ftyp"))?;
    let moov_span = top
        .iter()
        .find(|(t, _)| t == b"moov")
        .map(|(_, s)| *s)
        .ok_or_else(|| parse_error("missing moov"))?;
    let mdats: Vec<Span> = top.iter().filter(|(t, _)| t == b"mdat").map(|(_, s)| *s).collect();
    if mdats.len() != 1 {
        return Err(parse_error("native trim needs exactly one mdat"));
    }
    let (ms, me) = moov_span.content_range();
    let moov = &data[ms..me];
    let moov_kids = scan_children(moov)?;
    let mvhd_span =
        find_child(&moov_kids, b"mvhd").ok_or_else(|| parse_error("moov missing mvhd"))?;
    let (vs, ve) = mvhd_span.content_range();
    let mvhd_body = &moov[vs..ve];
    if mvhd_body.len() < 20 {
        return Err(parse_error("truncated mvhd"));
    }
    let mvhd_version = mvhd_body[0];
    let movie_timescale = if mvhd_version == 1 {
        be32(mvhd_body, 20)?
    } else {
        be32(mvhd_body, 12)?
    };
    if movie_timescale == 0 {
        return Err(parse_error("null movie timescale"));
    }
    let trak_spans: Vec<Span> = moov_kids
        .iter()
        .filter(|(t, _)| t == b"trak")
        .map(|(_, s)| *s)
        .collect();
    if trak_spans.len() > 1 {
        let (ds, de) = mdats[0].content_range();
        return trim_mov_bytes_multi(
            data,
            moov,
            &trak_spans,
            &data[ds..de],
            ds as u64,
            movie_timescale,
            mvhd_span,
            mvhd_version,
            start_secs,
            end_secs,
            ftyp,
        );
    }
    if trak_spans.is_empty() {
        return Err(parse_error("missing trak"));
    }
    // scan_children ran on the moov content, so spans are already
    // moov-relative.
    let trak_rel = trak_spans[0];
    let trak = parse_trak_tables(moov, trak_rel)?;
    let samples = expand_samples(&trak)?;
    let media_ts = trak.media_timescale as f64;
    let s_ts = (start_secs * media_ts).round() as u64;
    let e_ts = (end_secs * media_ts).round() as u64;
    let mut time = 0u64;
    let mut selected: Vec<usize> = Vec::new();
    for (i, &delta) in samples.deltas.iter().enumerate() {
        if time >= s_ts && time < e_ts {
            selected.push(i);
        }
        time += delta as u64;
    }
    if selected.is_empty() {
        return Err(MediaError::InvalidSeek(format!("{start_secs}-{end_secs}")));
    }
    let n = selected.len();
    let new_deltas: Vec<u32> = selected.iter().map(|&i| samples.deltas[i]).collect();
    let new_sizes: Vec<u32> = selected.iter().map(|&i| samples.sizes[i]).collect();
    let media_duration: u64 = new_deltas.iter().map(|&d| d as u64).sum();
    let movie_duration =
        ((media_duration as u128 * movie_timescale as u128 + trak.media_timescale as u128 / 2)
            / trak.media_timescale as u128) as u64;
    // Copy selected sample bytes.
    let (ds, de) = mdats[0].content_range();
    let mdat = &data[ds..de];
    let mdat_base = ds as u64;
    let mut new_mdat = Vec::new();
    for &i in &selected {
        let off = samples.offsets[i]
            .checked_sub(mdat_base)
            .ok_or_else(|| parse_error("sample outside mdat"))? as usize;
        let size = samples.sizes[i] as usize;
        if off + size > mdat.len() {
            return Err(parse_error("sample overruns mdat"));
        }
        new_mdat.extend_from_slice(&mdat[off..off + size]);
    }
    // ctts slice.
    let new_ctts: Option<(u8, Vec<(u32, i64)>)> = match trak.ctts_version {
        Some(v) => {
            let expanded = expand_ctts(&trak.ctts, samples.deltas.len())?;
            let sliced: Vec<i64> = selected.iter().map(|&i| expanded[i]).collect();
            Some((v, repack_runs_i64(&sliced)))
        }
        None => None,
    };
    // stss remap (1-based).
    let new_stss: Option<Vec<u32>> = trak.stss.as_ref().map(|sync| {
        let set: std::collections::HashSet<u32> = sync.iter().copied().collect();
        selected
            .iter()
            .enumerate()
            .filter(|(_, &orig)| set.contains(&(orig as u32 + 1)))
            .map(|(pos, _)| pos as u32 + 1)
            .collect()
    });
    // stts rebuild.
    let mut stts_payload = vec![0u8; 8];
    let stts_runs = repack_runs_u32(&new_deltas);
    stts_payload[4..8].copy_from_slice(&(stts_runs.len() as u32).to_be_bytes());
    for (count, delta) in &stts_runs {
        stts_payload.extend_from_slice(&count.to_be_bytes());
        stts_payload.extend_from_slice(&delta.to_be_bytes());
    }
    let new_stts = make_box(b"stts", &stts_payload)?;
    // stsc single chunk.
    let desc_id = trak.stsc[0].2;
    let mut stsc_payload = vec![0u8; 8];
    stsc_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&(n as u32).to_be_bytes());
    stsc_payload.extend_from_slice(&desc_id.to_be_bytes());
    let new_stsc = make_box(b"stsc", &stsc_payload)?;
    // stsz uniform when possible.
    let uniform = new_sizes.iter().all(|&s| s == new_sizes[0]);
    let mut stsz_payload = vec![0u8; 12];
    if uniform {
        stsz_payload[4..8].copy_from_slice(&new_sizes[0].to_be_bytes());
        stsz_payload[8..12].copy_from_slice(&(n as u32).to_be_bytes());
    } else {
        stsz_payload[8..12].copy_from_slice(&(n as u32).to_be_bytes());
        for &s in &new_sizes {
            stsz_payload.extend_from_slice(&s.to_be_bytes());
        }
    }
    let new_stsz = make_box(b"stsz", &stsz_payload)?;
    // stss rebuild.
    let mut new_stss_box: Option<Vec<u8>> = None;
    if let Some(remapped) = &new_stss {
        let mut payload = vec![0u8; 8];
        payload[4..8].copy_from_slice(&(remapped.len() as u32).to_be_bytes());
        for &s in remapped {
            payload.extend_from_slice(&s.to_be_bytes());
        }
        new_stss_box = Some(make_box(b"stss", &payload)?);
    }
    // ctts rebuild.
    let mut new_ctts_box: Option<Vec<u8>> = None;
    if let Some((version, runs)) = &new_ctts {
        let mut payload = vec![*version, 0, 0, 0];
        payload.extend_from_slice(&(runs.len() as u32).to_be_bytes());
        for (count, offset) in runs {
            payload.extend_from_slice(&count.to_be_bytes());
            if *version == 1 {
                payload.extend_from_slice(&(*offset as i32).to_be_bytes());
            } else {
                payload.extend_from_slice(&(*offset as u32).to_be_bytes());
            }
        }
        new_ctts_box = Some(make_box(b"ctts", &payload)?);
    }
/// Builds the single-entry `stco`/`co64` box for a new chunk offset.
fn chunk_offset_box(wide: bool, chunk_offset: u64) -> Result<Vec<u8>> {
    let mut payload = vec![0u8; 8];
    payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    if wide {
        payload.extend_from_slice(&chunk_offset.to_be_bytes());
        make_box(b"co64", &payload)
    } else {
        payload.extend_from_slice(
            &checked_u32(chunk_offset, "mdat offset overflow")?.to_be_bytes(),
        );
        make_box(b"stco", &payload)
    }
}

/// Rebuilds one trak box with a new `stbl`, resolving `mdia`/`minf`
/// against `base` (a moov-payload copy with identical layout).
fn rebuild_trak(base: &[u8], trak_rel: Span, new_stbl: Vec<u8>) -> Result<Vec<u8>> {
    let (tcs, tce) = trak_rel.content_range();
    let trak_bytes = base
        .get(tcs..tce)
        .ok_or_else(|| parse_error("trak out of range"))?;
    let trak_kids = scan_children(trak_bytes)?;
    let mdia_box =
        find_child(&trak_kids, b"mdia").ok_or_else(|| parse_error("trak missing mdia"))?;
    let mdia_abs = Span {
        start: tcs + mdia_box.start,
        end: tcs + mdia_box.end,
        header: mdia_box.header,
    };
    let (mdcs, mdce) = mdia_abs.content_range();
    let mdia_bytes = base
        .get(mdcs..mdce)
        .ok_or_else(|| parse_error("mdia out of range"))?;
    let mdia_kids = scan_children(mdia_bytes)?;
    let minf_box =
        find_child(&mdia_kids, b"minf").ok_or_else(|| parse_error("mdia missing minf"))?;
    let minf_abs = Span {
        start: mdcs + minf_box.start,
        end: mdcs + minf_box.end,
        header: minf_box.header,
    };
    let (mfs, mfe) = minf_abs.content_range();
    let minf_payload = splice(&base[mfs..mfe], &[(*b"stbl", new_stbl)])?;
    let new_minf = make_box(b"minf", &minf_payload)?;
    let (mds, mde) = mdia_abs.content_range();
    let mdia_payload = splice(&base[mds..mde], &[(*b"minf", new_minf)])?;
    let new_mdia = make_box(b"mdia", &mdia_payload)?;
    let trak_payload = splice(&base[tcs..tce], &[(*b"mdia", new_mdia)])?;
    make_box(b"trak", &trak_payload)
}

/// Lossless native trim of a multi-track `.mov`: each video/audio
/// track is cut independently in its own timescale, samples land in
/// one chunk per track. No ffmpeg involved.
#[allow(clippy::too_many_arguments)]
fn trim_mov_bytes_multi(
    data: &[u8],
    moov: &[u8],
    trak_spans: &[Span],
    mdat: &[u8],
    mdat_base: u64,
    movie_timescale: u32,
    mvhd_span: Span,
    mvhd_version: u8,
    start_secs: f64,
    end_secs: f64,
    ftyp: Span,
) -> Result<Vec<u8>> {
    struct Cut {
        trak_rel: Span,
        trak: ParsedTrak,
        data: Vec<u8>,
        media_duration: u64,
        stts: Vec<u8>,
        stsc: Vec<u8>,
        stsz: Vec<u8>,
        stss: Option<Vec<u8>>,
        ctts: Option<Vec<u8>>,
    }
    let mut cuts: Vec<Cut> = Vec::new();
    for rel in trak_spans {
        let trak = parse_trak_tables(moov, *rel)?;
        if trak.handler != "vide" && trak.handler != "soun" {
            return Err(parse_error("native trim supports video+audio only"));
        }
        let samples = expand_samples(&trak)?;
        let media_ts = trak.media_timescale as f64;
        let s_ts = (start_secs * media_ts).round() as u64;
        let e_ts = (end_secs * media_ts).round() as u64;
        let mut time = 0u64;
        let mut selected = Vec::new();
        for (i, &delta) in samples.deltas.iter().enumerate() {
            if time >= s_ts && time < e_ts {
                selected.push(i);
            }
            time += delta as u64;
        }
        if selected.is_empty() {
            return Err(MediaError::InvalidSeek(format!("{start_secs}-{end_secs}")));
        }
        let n = selected.len();
        let deltas: Vec<u32> = selected.iter().map(|&i| samples.deltas[i]).collect();
        let sizes: Vec<u32> = selected.iter().map(|&i| samples.sizes[i]).collect();
        let media_duration: u64 = deltas.iter().map(|&d| d as u64).sum();
        let mut track_data = Vec::new();
        for &i in &selected {
            let off = samples.offsets[i]
                .checked_sub(mdat_base)
                .ok_or_else(|| parse_error("sample outside mdat"))?
                as usize;
            let size = samples.sizes[i] as usize;
            if off + size > mdat.len() {
                return Err(parse_error("sample overruns mdat"));
            }
            track_data.extend_from_slice(&mdat[off..off + size]);
        }
        let mut stts_payload = vec![0u8; 8];
        let runs = repack_runs_u32(&deltas);
        stts_payload[4..8].copy_from_slice(&(runs.len() as u32).to_be_bytes());
        for (count, delta) in &runs {
            stts_payload.extend_from_slice(&count.to_be_bytes());
            stts_payload.extend_from_slice(&delta.to_be_bytes());
        }
        let new_stts = make_box(b"stts", &stts_payload)?;
        let mut stsc_payload = vec![0u8; 8];
        stsc_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
        stsc_payload.extend_from_slice(&1u32.to_be_bytes());
        stsc_payload.extend_from_slice(&(n as u32).to_be_bytes());
        stsc_payload.extend_from_slice(&trak.stsc[0].2.to_be_bytes());
        let new_stsc = make_box(b"stsc", &stsc_payload)?;
        let uniform = sizes.iter().all(|&s| s == sizes[0]);
        let mut stsz_payload = vec![0u8; 12];
        if uniform {
            stsz_payload[4..8].copy_from_slice(&sizes[0].to_be_bytes());
            stsz_payload[8..12].copy_from_slice(&(n as u32).to_be_bytes());
        } else {
            stsz_payload[8..12].copy_from_slice(&(n as u32).to_be_bytes());
            for &s in &sizes {
                stsz_payload.extend_from_slice(&s.to_be_bytes());
            }
        }
        let new_stsz = make_box(b"stsz", &stsz_payload)?;
        let new_stss = match &trak.stss {
            Some(sync) => {
                let set: std::collections::HashSet<u32> = sync.iter().copied().collect();
                let remapped: Vec<u32> = selected
                    .iter()
                    .enumerate()
                    .filter(|(_, &orig)| set.contains(&(orig as u32 + 1)))
                    .map(|(pos, _)| pos as u32 + 1)
                    .collect();
                let mut payload = vec![0u8; 8];
                payload[4..8].copy_from_slice(&(remapped.len() as u32).to_be_bytes());
                for &s in &remapped {
                    payload.extend_from_slice(&s.to_be_bytes());
                }
                Some(make_box(b"stss", &payload)?)
            }
            None => None,
        };
        let new_ctts = match trak.ctts_version {
            Some(v) => {
                let expanded = expand_ctts(&trak.ctts, samples.deltas.len())?;
                let sliced: Vec<i64> = selected.iter().map(|&i| expanded[i]).collect();
                let runs = repack_runs_i64(&sliced);
                let mut payload = vec![v, 0, 0, 0];
                payload.extend_from_slice(&(runs.len() as u32).to_be_bytes());
                for (count, offset) in &runs {
                    payload.extend_from_slice(&count.to_be_bytes());
                    if v == 1 {
                        payload.extend_from_slice(&(*offset as i32).to_be_bytes());
                    } else {
                        payload.extend_from_slice(&(*offset as u32).to_be_bytes());
                    }
                }
                Some(make_box(b"ctts", &payload)?)
            }
            None => None,
        };
        cuts.push(Cut {
            trak_rel: *rel,
            trak,
            data: track_data,
            media_duration,
            stts: new_stts,
            stsc: new_stsc,
            stsz: new_stsz,
            stss: new_stss,
            ctts: new_ctts,
        });
    }
    // Movie duration = longest track, scaled to the movie timescale.
    let mut movie_duration = 0u64;
    for c in &cuts {
        let scaled = (c.media_duration as u128 * movie_timescale as u128
            + c.trak.media_timescale as u128 / 2)
            / c.trak.media_timescale as u128;
        movie_duration = movie_duration.max(scaled as u64);
    }
    let mut patched = moov.to_vec();
    {
        let (s, e) = mvhd_span.content_range();
        let body = &mut patched[s..e];
        if mvhd_version == 1 {
            patch_u64(body, 24, movie_duration)?;
        } else {
            patch_u32(body, 16, checked_u32(movie_duration, "movie duration overflow")?)?;
        }
    }
    for c in &cuts {
        let (s, e) = c.trak.mdhd.content_range();
        let body = &mut patched[s..e];
        if c.trak.mdhd_version == 1 {
            patch_u64(body, 24, c.media_duration)?;
        } else {
            patch_u32(
                body,
                16,
                checked_u32(c.media_duration, "media duration overflow")?,
            )?;
        }
        let (s, e) = c.trak.tkhd.content_range();
        let body = &mut patched[s..e];
        if c.trak.tkhd_version == 1 {
            patch_u64(body, 28, movie_duration)?;
        } else {
            patch_u32(body, 20, checked_u32(movie_duration, "track duration overflow")?)?;
        }
    }
    // Two passes: measure the moov with placeholder offsets, then
    // lay out one chunk per track.
    let build_all = |offsets: &[u64]| -> Result<Vec<Vec<u8>>> {
        let mut traks = Vec::with_capacity(cuts.len());
        for (c, &off) in cuts.iter().zip(offsets) {
            let mut repl: Vec<([u8; 4], Vec<u8>)> = vec![
                (*b"stts", c.stts.clone()),
                (*b"stsc", c.stsc.clone()),
                (*b"stsz", c.stsz.clone()),
                (
                    if c.trak.chunk_offsets_64 { *b"co64" } else { *b"stco" },
                    chunk_offset_box(c.trak.chunk_offsets_64, off)?,
                ),
            ];
            if let Some(b) = &c.stss {
                repl.push((*b"stss", b.clone()));
            }
            if let Some(b) = &c.ctts {
                repl.push((*b"ctts", b.clone()));
            }
            let (ss, se) = c.trak.stbl.content_range();
            let payload = splice(&patched[ss..se], &repl)?;
            traks.push(rebuild_trak(&patched, c.trak_rel, make_box(b"stbl", &payload)?)?);
        }
        Ok(traks)
    };
    let assemble = |new_traks: Vec<Vec<u8>>| -> Result<Vec<u8>> {
        let kids = scan_children(&patched)?;
        let mut out = Vec::new();
        let mut ti = 0usize;
        for (typ, span) in kids {
            if &typ == b"trak" {
                out.extend_from_slice(new_traks.get(ti).ok_or_else(|| {
                    parse_error("trak count mismatch")
                })?);
                ti += 1;
            } else {
                out.extend_from_slice(&patched[span.start..span.end]);
            }
        }
        if ti != new_traks.len() {
            return Err(parse_error("trak count mismatch"));
        }
        make_box(b"moov", &out)
    };
    let ftyp_len = ftyp.end - ftyp.start;
    let zeros = vec![0u64; cuts.len()];
    let moov_zero = assemble(build_all(&zeros)?)?;
    let mut offsets = Vec::with_capacity(cuts.len());
    let mut cursor = ftyp_len as u64 + moov_zero.len() as u64 + 8;
    for c in &cuts {
        offsets.push(cursor);
        cursor += c.data.len() as u64;
    }
    let moov_final = assemble(build_all(&offsets)?)?;
    let mut mdat_payload = Vec::new();
    for c in &cuts {
        mdat_payload.extend_from_slice(&c.data);
    }
    let new_mdat = make_box(b"mdat", &mdat_payload)?;
    let mut out =
        Vec::with_capacity(ftyp_len + moov_final.len() + new_mdat.len());
    out.extend_from_slice(&data[ftyp.start..ftyp.end]);
    out.extend_from_slice(&moov_final);
    out.extend_from_slice(&new_mdat);
    let _ = data.len();
    Ok(out)
}
    // Resolve the mdia/minf box spans as moov-absolute box spans
    // (content_range adds the header, so never re-wrap its output).
    let (tcs, tce) = trak_rel.content_range();
    let trak_bytes = &moov[tcs..tce];
    let trak_kids = scan_children(trak_bytes)?;
    let mdia_box = find_child(&trak_kids, b"mdia").ok_or_else(|| parse_error("trak missing mdia"))?;
    let mdia_abs = Span {
        start: tcs + mdia_box.start,
        end: tcs + mdia_box.end,
        header: mdia_box.header,
    };
    let (mdcs, mdce) = mdia_abs.content_range();
    let mdia_bytes = &moov[mdcs..mdce];
    let mdia_kids = scan_children(mdia_bytes)?;
    let minf_box = find_child(&mdia_kids, b"minf").ok_or_else(|| parse_error("mdia missing minf"))?;
    let minf_abs = Span {
        start: mdcs + minf_box.start,
        end: mdcs + minf_box.end,
        header: minf_box.header,
    };
    // Patched copies of the moov payload for duration fields.
    let mut patched = moov.to_vec();
    {
        let (s, e) = mvhd_span.content_range();
        let body = &mut patched[s..e];
        if mvhd_version == 1 {
            patch_u64(body, 24, movie_duration)?;
        } else {
            patch_u32(body, 16, checked_u32(movie_duration, "movie duration overflow")?)?;
        }
    }
    {
        let (s, e) = trak.mdhd.content_range();
        let body = &mut patched[s..e];
        if trak.mdhd_version == 1 {
            patch_u64(body, 24, media_duration)?;
        } else {
            patch_u32(body, 16, checked_u32(media_duration, "media duration overflow")?)?;
        }
    }
    {
        let (s, e) = trak.tkhd.content_range();
        let body = &mut patched[s..e];
        if trak.tkhd_version == 1 {
            patch_u64(body, 28, movie_duration)?;
        } else {
            patch_u32(body, 20, checked_u32(movie_duration, "track duration overflow")?)?;
        }
    }
    // Shared stbl replacements (offset box filled per pass).
    let stbl_base: Vec<([u8; 4], Vec<u8>)> = {
        let mut repl: Vec<([u8; 4], Vec<u8>)> = vec![
            (*b"stts", new_stts),
            (*b"stsc", new_stsc),
            (*b"stsz", new_stsz),
        ];
        if let Some(b) = new_stss_box {
            repl.push((*b"stss", b));
        }
        if let Some(b) = new_ctts_box {
            repl.push((*b"ctts", b));
        }
        repl
    };
    let off_name = if trak.chunk_offsets_64 { *b"co64" } else { *b"stco" };
    let build_stbl = |chunk_offset: u64| -> Result<Vec<u8>> {
        let mut repl = stbl_base.clone();
        repl.push((off_name, chunk_offset_box(trak.chunk_offsets_64, chunk_offset)?));
        let (ss, se) = trak.stbl.content_range();
        // stbl carries no duration fields; `patched` copy is identical.
        let payload = splice(&patched[ss..se], &repl)?;
        make_box(b"stbl", &payload)
    };
    let assemble = |new_stbl: Vec<u8>| -> Result<Vec<u8>> {
        let (mfs, mfe) = minf_abs.content_range();
        let minf_payload = splice(&patched[mfs..mfe], &[( *b"stbl", new_stbl)])?;
        let new_minf = make_box(b"minf", &minf_payload)?;
        let (mds, mde) = mdia_abs.content_range();
        let mdia_payload = splice(&patched[mds..mde], &[(*b"minf", new_minf)])?;
        let new_mdia = make_box(b"mdia", &mdia_payload)?;
        let (tcs, tce) = trak_rel.content_range();
        let trak_payload = splice(&patched[tcs..tce], &[(*b"mdia", new_mdia)])?;
        let new_trak = make_box(b"trak", &trak_payload)?;
        let moov_payload = splice(&patched, &[(*b"trak", new_trak)])?;
        make_box(b"moov", &moov_payload)
    };
    // Pass 1 with placeholder offset to measure the moov size.
    let ftyp_len = ftyp.end - ftyp.start;
    let moov_zero = assemble(build_stbl(0)?)?;
    let chunk_offset = ftyp_len as u64 + moov_zero.len() as u64 + 8;
    // Pass 2 with the real chunk offset.
    let moov_final = assemble(build_stbl(chunk_offset)?)?;
    let new_mdat = make_box(b"mdat", &new_mdat)?;
    let mut out = Vec::with_capacity(ftyp_len + moov_final.len() + new_mdat.len());
    out.extend_from_slice(&data[ftyp.start..ftyp.end]);
    out.extend_from_slice(&moov_final);
    out.extend_from_slice(&new_mdat);
    Ok(out)
}

/// Builds the PCM (`sowt`, s16le) audio `trak` for [`build_raw_mov`].
fn audio_trak(chunk_offset: u32, a: &RawAudioParams, samples: u32) -> Result<Vec<u8>> {
    let frame_bytes = 2 * a.channels as usize;
    // stsd with one `sowt` entry.
    let mut entry = Vec::new();
    entry.extend_from_slice(&[0u8; 8]); // size + fourcc filled below
    entry.extend_from_slice(&[0u8; 6]); // reserved
    entry.extend_from_slice(&1u16.to_be_bytes()); // dataref
    entry.extend_from_slice(&0u16.to_be_bytes()); // version
    entry.extend_from_slice(&0u16.to_be_bytes()); // revision
    entry.extend_from_slice(&0u32.to_be_bytes()); // vendor
    entry.extend_from_slice(&a.channels.to_be_bytes());
    entry.extend_from_slice(&16u16.to_be_bytes()); // sample size
    entry.extend_from_slice(&0u16.to_be_bytes()); // compression id
    entry.extend_from_slice(&0u16.to_be_bytes()); // packet size
    entry.extend_from_slice(&((a.sample_rate << 16) as u32).to_be_bytes()); // 16.16 rate
    let entry_len = entry.len() as u32;
    entry[0..4].copy_from_slice(&entry_len.to_be_bytes());
    entry[4..8].copy_from_slice(b"sowt");
    let mut stsd_payload = vec![0u8; 8];
    stsd_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&entry);
    let stsd_box = make_box(b"stsd", &stsd_payload)?;
    let mut stts_payload = vec![0u8; 8];
    stts_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stts_payload.extend_from_slice(&samples.to_be_bytes());
    stts_payload.extend_from_slice(&1u32.to_be_bytes());
    let stts_box = make_box(b"stts", &stts_payload)?;
    let mut stsc_payload = vec![0u8; 8];
    stsc_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&samples.to_be_bytes());
    stsc_payload.extend_from_slice(&1u32.to_be_bytes());
    let stsc_box = make_box(b"stsc", &stsc_payload)?;
    let mut stsz_payload = vec![0u8; 12];
    stsz_payload[4..8].copy_from_slice(&(frame_bytes as u32).to_be_bytes());
    stsz_payload[8..12].copy_from_slice(&samples.to_be_bytes());
    let stsz_box = make_box(b"stsz", &stsz_payload)?;
    let mut stco_payload = vec![0u8; 8];
    stco_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stco_payload.extend_from_slice(&chunk_offset.to_be_bytes());
    let stco_box = make_box(b"stco", &stco_payload)?;
    let mut stbl_payload = Vec::new();
    stbl_payload.extend_from_slice(&stsd_box);
    stbl_payload.extend_from_slice(&stts_box);
    stbl_payload.extend_from_slice(&stsc_box);
    stbl_payload.extend_from_slice(&stsz_box);
    stbl_payload.extend_from_slice(&stco_box);
    let stbl_box = make_box(b"stbl", &stbl_payload)?;
    // smhd / dinf / minf.
    let smhd_box = make_box(b"smhd", &vec![0u8; 8])?;
    let dinf_box = dref_self_contained()?;
    let mut minf_payload = Vec::new();
    minf_payload.extend_from_slice(&smhd_box);
    minf_payload.extend_from_slice(&dinf_box);
    minf_payload.extend_from_slice(&stbl_box);
    let minf_box = make_box(b"minf", &minf_payload)?;
    // mdhd (timescale = sample rate) / hdlr / mdia.
    let mut mdhd_payload = vec![0u8; 24];
    mdhd_payload[12..16].copy_from_slice(&a.sample_rate.to_be_bytes());
    mdhd_payload[16..20].copy_from_slice(&samples.to_be_bytes());
    mdhd_payload[20..22].copy_from_slice(&0x55C4u16.to_be_bytes());
    let mdhd_box = make_box(b"mdhd", &mdhd_payload)?;
    let mut hdlr_payload = vec![0u8; 8];
    hdlr_payload.extend_from_slice(b"soun");
    hdlr_payload.extend_from_slice(&[0u8; 12]);
    hdlr_payload.extend_from_slice(b"SoundHandler\0");
    let hdlr_box = make_box(b"hdlr", &hdlr_payload)?;
    let mut mdia_payload = Vec::new();
    mdia_payload.extend_from_slice(&mdhd_box);
    mdia_payload.extend_from_slice(&hdlr_box);
    mdia_payload.extend_from_slice(&minf_box);
    let mdia_box = make_box(b"mdia", &mdia_payload)?;
    let tkhd_box = tkhd_box(
        2,
        // tkhd runs on the movie timescale (600 here).
        ((samples as u64 * 600 / a.sample_rate as u64) as u32),
        0x100,
        0,
        0,
    )?;
    let mut trak_payload = Vec::new();
    trak_payload.extend_from_slice(&tkhd_box);
    trak_payload.extend_from_slice(&mdia_box);
    make_box(b"trak", &trak_payload)
}

// ---------- Minimal raw RGB .mov writer (fixtures, capture targets) ----------

/// Parameters for [`build_raw_mov`].
#[derive(Debug, Clone, Default)]
pub struct RawMovParams {
    pub width: u16,
    pub height: u16,
    pub fps: u32,
    pub audio: Option<RawAudioParams>,
}

/// PCM audio track parameters (s16le interleaved) for [`build_raw_mov`].
#[derive(Debug, Clone)]
pub struct RawAudioParams {
    pub sample_rate: u32,
    pub channels: u16,
    pub pcm: Vec<u8>,
}

/// Builds a `tkhd` v0 box: version/flags, creation/modification,
/// track id, duration (movie timescale), layer/group/volume,
/// identity matrix, 16.16 dimensions.
fn tkhd_box(track_id: u32, duration: u32, volume: u16, w: u32, h: u32) -> Result<Vec<u8>> {
    let mut payload = vec![0u8; 84];
    payload[12..16].copy_from_slice(&track_id.to_be_bytes());
    payload[20..24].copy_from_slice(&duration.to_be_bytes());
    payload[36..38].copy_from_slice(&volume.to_be_bytes());
    payload[40..76].copy_from_slice(&identity_matrix());
    payload[76..80].copy_from_slice(&(w << 16).to_be_bytes());
    payload[80..84].copy_from_slice(&(h << 16).to_be_bytes());
    make_box(b"tkhd", &payload)
}

fn dref_self_contained() -> Result<Vec<u8>> {
    let mut url_entry = Vec::new();
    url_entry.extend_from_slice(&12u32.to_be_bytes());
    url_entry.extend_from_slice(b"url ");
    url_entry.extend_from_slice(&[0u8, 0, 0, 1]);
    let mut dref_payload = vec![0u8; 8];
    dref_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    dref_payload.extend_from_slice(&url_entry);
    let dref_box = make_box(b"dref", &dref_payload)?;
    make_box(b"dinf", &dref_box)
}

fn identity_matrix() -> Vec<u8> {
    let words: [u32; 9] = [
        0x00010000, 0, 0, 0, 0x00010000, 0, 0, 0, 0x40000000,
    ];
    let mut out = Vec::with_capacity(36);
    for w in words {
        out.extend_from_slice(&w.to_be_bytes());
    }
    out
}

/// Builds a minimal playable raw-RGB24 QuickTime `.mov` (faststart:
/// ftyp + moov + mdat, codec `raw `, one video track plus optional
/// PCM `sowt` audio). Each frame must be exactly `width*height*3`
/// bytes; `fps` must divide 600 evenly. Audio is s16le interleaved;
/// the sample count is `pcm.len() / (2 * channels)`. Pure Rust.
pub fn build_raw_mov(params: &RawMovParams, frames: &[Vec<u8>]) -> Result<Vec<u8>> {
    if params.width == 0 || params.height == 0 || frames.is_empty() {
        return Err(parse_error("empty raw mov"));
    }
    if params.fps == 0 || 600 % params.fps != 0 {
        return Err(parse_error("fps must divide 600"));
    }
    if let Some(a) = &params.audio {
        if a.sample_rate == 0 || a.channels == 0 || a.pcm.is_empty() {
            return Err(parse_error("bad raw audio params"));
        }
        if a.pcm.len() % (2 * a.channels as usize) != 0 {
            return Err(parse_error("pcm size mismatch"));
        }
    }
    let frame_size = params.width as usize * params.height as usize * 3;
    for f in frames {
        if f.len() != frame_size {
            return Err(parse_error("frame size mismatch"));
        }
    }
    let n = frames.len() as u32;
    let delta = 600 / params.fps;
    let duration = n * delta;
    // ftyp.
    let mut ftyp_payload = Vec::new();
    ftyp_payload.extend_from_slice(b"qt  ");
    ftyp_payload.extend_from_slice(&0u32.to_be_bytes());
    ftyp_payload.extend_from_slice(b"qt  ");
    let ftyp_box = make_box(b"ftyp", &ftyp_payload)?;
    // stsd with one `raw ` entry.
    let mut entry = Vec::new();
    entry.extend_from_slice(&[0u8; 2]); // placeholder size filled below
    entry.extend_from_slice(&[0u8; 2]);
    entry.extend_from_slice(b"raw ");
    entry.extend_from_slice(&[0u8; 6]); // reserved
    entry.extend_from_slice(&1u16.to_be_bytes()); // dataref
    entry.extend_from_slice(&0u16.to_be_bytes()); // version
    entry.extend_from_slice(&0u16.to_be_bytes()); // revision
    entry.extend_from_slice(b"appl"); // vendor
    entry.extend_from_slice(&0u32.to_be_bytes()); // temporal quality
    entry.extend_from_slice(&0x00000200u32.to_be_bytes()); // spatial quality
    entry.extend_from_slice(&params.width.to_be_bytes());
    entry.extend_from_slice(&params.height.to_be_bytes());
    entry.extend_from_slice(&0x00480000u32.to_be_bytes()); // horiz resolution
    entry.extend_from_slice(&0x00480000u32.to_be_bytes()); // vert resolution
    entry.extend_from_slice(&0u32.to_be_bytes()); // data size
    entry.extend_from_slice(&1u16.to_be_bytes()); // frames per sample
    entry.extend_from_slice(&[0u8; 32]); // compressor name
    entry.extend_from_slice(&0x0018u16.to_be_bytes()); // depth 24
    entry.extend_from_slice(&0xFFFFu16.to_be_bytes()); // predefined
    let entry_len = entry.len() as u32;
    entry[0..4].copy_from_slice(&entry_len.to_be_bytes());
    let mut real_entry = entry_len.to_be_bytes().to_vec();
    real_entry.extend_from_slice(&entry[4..]);
    let mut stsd_payload = vec![0u8; 8];
    stsd_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stsd_payload.extend_from_slice(&real_entry);
    let stsd_box = make_box(b"stsd", &stsd_payload)?;
    // stts / stsc / stsz / stco.
    let mut stts_payload = vec![0u8; 8];
    stts_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stts_payload.extend_from_slice(&n.to_be_bytes());
    stts_payload.extend_from_slice(&delta.to_be_bytes());
    let stts_box = make_box(b"stts", &stts_payload)?;
    let mut stsc_payload = vec![0u8; 8];
    stsc_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&1u32.to_be_bytes());
    stsc_payload.extend_from_slice(&n.to_be_bytes());
    stsc_payload.extend_from_slice(&1u32.to_be_bytes());
    let stsc_box = make_box(b"stsc", &stsc_payload)?;
    let mut stsz_payload = vec![0u8; 12];
    stsz_payload[4..8].copy_from_slice(&(frame_size as u32).to_be_bytes());
    stsz_payload[8..12].copy_from_slice(&n.to_be_bytes());
    let stsz_box = make_box(b"stsz", &stsz_payload)?;
    // Audio sample layout (single chunk, one sample per PCM frame).
    let audio_samples: u32 = match &params.audio {
        Some(a) => (a.pcm.len() / (2 * a.channels as usize)) as u32,
        None => 0,
    };
    // stco placeholders, fixed after moov size is known.
    let build_moov = |video_off: u32, audio_off: u32| -> Result<Vec<u8>> {
        let mut stco_payload = vec![0u8; 8];
        stco_payload[4..8].copy_from_slice(&1u32.to_be_bytes());
        stco_payload.extend_from_slice(&video_off.to_be_bytes());
        let stco_box = make_box(b"stco", &stco_payload)?;
        let mut stbl_payload = Vec::new();
        stbl_payload.extend_from_slice(&stsd_box);
        stbl_payload.extend_from_slice(&stts_box);
        stbl_payload.extend_from_slice(&stsc_box);
        stbl_payload.extend_from_slice(&stsz_box);
        stbl_payload.extend_from_slice(&stco_box);
        let stbl_box = make_box(b"stbl", &stbl_payload)?;
        // vmhd / dinf(dref url) / minf.
        let mut vmhd_payload = vec![0u8, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0];
        let _ = &mut vmhd_payload;
        let vmhd_box = make_box(b"vmhd", &vmhd_payload)?;
        let dinf_box = dref_self_contained()?;
        let mut minf_payload = Vec::new();
        minf_payload.extend_from_slice(&vmhd_box);
        minf_payload.extend_from_slice(&dinf_box);
        minf_payload.extend_from_slice(&stbl_box);
        let minf_box = make_box(b"minf", &minf_payload)?;
        // mdhd / hdlr / mdia.
        let mut mdhd_payload = vec![0u8; 24];
        mdhd_payload[12..16].copy_from_slice(&600u32.to_be_bytes());
        mdhd_payload[16..20].copy_from_slice(&duration.to_be_bytes());
        mdhd_payload[20..22].copy_from_slice(&0x55C4u16.to_be_bytes());
        let mdhd_box = make_box(b"mdhd", &mdhd_payload)?;
        let mut hdlr_payload = vec![0u8; 8];
        hdlr_payload.extend_from_slice(b"vide");
        hdlr_payload.extend_from_slice(&[0u8; 12]);
        hdlr_payload.extend_from_slice(b"VideoHandler\0");
        let hdlr_box = make_box(b"hdlr", &hdlr_payload)?;
        let mut mdia_payload = Vec::new();
        mdia_payload.extend_from_slice(&mdhd_box);
        mdia_payload.extend_from_slice(&hdlr_box);
        mdia_payload.extend_from_slice(&minf_box);
        let mdia_box = make_box(b"mdia", &mdia_payload)?;
        let tkhd_box = tkhd_box(1, duration, 0, params.width as u32, params.height as u32)?;
        let mut trak_payload = Vec::new();
        trak_payload.extend_from_slice(&tkhd_box);
        trak_payload.extend_from_slice(&mdia_box);
        let trak_box = make_box(b"trak", &trak_payload)?;
        let mut moov_payload = Vec::new();
        moov_payload.extend_from_slice(&trak_box);
        if let Some(a) = &params.audio {
            moov_payload.extend_from_slice(&audio_trak(audio_off, a, audio_samples)?);
        }
        // mvhd.
        let mut mvhd_payload = vec![0u8; 100];
        mvhd_payload[12..16].copy_from_slice(&600u32.to_be_bytes());
        mvhd_payload[16..20].copy_from_slice(&duration.to_be_bytes());
        mvhd_payload[20..24].copy_from_slice(&0x00010000u32.to_be_bytes());
        mvhd_payload[24..26].copy_from_slice(&0x0100u16.to_be_bytes());
        mvhd_payload[32..68].copy_from_slice(&identity_matrix());
        let next_id = if params.audio.is_some() { 3u32 } else { 2u32 };
        mvhd_payload[96..100].copy_from_slice(&next_id.to_be_bytes());
        let mvhd_box = make_box(b"mvhd", &mvhd_payload)?;
        let mut full = Vec::new();
        full.extend_from_slice(&mvhd_box);
        full.extend_from_slice(&moov_payload);
        make_box(b"moov", &full)
    };
    let video_bytes: usize = frames.iter().map(|f| f.len()).sum();
    let audio_bytes: usize = params.audio.as_ref().map(|a| a.pcm.len()).unwrap_or(0);
    let moov_zero = build_moov(0, 0)?;
    let video_off = (ftyp_box.len() + moov_zero.len() + 8) as u32;
    let audio_off = video_off + video_bytes as u32;
    let moov_box = build_moov(video_off, audio_off)?;
    let mut mdat_payload = Vec::with_capacity(video_bytes + audio_bytes);
    for f in frames {
        mdat_payload.extend_from_slice(f);
    }
    if let Some(a) = &params.audio {
        mdat_payload.extend_from_slice(&a.pcm);
    }
    let mdat_box = make_box(b"mdat", &mdat_payload)?;
    let mut out = Vec::with_capacity(ftyp_box.len() + moov_box.len() + mdat_box.len());
    out.extend_from_slice(&ftyp_box);
    out.extend_from_slice(&moov_box);
    out.extend_from_slice(&mdat_box);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_bytes(typ: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (8 + payload.len()) as u32;
        let mut out = Vec::new();
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(typ);
        out.extend_from_slice(payload);
        out
    }

    fn sample_mov() -> Vec<u8> {
        // ftyp: major qt, one compatible entry.
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"qt  ");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        ftyp.extend_from_slice(b"qt  ");
        // mvhd v0: timescale 600, duration 6000 => 10s.
        let mut mvhd = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        mvhd.extend_from_slice(&600u32.to_be_bytes());
        mvhd.extend_from_slice(&6000u32.to_be_bytes());
        // stsd with one ap4h video entry 1920x1080.
        let mut entry = Vec::new();
        entry.extend_from_slice(&86u32.to_be_bytes());
        entry.extend_from_slice(b"ap4h");
        entry.extend_from_slice(&[0u8; 6]);
        entry.extend_from_slice(&1u16.to_be_bytes());
        entry.extend_from_slice(&[0u8; 16]);
        entry.extend_from_slice(&1920u16.to_be_bytes());
        entry.extend_from_slice(&1080u16.to_be_bytes());
        entry.extend_from_slice(&[0u8; 50]);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend_from_slice(&entry);
        // stts: 240 samples, delta 25 at 600 timescale => 24 fps.
        let mut stts = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stts.extend_from_slice(&240u32.to_be_bytes());
        stts.extend_from_slice(&25u32.to_be_bytes());
        let stbl = {
            let mut b = Vec::new();
            let s = box_bytes(b"stsd", &stsd);
            b.extend_from_slice(&s);
            let t = box_bytes(b"stts", &stts);
            b.extend_from_slice(&t);
            b
        };
        let minf = box_bytes(b"minf", &box_bytes(b"stbl", &stbl));
        // mdhd v0: timescale 600, duration 6000.
        let mut mdhd = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        mdhd.extend_from_slice(&600u32.to_be_bytes());
        mdhd.extend_from_slice(&6000u32.to_be_bytes());
        // hdlr vide.
        let mut hdlr = vec![0, 0, 0, 0, 0, 0, 0, 0];
        hdlr.extend_from_slice(b"vide");
        hdlr.extend_from_slice(&[0u8; 12]);
        let mut mdia = Vec::new();
        mdia.extend_from_slice(&box_bytes(b"mdhd", &mdhd));
        mdia.extend_from_slice(&box_bytes(b"hdlr", &hdlr));
        mdia.extend_from_slice(&minf);
        // tkhd v0: version+flags(4) + 80 bytes fixed fields incl.
        // 16.16 width/height tail (84 bytes content total).
        let mut tkhd = vec![0, 0, 0, 0];
        tkhd.extend_from_slice(&[0u8; 80]);
        tkhd.extend_from_slice(&(1920u32 << 16).to_be_bytes());
        tkhd.extend_from_slice(&(1080u32 << 16).to_be_bytes());
        let mut trak = Vec::new();
        trak.extend_from_slice(&box_bytes(b"tkhd", &tkhd));
        trak.extend_from_slice(&box_bytes(b"mdia", &mdia));
        let mut moov = Vec::new();
        moov.extend_from_slice(&box_bytes(b"mvhd", &mvhd));
        moov.extend_from_slice(&box_bytes(b"trak", &trak));
        let mut file = Vec::new();
        file.extend_from_slice(&box_bytes(b"ftyp", &ftyp));
        file.extend_from_slice(&box_bytes(b"moov", &moov));
        file
    }

    #[test]
    fn sniffs_ftyp() {
        let file = sample_mov();
        assert!(is_mov_bytes(&file));
        assert!(!is_mov_bytes(b"not a video file at all...."));
    }

    #[test]
    fn parses_metadata_from_memory_file() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-mov-test-{}.mov", std::process::id()));
        std::fs::write(&path, sample_mov()).unwrap();
        let info = read_mov_info(&path).unwrap();
        assert_eq!(info.major_brand, "qt  ");
        assert!((info.duration_secs - 10.0).abs() < 0.001);
        assert_eq!(info.video_fourcc, "ap4h");
        assert_eq!((info.width, info.height), (1920, 1080));
        assert!((info.framerate - 24.0).abs() < 0.01);
        let meta = mov_to_metadata(&info, &path);
        assert_eq!(meta.video_codec, "ap4h");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_missing_moov() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-mov-bad-{}.mov", std::process::id()));
        std::fs::write(&path, b"....ftypqt  ................").unwrap();
        assert!(read_mov_info(&path).is_err());
        let _ = std::fs::remove_file(&path);
    }

    fn raw_fixture(frames: u32) -> Vec<u8> {
        let params = RawMovParams {
            width: 4,
            height: 2,
            fps: 10,
            audio: None,
        };
        let body: Vec<Vec<u8>> = (0..frames)
            .map(|f| vec![f as u8; 4 * 2 * 3])
            .collect();
        build_raw_mov(&params, &body).unwrap()
    }

    #[test]
    fn raw_writer_roundtrips() {
        let file = raw_fixture(10);
        assert!(is_mov_bytes(&file));
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-raw-{}.mov", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let info = read_mov_info(&path).unwrap();
        assert_eq!((info.width, info.height), (4, 2));
        assert_eq!(info.video_fourcc, "raw ");
        assert!((info.duration_secs - 1.0).abs() < 0.001);
        assert!((info.framerate - 10.0).abs() < 0.01);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn native_trim_selects_frames() {
        let file = raw_fixture(10);
        // 10 fps: frames 2,3,4 live in [0.2, 0.5).
        let out = trim_mov_bytes(&file, 0.2, 0.5).unwrap();
        assert!(is_mov_bytes(&out));
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-trim-{}.mov", std::process::id()));
        std::fs::write(&path, &out).unwrap();
        let info = read_mov_info(&path).unwrap();
        assert!((info.duration_secs - 0.3).abs() < 0.001);
        assert_eq!((info.width, info.height), (4, 2));
        // First output frame must equal source frame 2.
        let top = scan_children(&out).unwrap();
        let mdat = top.iter().find(|(t, _)| t == b"mdat").unwrap().1;
        let (s, _) = mdat.content_range();
        assert_eq!(&out[s..s + 24], &[2u8; 24][..]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn native_trim_rejects_empty_range() {
        let file = raw_fixture(10);
        assert!(trim_mov_bytes(&file, 5.0, 6.0).is_err());
        assert!(trim_mov_bytes(&file, 0.5, 0.5).is_err());
    }

    fn av_fixture() -> Vec<u8> {
        // 10 video frames @10fps + 1s of 8kHz mono s16le (ramp bytes).
        let params = RawMovParams {
            width: 4,
            height: 2,
            fps: 10,
            audio: Some(RawAudioParams {
                sample_rate: 8000,
                channels: 1,
                pcm: (0..16000u32).map(|i| (i % 256) as u8).collect(),
            }),
        };
        let body: Vec<Vec<u8>> = (0..10).map(|f| vec![f as u8; 4 * 2 * 3]).collect();
        build_raw_mov(&params, &body).unwrap()
    }

    #[test]
    fn av_fixture_has_two_tracks() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-av-{}.mov", std::process::id()));
        std::fs::write(&path, av_fixture()).unwrap();
        let info = read_mov_info(&path).unwrap();
        assert_eq!((info.width, info.height), (4, 2));
        assert_eq!(info.video_fourcc, "raw ");
        assert_eq!(info.audio_fourcc.as_deref(), Some("sowt"));
        assert!((info.duration_secs - 1.0).abs() < 0.001);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn native_trim_keeps_av_sync() {
        let file = av_fixture();
        // [0.2, 0.5): video frames 2,3,4 + audio samples 1600..3999.
        let out = trim_mov_bytes(&file, 0.2, 0.5).unwrap();
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-avtrim-{}.mov", std::process::id()));
        std::fs::write(&path, &out).unwrap();
        let info = read_mov_info(&path).unwrap();
        assert!((info.duration_secs - 0.3).abs() < 0.001);
        assert_eq!(info.audio_fourcc.as_deref(), Some("sowt"));
        // Audio payload must start at original sample 1600 (byte 3200).
        let top = scan_children(&out).unwrap();
        let mdat = top.iter().find(|(t, _)| t == b"mdat").unwrap().1;
        let (s, _) = mdat.content_range();
        let video_bytes = 3 * 24;
        let first_audio = &out[s + video_bytes..s + video_bytes + 8];
        let expect: Vec<u8> = (3200..3208u32).map(|i| (i % 256) as u8).collect();
        assert_eq!(first_audio, &expect[..]);
        let _ = std::fs::remove_file(&path);
    }
}
