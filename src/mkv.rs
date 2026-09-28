//! Native Matroska / WebM (`.mkv`, `.webm`) parser, pure Rust.
//!
//! EBML walk without loading `Cluster` payloads: the parser reads the
//! `EBML` header, `Info` and `Tracks`, then scans cluster timestamps
//! for a duration fallback. No ffmpeg/ffprobe dependency.
//!
//! Coverage is the metadata subset MediaKit needs: duration,
//! resolution, video/audio codec IDs and a framerate estimate from
//! `DefaultDuration`. Sample editing for EBML is a later milestone.

use crate::error::{MediaError, Result};
use crate::metadata::VideoMetadata;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Extensions handled by the native Matroska parser.
pub const MKV_EXTENSIONS: &[&str] = &["mkv", "webm"];

const MAX_TOP_ELEMENTS: usize = 512;
const MAX_INFO_SIZE: usize = 4 * 1024 * 1024;
const MAX_TRACKS_SIZE: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct MkvInfo {
    pub doctype: String,
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub framerate: f64,
    pub has_segment: bool,
}

impl MkvInfo {
    pub fn container_name(&self) -> String {
        if self.doctype.is_empty() {
            "matroska".to_string()
        } else {
            self.doctype.clone()
        }
    }
}

/// True when the extension is handled natively.
pub fn is_mkv_extension(ext: &str) -> bool {
    MKV_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
}

fn parse_error(msg: &str) -> MediaError {
    MediaError::ParseError(msg.into())
}

/// Reads an EBML ID (length marker kept) at `off`.
/// Returns `(id, id_len)`.
fn read_id(data: &[u8], off: usize) -> Result<(u32, usize)> {
    let first = *data.get(off).ok_or_else(|| parse_error("truncated id"))?;
    let len = if first & 0x80 != 0 {
        1
    } else if first & 0x40 != 0 {
        2
    } else if first & 0x20 != 0 {
        3
    } else if first & 0x10 != 0 {
        4
    } else {
        return Err(parse_error("bad ebml id"));
    };
    if off + len > data.len() {
        return Err(parse_error("truncated id"));
    }
    let mut id = 0u32;
    for b in &data[off..off + len] {
        id = (id << 8) | *b as u32;
    }
    Ok((id, len))
}

/// Reads an EBML VINT size at `off`.
/// Returns `(value, len, unknown)`.
fn read_vint(data: &[u8], off: usize) -> Result<(u64, usize, bool)> {
    let first = *data.get(off).ok_or_else(|| parse_error("truncated vint"))?;
    let len = if first & 0x80 != 0 {
        1
    } else if first & 0x40 != 0 {
        2
    } else if first & 0x20 != 0 {
        3
    } else if first & 0x10 != 0 {
        4
    } else if first & 0x08 != 0 {
        5
    } else if first & 0x04 != 0 {
        6
    } else if first & 0x02 != 0 {
        7
    } else if first & 0x01 != 0 {
        8
    } else {
        return Err(parse_error("bad vint"));
    };
    if off + len > data.len() {
        return Err(parse_error("truncated vint"));
    }
    let mask = 0xFFu64 >> len;
    let mut value = (first as u64) & mask;
    for b in &data[off + 1..off + len] {
        value = (value << 8) | *b as u64;
    }
    let max = (1u64 << (7 * len)) - 1;
    Ok((value, len, value == max))
}

/// Byte-level sniff: EBML magic `0x1A45DFA3` at offset 0.
pub fn is_mkv_bytes(buf: &[u8]) -> bool {
    buf.len() >= 4 && buf[0] == 0x1A && buf[1] == 0x45 && buf[2] == 0xDF && buf[3] == 0xA3
}

/// Sniffs the first 4 bytes of `path` for the EBML magic.
/// Returns false (not an error) for missing/unreadable files.
pub fn sniff_mkv(path: &Path) -> bool {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut head = [0u8; 4];
    match f.read_exact(&mut head) {
        Ok(()) => is_mkv_bytes(&head),
        Err(_) => false,
    }
}

fn read_uint(data: &[u8]) -> u64 {
    let mut v = 0u64;
    for b in data.iter().take(8) {
        v = (v << 8) | *b as u64;
    }
    v
}

fn read_float(data: &[u8]) -> f64 {
    match data.len() {
        4 => f32::from_be_bytes([data[0], data[1], data[2], data[3]]) as f64,
        _ if data.len() >= 8 => {
            f64::from_be_bytes([
                data[0], data[1], data[2], data[3], data[4], data[5], data[6], data[7],
            ])
        }
        _ => 0.0,
    }
}

/// Walks EBML children of `data`, calling `f(id, body)`.
/// Unknown-size elements run to the end of `data`.
fn walk_elements(data: &[u8], mut f: impl FnMut(u32, &[u8]) -> Result<()>) -> Result<()> {
    let mut off = 0usize;
    let mut count = 0usize;
    while off + 2 <= data.len() {
        count += 1;
        if count > 65536 {
            return Err(parse_error("too many elements"));
        }
        let (id, id_len) = read_id(data, off)?;
        let (size, size_len, unknown) = read_vint(data, off + id_len)?;
        let head = id_len + size_len;
        if unknown {
            f(id, &data[off + head..])?;
            break;
        }
        let size = size as usize;
        if off + head + size > data.len() {
            return Err(parse_error("element overruns parent"));
        }
        f(id, &data[off + head..off + head + size])?;
        off += head + size;
    }
    Ok(())
}

// EBML / Matroska element IDs.
const ID_EBML: u32 = 0x1A45DFA3;
const ID_DOCTYPE: u32 = 0x4282;
const ID_SEGMENT: u32 = 0x18538067;
const ID_INFO: u32 = 0x1549A966;
const ID_TIMESCALE: u32 = 0x2AD7B1;
const ID_DURATION: u32 = 0x4489;
const ID_TRACKS: u32 = 0x1654AE6B;
const ID_TRACK: u32 = 0xAE;
const ID_TRACK_NUMBER: u32 = 0xD7;
const ID_TRACK_TYPE: u32 = 0x83;
const ID_CODEC_ID: u32 = 0x86;
const ID_DEFAULT_DURATION: u32 = 0x23E383;
const ID_VIDEO: u32 = 0xE0;
const ID_PIXEL_WIDTH: u32 = 0xB0;
const ID_PIXEL_HEIGHT: u32 = 0xBA;
const ID_CLUSTER: u32 = 0x1F43B675;
const ID_CLUSTER_TIME: u32 = 0xE7;
const ID_SIMPLE_BLOCK: u32 = 0xA3;
const ID_BLOCK_GROUP: u32 = 0xA0;
const ID_BLOCK: u32 = 0xA1;

fn parse_doctype(header: &[u8]) -> String {
    let mut doctype = String::new();
    let _ = walk_elements(header, |id, body| {
        if id == ID_DOCTYPE {
            doctype = String::from_utf8_lossy(body).trim_matches('\0').to_string();
        }
        Ok(())
    });
    doctype
}

#[derive(Debug, Default)]
struct InfoData {
    timescale: u64,
    duration: f64,
    has_duration: bool,
}

fn parse_info(body: &[u8]) -> Result<InfoData> {
    let mut info = InfoData {
        timescale: 1_000_000,
        ..Default::default()
    };
    walk_elements(body, |id, el| {
        match id {
            ID_TIMESCALE => {
                let v = read_uint(el);
                if v > 0 {
                    info.timescale = v;
                }
            }
            ID_DURATION => {
                info.duration = read_float(el);
                info.has_duration = true;
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(info)
}

#[derive(Debug, Default, Clone)]
struct TrackData {
    number: u64,
    kind: u64,
    codec: String,
    width: u32,
    height: u32,
    default_duration: u64,
}

fn parse_track(body: &[u8]) -> Result<TrackData> {
    let mut track = TrackData::default();
    walk_elements(body, |id, el| {
        match id {
            ID_TRACK_NUMBER => track.number = read_uint(el),
            ID_TRACK_TYPE => track.kind = read_uint(el),
            ID_CODEC_ID => {
                track.codec = String::from_utf8_lossy(el).trim_matches('\0').to_string();
            }
            ID_DEFAULT_DURATION => track.default_duration = read_uint(el),
            ID_VIDEO => {
                let _ = walk_elements(el, |vid, vel| {
                    match vid {
                        ID_PIXEL_WIDTH => track.width = read_uint(vel) as u32,
                        ID_PIXEL_HEIGHT => track.height = read_uint(vel) as u32,
                        _ => {}
                    }
                    Ok(())
                });
            }
            _ => {}
        }
        Ok(())
    })?;
    Ok(track)
}

/// Scans cluster timestamps for a duration fallback.
/// `timescale` converts ticks to seconds; `extra` pads one frame.
fn scan_cluster_duration(data: &[u8], timescale: u64, extra: f64) -> f64 {
    let mut max_tick = 0u64;
    let _ = walk_elements(data, |id, el| {
        if id != ID_CLUSTER {
            return Ok(());
        }
        let mut base = 0u64;
        let _ = walk_elements(el, |cid, cel| {
            match cid {
                ID_CLUSTER_TIME => base = read_uint(cel),
                ID_SIMPLE_BLOCK | ID_BLOCK => {
                    if cel.len() >= 4 {
                        // Track VINT is 1 byte for tracks < 127.
                        let ts = i16::from_be_bytes([cel[1], cel[2]]) as i64;
                        if ts >= 0 {
                            max_tick = max_tick.max(base + ts as u64);
                        }
                    }
                }
                ID_BLOCK_GROUP => {
                    let _ = walk_elements(cel, |gid, gel| {
                        if gid == ID_BLOCK && gel.len() >= 4 {
                            let ts = i16::from_be_bytes([gel[1], gel[2]]) as i64;
                            if ts >= 0 {
                                max_tick = max_tick.max(base + ts as u64);
                            }
                        }
                        Ok(())
                    });
                }
                _ => {}
            }
            Ok(())
        });
        Ok(())
    });
    if timescale == 0 {
        return 0.0;
    }
    max_tick as f64 * timescale as f64 / 1_000_000_000.0 + extra
}

/// Parses a full Matroska/WebM byte buffer.
pub fn parse_mkv_bytes(data: &[u8]) -> Result<MkvInfo> {
    let mut info = MkvInfo::default();
    // Top-level scan with offsets (avoids borrowing slices out of
    // the walk closure).
    let mut elems: Vec<(u32, usize, usize)> = Vec::new();
    {
        let mut off = 0usize;
        let mut count = 0usize;
        while off + 2 <= data.len() {
            count += 1;
            if count > MAX_TOP_ELEMENTS {
                return Err(parse_error("too many top-level elements"));
            }
            let (id, id_len) = read_id(data, off)?;
            let (size, size_len, unknown) = read_vint(data, off + id_len)?;
            let head = id_len + size_len;
            let end = if unknown {
                data.len()
            } else {
                let size = size as usize;
                if off + head + size > data.len() {
                    return Err(parse_error("element overruns buffer"));
                }
                off + head + size
            };
            elems.push((id, off + head, end));
            if unknown {
                break;
            }
            off = end;
        }
    }
    let mut segment: Option<&[u8]> = None;
    for (id, start, end) in elems {
        match id {
            ID_EBML => info.doctype = parse_doctype(&data[start..end]),
            ID_SEGMENT => segment = Some(&data[start..end]),
            _ => {}
        }
    }
    let segment = segment.ok_or_else(|| parse_error("missing segment"))?;
    info.has_segment = true;
    if info.doctype != "matroska" && info.doctype != "webm" {
        return Err(parse_error("not matroska/webm"));
    }
    let mut timescale = 1_000_000u64;
    let mut info_duration: Option<f64> = None;
    let mut tracks: Vec<TrackData> = Vec::new();
    walk_elements(segment, |id, body| {
        match id {
            ID_INFO => {
                if body.len() > MAX_INFO_SIZE {
                    return Err(parse_error("info too large"));
                }
                let parsed = parse_info(body)?;
                timescale = parsed.timescale;
                if parsed.has_duration {
                    info_duration = Some(parsed.duration);
                }
            }
            ID_TRACKS => {
                if body.len() > MAX_TRACKS_SIZE {
                    return Err(parse_error("tracks too large"));
                }
                walk_elements(body, |tid, tel| {
                    if tid == ID_TRACK {
                        tracks.push(parse_track(tel)?);
                    }
                    Ok(())
                })?;
            }
            _ => {}
        }
        Ok(())
    })?;
    let video = tracks.iter().find(|t| t.kind == 1);
    let audio = tracks.iter().find(|t| t.kind == 2);
    if let Some(v) = video {
        info.width = v.width;
        info.height = v.height;
        info.video_codec = v.codec.clone();
        if v.default_duration > 0 {
            info.framerate = 1_000_000_000.0 / v.default_duration as f64;
        }
    }
    if let Some(a) = audio {
        info.audio_codec = Some(a.codec.clone());
    }
    info.duration_secs = match info_duration {
        Some(d) if d > 0.0 => d * timescale as f64 / 1_000_000_000.0,
        _ => {
            let extra = if info.framerate > 0.0 {
                1.0 / info.framerate
            } else {
                0.0
            };
            scan_cluster_duration(segment, timescale, extra)
        }
    };
    Ok(info)
}

/// Reads `.mkv`/`.webm` metadata without loading clusters.
pub fn read_mkv_info(path: &Path) -> Result<MkvInfo> {
    let meta = std::fs::metadata(path).map_err(MediaError::from_io)?;
    if meta.len() > crate::mov::MAX_MOV_FILE_SIZE as u64 {
        return Err(parse_error("file too large"));
    }
    // Clusters are usually small in test files; stream the head and
    // rely on Info timestamps, falling back to a bounded cluster scan.
    let f = File::open(path).map_err(MediaError::from_io)?;
    let take = meta.len().min(64 * 1024 * 1024);
    let mut head = Vec::new();
    f.take(take)
        .read_to_end(&mut head)
        .map_err(MediaError::from_io)?;
    // If the file was truncated by the head window, parsing still
    // succeeds when Info/Tracks fit; cluster fallback degrades.
    parse_mkv_bytes(&head).or_else(|_| {
        let full = std::fs::read(path).map_err(MediaError::from_io)?;
        parse_mkv_bytes(&full)
    })
}

/// Converts native `MkvInfo` into the shared `VideoMetadata` shape.
pub fn mkv_to_metadata(info: &MkvInfo, path: &Path) -> VideoMetadata {
    let size_bytes = std::fs::metadata(path).ok().map(|m| m.len());
    VideoMetadata {
        duration_secs: info.duration_secs,
        width: info.width,
        height: info.height,
        video_codec: if info.video_codec.is_empty() {
            info.container_name()
        } else {
            info.video_codec.clone()
        },
        audio_codec: info.audio_codec.clone(),
        framerate: info.framerate,
        container: info.container_name(),
        size_bytes,
    }
}

/// Native metadata for `.mkv`/`.webm` without ffprobe.
pub fn read_mkv_metadata(path: &Path) -> Result<VideoMetadata> {
    let info = read_mkv_info(path)?;
    Ok(mkv_to_metadata(&info, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- fixture writer (EBML) ----

    fn vint(value: u64) -> Vec<u8> {
        for len in 1..=8usize {
            if value < (1u64 << (7 * len)) {
                let mut out = vec![0u8; len];
                let mut v = value;
                for b in out.iter_mut().rev() {
                    *b = (v & 0xFF) as u8;
                    v >>= 8;
                }
                out[0] |= 0x80 >> (len - 1);
                return out;
            }
        }
        unreachable!()
    }

    fn id_bytes(id: u32) -> Vec<u8> {
        let raw = id.to_be_bytes();
        let len = if id & 0xFF000000 != 0 {
            4
        } else if id & 0x00FF0000 != 0 {
            3
        } else if id & 0x0000FF00 != 0 {
            2
        } else {
            1
        };
        raw[4 - len..].to_vec()
    }

    fn el(id: u32, payload: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.extend_from_slice(&vint(payload.len() as u64));
        out.extend_from_slice(payload);
        out
    }

    fn uint_el(id: u32, v: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut tmp = v;
        let mut raw = Vec::new();
        if tmp == 0 {
            raw.push(0);
        }
        while tmp > 0 {
            raw.push((tmp & 0xFF) as u8);
            tmp >>= 8;
        }
        raw.reverse();
        bytes.extend_from_slice(&raw);
        el(id, &bytes)
    }

    fn str_el(id: u32, s: &str) -> Vec<u8> {
        el(id, s.as_bytes())
    }

    fn float_el(id: u32, v: f64) -> Vec<u8> {
        el(id, &v.to_be_bytes())
    }

    fn sample_mkv() -> Vec<u8> {
        sample_matroska("matroska", "V_MPEG4/ISO/AVC", "A_AAC")
    }

    fn sample_webm() -> Vec<u8> {
        sample_matroska("webm", "V_VP9", "A_OPUS")
    }

    fn sample_matroska(doctype: &str, vcodec: &str, acodec: &str) -> Vec<u8> {
        let ebml = el(ID_EBML, &[str_el(ID_DOCTYPE, doctype)].concat());
        let info = el(
            ID_INFO,
            &[
                uint_el(ID_TIMESCALE, 1_000_000),
                float_el(ID_DURATION, 6000.0),
            ]
            .concat(),
        );
        let video_track = el(
            ID_TRACK,
            &[
                uint_el(ID_TRACK_NUMBER, 1),
                uint_el(ID_TRACK_TYPE, 1),
                str_el(ID_CODEC_ID, vcodec),
                uint_el(ID_DEFAULT_DURATION, 41_666_666),
                el(
                    ID_VIDEO,
                    &[uint_el(ID_PIXEL_WIDTH, 1280), uint_el(ID_PIXEL_HEIGHT, 720)]
                        .concat(),
                ),
            ]
            .concat(),
        );
        let audio_track = el(
            ID_TRACK,
            &[
                uint_el(ID_TRACK_NUMBER, 2),
                uint_el(ID_TRACK_TYPE, 2),
                str_el(ID_CODEC_ID, acodec),
            ]
            .concat(),
        );
        let tracks = el(ID_TRACKS, &[video_track, audio_track].concat());
        // One cluster at ts 0 with a single SimpleBlock (track 1).
        let mut block = vec![0x81, 0x00, 0x00, 0x00]; // track 1, ts 0, flags 0
        block.extend_from_slice(&[0u8; 16]);
        let cluster = el(
            ID_CLUSTER,
            &[uint_el(ID_CLUSTER_TIME, 0), el(ID_SIMPLE_BLOCK, &block)].concat(),
        );
        let segment_body = [info, tracks, cluster].concat();
        let mut out = ebml;
        out.extend_from_slice(&id_bytes(ID_SEGMENT));
        out.extend_from_slice(&[0xFF]); // unknown size
        out.extend_from_slice(&segment_body);
        out
    }

    #[test]
    fn vint_roundtrip() {
        // 126 is the max 1-byte data value; 0xFF means unknown size.
        for v in [0u64, 1, 126, 128, 1000, 1 << 20, (1 << 28) - 2] {
            let enc = vint(v);
            let (back, len, unknown) = read_vint(&enc, 0).unwrap();
            assert_eq!((back, len, unknown), (v, enc.len(), false));
        }
        let (_, _, unknown) = read_vint(&[0xFF], 0).unwrap();
        assert!(unknown);
    }

    #[test]
    fn sniffs_ebml() {
        assert!(is_mkv_bytes(&sample_mkv()));
        assert!(!is_mkv_bytes(b"not ebml...."));
    }

    #[test]
    fn parses_metadata() {
        let file = sample_mkv();
        let info = parse_mkv_bytes(&file).unwrap();
        assert_eq!(info.doctype, "matroska");
        assert!((info.duration_secs - 6.0).abs() < 0.01);
        assert_eq!((info.width, info.height), (1280, 720));
        assert_eq!(info.video_codec, "V_MPEG4/ISO/AVC");
        assert_eq!(info.audio_codec.as_deref(), Some("A_AAC"));
        assert!((info.framerate - 24.0).abs() < 0.01);
    }

    #[test]
    fn cluster_fallback_duration() {
        // Same file without the Info duration: falls back to clusters.
        let mut file = sample_mkv();
        // Blank the duration float payload (8 bytes) to zeros -> has_duration
        // stays true but d == 0.0, which triggers the fallback path.
        let needle = 6000.0f64.to_be_bytes();
        let pos = file
            .windows(8)
            .position(|w| w == needle)
            .expect("duration float present");
        file[pos..pos + 8].copy_from_slice(&0.0f64.to_be_bytes());
        let info = parse_mkv_bytes(&file).unwrap();
        // One cluster at ts 0 + one frame of padding (~1/24s).
        assert!(info.duration_secs > 0.0 && info.duration_secs < 1.0);
    }

    #[test]
    fn parses_webm_doctype() {
        let file = sample_webm();
        assert!(is_mkv_bytes(&file));
        let info = parse_mkv_bytes(&file).unwrap();
        assert_eq!(info.doctype, "webm");
        assert_eq!(info.container_name(), "webm");
        assert!((info.duration_secs - 6.0).abs() < 0.01);
        assert_eq!((info.width, info.height), (1280, 720));
        assert_eq!(info.video_codec, "V_VP9");
        assert_eq!(info.audio_codec.as_deref(), Some("A_OPUS"));
        assert!(is_mkv_extension("webm"));
    }

    #[test]
    fn rejects_non_matroska() {
        let ebml = el(ID_EBML, &str_el(ID_DOCTYPE, "webX"));
        let mut file = ebml;
        file.extend_from_slice(&id_bytes(ID_SEGMENT));
        file.extend_from_slice(&[0xFF]);
        assert!(parse_mkv_bytes(&file).is_err());
    }
}
