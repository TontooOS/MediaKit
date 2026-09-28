//! Native AVI (RIFF) parser, pure Rust.
//!
//! Walks `RIFF 'AVI '` top-level chunks: `hdrl` (avih + one `strl`
//! per stream) is buffered and parsed, `movi`/`idx1` payloads are
//! skipped via seeking so large files never load fully. No
//! ffmpeg/ffprobe dependency.
//!
//! Coverage is the metadata subset MediaKit needs 1:1: duration,
//! resolution, video fourcc, audio format and framerate. v1 scope is
//! classic AVI 1.0; OpenDML (`AVIX`, `dmlh` 64-bit frame counts)
//! returns `ParseError` until multi-RIFF spanning lands.

use crate::error::{MediaError, Result};
use crate::metadata::VideoMetadata;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions handled by the native AVI parser.
pub const AVI_EXTENSIONS: &[&str] = &["avi"];

const MAX_CHUNKS: usize = 4096;
const MAX_HDRL_SIZE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct AviInfo {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub video_fourcc: String,
    pub audio_codec: Option<String>,
    pub framerate: f64,
    pub stream_count: u32,
    pub has_movi: bool,
}

impl AviInfo {
    pub fn container_name(&self) -> String {
        "avi".to_string()
    }
}

/// True when the extension is handled natively.
pub fn is_avi_extension(ext: &str) -> bool {
    AVI_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

fn parse_error(msg: &str) -> MediaError {
    MediaError::ParseError(msg.into())
}

fn le_u16(data: &[u8], off: usize) -> Result<u16> {
    data.get(off..off + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| parse_error("truncated chunk"))
}

fn le_u32(data: &[u8], off: usize) -> Result<u32> {
    data.get(off..off + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| parse_error("truncated chunk"))
}

fn le_i32(data: &[u8], off: usize) -> Result<i32> {
    data.get(off..off + 4)
        .map(|s| i32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| parse_error("truncated chunk"))
}

fn fourcc_le(data: &[u8], off: usize) -> Result<String> {
    let bytes = data
        .get(off..off + 4)
        .ok_or_else(|| parse_error("truncated fourcc"))?;
    Ok(bytes
        .iter()
        .map(|&b| {
            if b.is_ascii_graphic() || b == b' ' {
                b as char
            } else {
                '?'
            }
        })
        .collect())
}

/// Byte-level sniff: `RIFF....AVI ` magic.
pub fn is_avi_bytes(buf: &[u8]) -> bool {
    buf.len() >= 12 && &buf[0..4] == b"RIFF" && &buf[8..12] == b"AVI "
}

/// Sniffs the first 12 bytes of `path` for the RIFF/AVI magic.
/// Returns false (not an error) for missing/unreadable files.
pub fn sniff_avi(path: &Path) -> bool {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut head = [0u8; 12];
    match f.read_exact(&mut head) {
        Ok(()) => is_avi_bytes(&head),
        Err(_) => false,
    }
}

#[derive(Debug, Clone, Copy)]
struct Chunk {
    id: [u8; 4],
    start: usize,
    end: usize,
}

/// Walks RIFF chunks in `data` (word-aligned, padding byte after
/// odd sizes). Calls `f(id, body)` for each complete chunk.
fn walk_chunks(data: &[u8], mut f: impl FnMut([u8; 4], &[u8]) -> Result<()>) -> Result<()> {
    let mut off = 0usize;
    let mut count = 0usize;
    while off + 8 <= data.len() {
        count += 1;
        if count > MAX_CHUNKS {
            return Err(parse_error("too many chunks"));
        }
        let id = [data[off], data[off + 1], data[off + 2], data[off + 3]];
        let size = le_u32(data, off + 4)? as usize;
        if off + 8 + size > data.len() {
            return Err(parse_error("chunk overruns parent"));
        }
        f(id, &data[off + 8..off + 8 + size])?;
        off += 8 + size + (size & 1);
    }
    Ok(())
}

/// Collects chunk spans (for handlers that need positions).
fn collect_chunks(data: &[u8]) -> Result<Vec<(Chunk, bool)>> {
    // Returns (chunk, is_list).
    let mut out = Vec::new();
    let mut off = 0usize;
    let mut count = 0usize;
    while off + 8 <= data.len() {
        count += 1;
        if count > MAX_CHUNKS {
            return Err(parse_error("too many chunks"));
        }
        let id = [data[off], data[off + 1], data[off + 2], data[off + 3]];
        let size = le_u32(data, off + 4)? as usize;
        if off + 8 + size > data.len() {
            return Err(parse_error("chunk overruns parent"));
        }
        out.push((
            Chunk {
                id,
                start: off + 8,
                end: off + 8 + size,
            },
            &id == b"LIST",
        ));
        off += 8 + size + (size & 1);
    }
    Ok(out)
}

fn list_type(data: &[u8], chunk: &Chunk) -> Result<[u8; 4]> {
    let t = data
        .get(chunk.start..chunk.start + 4)
        .ok_or_else(|| parse_error("truncated LIST"))?;
    Ok([t[0], t[1], t[2], t[3]])
}

fn list_body<'a>(data: &'a [u8], chunk: &Chunk) -> Result<&'a [u8]> {
    data.get(chunk.start + 4..chunk.end)
        .ok_or_else(|| parse_error("truncated LIST"))
}

/// Maps WAVE format tags to short codec names.
fn audio_tag_name(tag: u16) -> String {
    match tag {
        0x0001 => "PCM".to_string(),
        0x0003 => "FLOAT".to_string(),
        0x0006 => "ALAW".to_string(),
        0x0007 => "MULAW".to_string(),
        0x0055 => "MP3".to_string(),
        0x00FF => "AAC".to_string(),
        0x0161 => "WMA".to_string(),
        0x1622 => "OPUS".to_string(),
        _ => format!("0x{tag:04X}"),
    }
}

#[derive(Debug, Default)]
struct StreamInfo {
    kind: String,
    handler: String,
    rate: u32,
    scale: u32,
    length: u32,
    width: i32,
    height: i32,
    audio_tag: Option<u16>,
}

fn parse_strh(body: &[u8], stream: &mut StreamInfo) -> Result<()> {
    if body.len() < 36 {
        return Err(parse_error("truncated strh"));
    }
    stream.kind = fourcc_le(body, 0)?;
    stream.handler = fourcc_le(body, 4)?;
    stream.scale = le_u32(body, 20)?;
    stream.rate = le_u32(body, 24)?;
    stream.length = le_u32(body, 32)?;
    Ok(())
}

fn parse_strf(kind: &str, body: &[u8], stream: &mut StreamInfo) -> Result<()> {
    if kind == "vids" {
        if body.len() < 40 {
            return Err(parse_error("truncated BITMAPINFOHEADER"));
        }
        stream.width = le_i32(body, 4)?;
        stream.height = le_i32(body, 8)?;
        let fourcc = fourcc_le(body, 16)?;
        if fourcc.trim() != "????" {
            stream.handler = fourcc;
        }
    } else if kind == "auds" {
        if body.len() < 2 {
            return Err(parse_error("truncated WAVEFORMATEX"));
        }
        stream.audio_tag = Some(le_u16(body, 0)?);
    }
    Ok(())
}

fn parse_strl(body: &[u8]) -> Result<StreamInfo> {
    let mut stream = StreamInfo::default();
    walk_chunks(body, |id, chunk| {
        match &id {
            b"strh" => parse_strh(chunk, &mut stream)?,
            b"strf" => parse_strf(&stream.kind.clone(), chunk, &mut stream)?,
            _ => {}
        }
        Ok(())
    })?;
    Ok(stream)
}

fn parse_avih(body: &[u8], info: &mut AviInfo) -> Result<(u32, u32)> {
    if body.len() < 40 {
        return Err(parse_error("truncated avih"));
    }
    let micro_sec = le_u32(body, 0)?;
    let total_frames = le_u32(body, 16)?;
    info.stream_count = le_u32(body, 24)?;
    info.width = le_u32(body, 32)?;
    info.height = le_u32(body, 36)?;
    if micro_sec > 0 {
        info.framerate = 1_000_000.0 / micro_sec as f64;
        info.duration_secs = total_frames as f64 * micro_sec as f64 / 1_000_000.0;
    }
    Ok((micro_sec, total_frames))
}

/// Parses a full AVI byte buffer (RIFF form `AVI `).
pub fn parse_avi_bytes(data: &[u8]) -> Result<AviInfo> {
    if !is_avi_bytes(data) {
        return Err(parse_error("not riff/avi"));
    }
    if data.len() >= 16 && &data[12..16] == b"AVIX" {
        // OpenDML extended form handled at the top level only in v1.
    }
    let mut info = AviInfo::default();
    // Top-level: RIFF header (12 bytes) then chunks.
    let top = &data[12..];
    let chunks = collect_chunks(top)?;
    let mut avih_seen = false;
    let mut streams: Vec<StreamInfo> = Vec::new();
    for (chunk, is_list) in &chunks {
        let body = top
            .get(chunk.start..chunk.end)
            .ok_or_else(|| parse_error("chunk out of range"))?;
        if *is_list {
            let ltype = list_type(top, chunk)?;
            let inner = list_body(top, chunk)?;
            match &ltype {
                b"hdrl" => {
                    walk_chunks(inner, |id, chunk| {
                        match &id {
                            b"avih" => {
                                parse_avih(chunk, &mut info)?;
                                avih_seen = true;
                            }
                            _ => {}
                        }
                        Ok(())
                    })?;
                    // Second pass for strl lists (order-independent).
                    for (s, _) in collect_chunks(inner)? {
                        if &s.id == b"LIST"
                            && list_type(inner, &s)? == *b"strl"
                            && list_body(inner, &s).map(|b| b.len()).unwrap_or(0) > 0
                        {
                            streams.push(parse_strl(list_body(inner, &s)?)?);
                        }
                    }
                }
                b"movi" => info.has_movi = true,
                b"AVIX" => return Err(parse_error("opendml spanned files unsupported")),
                _ => {}
            }
        } else if &chunk.id == b"avih" {
            parse_avih(body, &mut info)?;
            avih_seen = true;
        }
    }
    if !avih_seen {
        return Err(parse_error("missing avih"));
    }
    if let Some(v) = streams.iter().find(|s| s.kind == "vids") {
        if !v.handler.trim().is_empty() && v.handler != "????" {
            info.video_fourcc = v.handler.trim().to_string();
        }
        if v.scale > 0 && v.rate > 0 {
            info.framerate = v.rate as f64 / v.scale as f64;
        }
        if v.width > 0 {
            info.width = v.width as u32;
        }
        if v.height != 0 {
            info.height = v.height.unsigned_abs();
        }
        if info.duration_secs == 0.0 && v.length > 0 && info.framerate > 0.0 {
            info.duration_secs = v.length as f64 / info.framerate;
        }
    }
    if let Some(a) = streams.iter().find(|s| s.kind == "auds") {
        info.audio_codec = Some(match a.audio_tag {
            Some(tag) => audio_tag_name(tag),
            None if !a.handler.trim().is_empty() => a.handler.trim().to_string(),
            None => "audio".to_string(),
        });
    }
    Ok(info)
}

/// Reads `.avi` metadata, buffering `hdrl` and skipping `movi`.
pub fn read_avi_info(path: &Path) -> Result<AviInfo> {
    let meta = std::fs::metadata(path).map_err(MediaError::from_io)?;
    if meta.len() > crate::mov::MAX_MOV_FILE_SIZE as u64 {
        return Err(parse_error("file too large"));
    }
    let mut f = File::open(path).map_err(MediaError::from_io)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).map_err(MediaError::from_io)?;
    if !is_avi_bytes(&head) {
        return Err(parse_error("not riff/avi"));
    }
    // Walk top-level chunks with seeking; buffer hdrl only.
    let file_len = meta.len();
    let mut pos = 12u64;
    let mut hdrl_buf: Option<Vec<u8>> = None;
    let mut seen_movi = false;
    let mut count = 0usize;
    let mut header = [0u8; 8];
    while pos + 8 <= file_len {
        count += 1;
        if count > MAX_CHUNKS {
            return Err(parse_error("too many chunks"));
        }
        f.seek(SeekFrom::Start(pos)).map_err(MediaError::from_io)?;
        f.read_exact(&mut header).map_err(MediaError::from_io)?;
        let id = [header[0], header[1], header[2], header[3]];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
        let total = 8u64
            .checked_add(size)
            .and_then(|v| v.checked_add(size & 1))
            .ok_or_else(|| parse_error("chunk size overflow"))?;
        if &id == b"LIST" {
            let mut ltype = [0u8; 4];
            f.read_exact(&mut ltype).map_err(MediaError::from_io)?;
            if &ltype == b"hdrl" {
                if size < 4 || size as usize > MAX_HDRL_SIZE {
                    return Err(parse_error("hdrl size insane"));
                }
                let mut buf = vec![0u8; (size - 4) as usize];
                f.read_exact(&mut buf).map_err(MediaError::from_io)?;
                hdrl_buf = Some(buf);
            }
            if &ltype == b"movi" {
                seen_movi = true;
            }
            if &ltype == b"AVIX" {
                return Err(parse_error("opendml spanned files unsupported"));
            }
        }
        pos += total;
        if pos >= file_len {
            break;
        }
    }
    let hdrl = hdrl_buf.ok_or_else(|| parse_error("missing hdrl"))?;
    // Reassemble a minimal RIFF image for the shared parser.
    let mut image = Vec::with_capacity(12 + 8 + 4 + hdrl.len());
    image.extend_from_slice(&head);
    image.extend_from_slice(b"LIST");
    image.extend_from_slice(&((hdrl.len() + 4) as u32).to_le_bytes());
    image.extend_from_slice(b"hdrl");
    image.extend_from_slice(&hdrl);
    let mut info = parse_avi_bytes(&image)?;
    info.has_movi = seen_movi;
    Ok(info)
}

/// One `movi` chunk: stream number (`00` in `00dc`), kind
/// (`dc` video, `wb` audio, `db` RGB) and file location.
/// Backs native thumbnails without ffmpeg.
#[derive(Debug, Clone)]
pub struct AviChunk {
    pub stream: u16,
    pub kind: String,
    pub offset: u64,
    pub size: u32,
}

/// Lists `movi` chunks (bounded, seek-based, payloads untouched).
/// Stops after `cap` chunks; OpenDML `AVIX` lists are skipped in v1.
pub fn read_avi_chunks(path: &Path, cap: usize) -> Result<Vec<AviChunk>> {
    let meta = std::fs::metadata(path).map_err(MediaError::from_io)?;
    let file_len = meta.len();
    let mut f = File::open(path).map_err(MediaError::from_io)?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).map_err(MediaError::from_io)?;
    if !is_avi_bytes(&head) {
        return Err(parse_error("not riff/avi"));
    }
    let cap = cap.min(1_000_000).max(1);
    let mut out = Vec::new();
    let mut pos = 12u64;
    let mut header = [0u8; 8];
    let mut top_count = 0usize;
    while pos + 8 <= file_len && out.len() < cap {
        top_count += 1;
        if top_count > MAX_CHUNKS {
            break;
        }
        f.seek(SeekFrom::Start(pos)).map_err(MediaError::from_io)?;
        f.read_exact(&mut header).map_err(MediaError::from_io)?;
        let id = [header[0], header[1], header[2], header[3]];
        let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
        let total = 8u64
            .checked_add(size)
            .and_then(|v| v.checked_add(size & 1))
            .ok_or_else(|| parse_error("chunk size overflow"))?;
        if &id == b"LIST" {
            let mut ltype = [0u8; 4];
            f.read_exact(&mut ltype).map_err(MediaError::from_io)?;
            if &ltype == b"movi" && size >= 4 {
                let mut inner = pos + 12;
                let end = pos + 8 + size;
                while inner + 8 <= end && inner + 8 <= file_len && out.len() < cap {
                    f.seek(SeekFrom::Start(inner)).map_err(MediaError::from_io)?;
                    f.read_exact(&mut header).map_err(MediaError::from_io)?;
                    let cid = [header[0], header[1], header[2], header[3]];
                    let csize =
                        u32::from_le_bytes([header[4], header[5], header[6], header[7]]) as u64;
                    let ctotal = 8u64
                        .checked_add(csize)
                        .and_then(|v| v.checked_add(csize & 1))
                        .ok_or_else(|| parse_error("chunk size overflow"))?;
                    if inner + ctotal > file_len + 1 {
                        break;
                    }
                    let tag = String::from_utf8_lossy(&cid).to_string();
                    let stream = tag
                        .get(0..2)
                        .and_then(|s| s.parse::<u16>().ok())
                        .unwrap_or(u16::MAX);
                    let kind = tag.get(2..4).unwrap_or("??").to_string();
                    if stream != u16::MAX {
                        out.push(AviChunk {
                            stream,
                            kind,
                            offset: inner + 8,
                            size: csize.min(u32::MAX as u64) as u32,
                        });
                    }
                    inner += ctotal;
                }
            }
        }
        pos += total;
        if pos >= file_len {
            break;
        }
    }
    Ok(out)
}

/// Converts native `AviInfo` into the shared `VideoMetadata` shape.
pub fn avi_to_metadata(info: &AviInfo, path: &Path) -> VideoMetadata {
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
        audio_codec: info.audio_codec.clone(),
        framerate: info.framerate,
        container: info.container_name(),
        size_bytes,
    }
}

/// Native metadata for `.avi` without ffprobe.
pub fn read_avi_metadata(path: &Path) -> Result<VideoMetadata> {
    let info = read_avi_info(path)?;
    Ok(avi_to_metadata(&info, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() & 1 == 1 {
            out.push(0);
        }
        out
    }

    fn list(ltype: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut inner = Vec::new();
        inner.extend_from_slice(ltype);
        inner.extend_from_slice(payload);
        chunk(b"LIST", &inner)
    }

    fn sample_avi() -> Vec<u8> {
        // avih: 10us/frame? No: 100000us/frame = 10fps, 30 frames,
        // 1 stream? Use 2 streams, 320x240.
        let mut avih = vec![0u8; 56];
        avih[0..4].copy_from_slice(&100_000u32.to_le_bytes());
        avih[16..20].copy_from_slice(&30u32.to_le_bytes());
        avih[24..28].copy_from_slice(&2u32.to_le_bytes());
        avih[32..36].copy_from_slice(&320u32.to_le_bytes());
        avih[36..40].copy_from_slice(&240u32.to_le_bytes());
        // strh vids MJPG, rate 10 / scale 1, length 30.
        let mut strh_v = vec![0u8; 56];
        strh_v[0..4].copy_from_slice(b"vids");
        strh_v[4..8].copy_from_slice(b"MJPG");
        strh_v[20..24].copy_from_slice(&1u32.to_le_bytes());
        strh_v[24..28].copy_from_slice(&10u32.to_le_bytes());
        strh_v[32..36].copy_from_slice(&30u32.to_le_bytes());
        // strf BITMAPINFOHEADER 320x240 MJPG.
        let mut strf_v = vec![0u8; 40];
        strf_v[0..4].copy_from_slice(&40u32.to_le_bytes());
        strf_v[4..8].copy_from_slice(&320i32.to_le_bytes());
        strf_v[8..12].copy_from_slice(&240i32.to_le_bytes());
        strf_v[12..14].copy_from_slice(&1u16.to_le_bytes());
        strf_v[14..16].copy_from_slice(&24u16.to_le_bytes());
        strf_v[16..20].copy_from_slice(b"MJPG");
        // strh auds, PCM; strf WAVEFORMATEX tag 1.
        let mut strh_a = vec![0u8; 56];
        strh_a[0..4].copy_from_slice(b"auds");
        strh_a[20..24].copy_from_slice(&1u32.to_le_bytes());
        strh_a[24..28].copy_from_slice(&8000u32.to_le_bytes());
        let mut strf_a = vec![0u8; 18];
        strf_a[0..2].copy_from_slice(&1u16.to_le_bytes());
        let strl_v = list(b"strl", &[chunk(b"strh", &strh_v), chunk(b"strf", &strf_v)].concat());
        let strl_a = list(b"strl", &[chunk(b"strh", &strh_a), chunk(b"strf", &strf_a)].concat());
        // Odd-sized JUNK to exercise padding.
        let junk = chunk(b"JUNK", &[0xABu8; 7]);
        let hdrl = list(
            b"hdrl",
            &[chunk(b"avih", &avih), junk, strl_v, strl_a].concat(),
        );
        // Minimal movi with one video + one audio chunk.
        let movi = list(
            b"movi",
            &[chunk(b"00dc", &[0u8; 8]), chunk(b"01wb", &[0u8; 4])].concat(),
        );
        let riff_body: Vec<u8> = [b"AVI ".as_slice(), &hdrl, &movi].concat();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(riff_body.len() as u32).to_le_bytes());
        out.extend_from_slice(&riff_body);
        out
    }

    #[test]
    fn sniffs_riff() {
        assert!(is_avi_bytes(&sample_avi()));
        assert!(!is_avi_bytes(b"RIFF....WEBP...."));
        assert!(!is_avi_bytes(b"short"));
    }

    #[test]
    fn parses_metadata() {
        let file = sample_avi();
        let info = parse_avi_bytes(&file).unwrap();
        assert!((info.duration_secs - 3.0).abs() < 0.001);
        assert_eq!((info.width, info.height), (320, 240));
        assert_eq!(info.video_fourcc, "MJPG");
        assert_eq!(info.audio_codec.as_deref(), Some("PCM"));
        assert!((info.framerate - 10.0).abs() < 0.001);
        assert!(info.has_movi);
    }

    #[test]
    fn reads_back_from_disk() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-avi-{}.avi", std::process::id()));
        std::fs::write(&path, sample_avi()).unwrap();
        assert!(sniff_avi(&path));
        let info = read_avi_info(&path).unwrap();
        assert_eq!(info.video_fourcc, "MJPG");
        assert!((info.duration_secs - 3.0).abs() < 0.001);
        let meta = avi_to_metadata(&info, &path);
        assert_eq!(meta.video_codec, "MJPG");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn rejects_non_avi() {
        assert!(parse_avi_bytes(b"RIFF\x08\x00\x00\x00WEBPVP8 data....").is_err());
        assert!(parse_avi_bytes(b"too short").is_err());
    }

    #[test]
    fn lists_movi_chunks() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-avichunk-{}.avi", std::process::id()));
        std::fs::write(&path, sample_avi()).unwrap();
        let chunks = read_avi_chunks(&path, 64).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].stream, 0);
        assert_eq!(chunks[0].kind, "dc");
        assert_eq!(chunks[1].stream, 1);
        assert_eq!(chunks[1].kind, "wb");
        assert!(chunks[0].size == 8 && chunks[1].size == 4);
        let _ = std::fs::remove_file(&path);
    }
}
