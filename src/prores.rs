//! Native ProRes support (SMPTE RDD 36 subset), pure Rust.
//!
//! Covers what MediaKit needs 1:1 for `.mov` today: fourcc detect,
//! profile mapping, `icpf` frame-header validate and a GPU slice plan
//! (tile ranges for a future WGPU compute decoder). Coefficient
//! bitstream decode is intentionally out of scope for this step and
//! stays on the ffmpeg path; the header API is stable so the GPU
//! decoder can land without API churn.

use crate::error::{MediaError, Result};
use crate::mov::MovInfo;

/// ProRes sample-entry fourccs (SMPTE RDD 36).
pub const PRORES_FOURCCS: &[&str] = &["apco", "apcs", "apcn", "apch", "ap4h", "ap4x"];

/// Frame identifier that opens every ProRes frame.
pub const PRORES_FRAME_ID: [u8; 4] = *b"icpf";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProResProfile {
    Proxy,
    Lt,
    Standard,
    Hq,
    FourFourFourFour,
    FourFourFourFourXq,
    #[default]
    Unknown,
}

impl ProResProfile {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProResProfile::Proxy => "apco",
            ProResProfile::Lt => "apcs",
            ProResProfile::Standard => "apcn",
            ProResProfile::Hq => "apch",
            ProResProfile::FourFourFourFour => "ap4h",
            ProResProfile::FourFourFourFourXq => "ap4x",
            ProResProfile::Unknown => "unknown",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            ProResProfile::Proxy => "ProRes 422 Proxy",
            ProResProfile::Lt => "ProRes 422 LT",
            ProResProfile::Standard => "ProRes 422",
            ProResProfile::Hq => "ProRes 422 HQ",
            ProResProfile::FourFourFourFour => "ProRes 4444",
            ProResProfile::FourFourFourFourXq => "ProRes 4444 XQ",
            ProResProfile::Unknown => "Unknown",
        }
    }

    pub fn has_alpha(&self) -> bool {
        matches!(
            self,
            ProResProfile::FourFourFourFour | ProResProfile::FourFourFourFourXq
        )
    }

    pub fn chroma(&self) -> ProResChroma {
        match self {
            ProResProfile::FourFourFourFour | ProResProfile::FourFourFourFourXq => {
                ProResChroma::FourFourFour
            }
            _ => ProResChroma::FourTwoTwo,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProResChroma {
    #[default]
    FourTwoTwo,
    FourFourFour,
}

impl ProResChroma {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProResChroma::FourTwoTwo => "4:2:2",
            ProResChroma::FourFourFour => "4:4:4",
        }
    }
}

/// Maps a sample-entry fourcc to a profile. Case-insensitive.
pub fn profile_from_fourcc(fourcc: &str) -> ProResProfile {
    match fourcc.to_ascii_lowercase().as_str() {
        "apco" => ProResProfile::Proxy,
        "apcs" => ProResProfile::Lt,
        "apcn" => ProResProfile::Standard,
        "apch" => ProResProfile::Hq,
        "ap4h" => ProResProfile::FourFourFourFour,
        "ap4x" => ProResProfile::FourFourFourFourXq,
        _ => ProResProfile::Unknown,
    }
}

/// True for any ProRes sample-entry fourcc.
pub fn is_prores_fourcc(fourcc: &str) -> bool {
    profile_from_fourcc(fourcc) != ProResProfile::Unknown
}

/// Profile of the video track in `info`, if it is ProRes.
pub fn detect_profile_from_mov(info: &MovInfo) -> Option<ProResProfile> {
    let profile = profile_from_fourcc(&info.video_fourcc);
    if profile == ProResProfile::Unknown {
        None
    } else {
        Some(profile)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProResFrameHeader {
    pub frame_size: u32,
    pub header_size: u16,
    pub version: u16,
    pub creator: String,
    pub width: u16,
    pub height: u16,
    pub chroma: ProResChroma,
    pub alpha: bool,
    pub slice_count_hint: u16,
}

fn be_u16_at(buf: &[u8], off: usize) -> Result<u16> {
    buf.get(off..off + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| MediaError::ParseError("truncated prores header".into()))
}

fn be_u32_at(buf: &[u8], off: usize) -> Result<u32> {
    buf.get(off..off + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| MediaError::ParseError("truncated prores header".into()))
}

/// Validates and parses an `icpf` frame header.
///
/// Layout (SMPTE RDD 36 subset):
/// `frame_size u32 | "icpf" | header_size u16 | version u16 |
/// creator[4] | width u16 | height u16 | flags u8 (chroma+alpha) |
/// reserved + slice_count_hint u16`.
///
/// Returns `Err(ParseError)` on bad magic, version, dimensions or
/// truncated input.
pub fn parse_frame_header(buf: &[u8]) -> Result<ProResFrameHeader> {
    if buf.len() < 24 {
        return Err(MediaError::ParseError("truncated prores frame".into()));
    }
    let frame_size = be_u32_at(buf, 0)?;
    if frame_size < 24 || frame_size as usize > buf.len() + 1024 * 1024 * 1024 {
        return Err(MediaError::ParseError("bad prores frame size".into()));
    }
    if buf[4..8] != PRORES_FRAME_ID {
        return Err(MediaError::ParseError("missing icpf identifier".into()));
    }
    let header_size = be_u16_at(buf, 8)?;
    let version = be_u16_at(buf, 10)?;
    if version > 1 {
        return Err(MediaError::ParseError("unsupported prores version".into()));
    }
    if header_size < 20 || header_size as usize > buf.len() {
        return Err(MediaError::ParseError("bad prores header size".into()));
    }
    let creator = String::from_utf8_lossy(&buf[12..16]).trim().to_string();
    let width = be_u16_at(buf, 16)?;
    let height = be_u16_at(buf, 18)?;
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(MediaError::ParseError("bad prores dimensions".into()));
    }
    let flags = buf[20];
    let chroma = if flags & 0xC0 == 0xC0 {
        ProResChroma::FourFourFour
    } else {
        ProResChroma::FourTwoTwo
    };
    let alpha = flags & 0x10 != 0;
    let slice_count_hint = be_u16_at(buf, 22).unwrap_or(0);
    Ok(ProResFrameHeader {
        frame_size,
        header_size,
        version,
        creator,
        width,
        height,
        chroma,
        alpha,
        slice_count_hint,
    })
}

/// Builds a synthetic `icpf` header for tests and fixtures.
pub fn build_frame_header(
    width: u16,
    height: u16,
    chroma: ProResChroma,
    alpha: bool,
    slices: u16,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(24);
    out.extend_from_slice(&24u32.to_be_bytes());
    out.extend_from_slice(&PRORES_FRAME_ID);
    out.extend_from_slice(&20u16.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(b"tont");
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    let mut flags = 0u8;
    if chroma == ProResChroma::FourFourFour {
        flags |= 0xC0;
    }
    if alpha {
        flags |= 0x10;
    }
    out.push(flags);
    out.push(0);
    out.extend_from_slice(&slices.to_be_bytes());
    out
}

/// GPU slice plan: horizontal 16px-aligned bands for WGPU compute
/// dispatch. Pure math, no wgpu dependency. One entry per slice
/// `(y_start, y_end)` in pixels.
pub fn gpu_slice_plan(width: u32, height: u32, slices: u16) -> Vec<(u32, u32)> {
    let n = (slices.max(1) as u32).min(32);
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let band = (height + n - 1) / n;
    let aligned = ((band + 15) / 16 * 16).max(16);
    let mut out = Vec::new();
    let mut y = 0u32;
    while y < height {
        let end = (y + aligned).min(height);
        out.push((y, end));
        y = end;
    }
    out
}

/// ffmpeg args for ProRes transcode targets (external path until the
/// native GPU encoder lands). Profile selects `prores_ks` profile index.
pub fn prores_ffmpeg_args(profile: ProResProfile) -> Vec<String> {
    let index = match profile {
        ProResProfile::Proxy => "0",
        ProResProfile::Lt => "1",
        ProResProfile::Standard => "2",
        ProResProfile::Hq => "3",
        ProResProfile::FourFourFourFour => "4",
        ProResProfile::FourFourFourFourXq => "5",
        ProResProfile::Unknown => "2",
    };
    vec![
        "-c:v".into(),
        "prores_ks".into(),
        "-profile:v".into(),
        index.into(),
        "-c:a".into(),
        "pcm_s16le".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_fourccs() {
        assert_eq!(profile_from_fourcc("apch"), ProResProfile::Hq);
        assert_eq!(profile_from_fourcc("AP4H"), ProResProfile::FourFourFourFour);
        assert_eq!(profile_from_fourcc("avc1"), ProResProfile::Unknown);
        assert!(is_prores_fourcc("apcn"));
        assert!(!is_prores_fourcc("mp4v"));
        assert!(ProResProfile::FourFourFourFour.has_alpha());
        assert!(!ProResProfile::Hq.has_alpha());
    }

    #[test]
    fn roundtrips_header() {
        let raw = build_frame_header(1920, 1080, ProResChroma::FourTwoTwo, false, 8);
        let hdr = parse_frame_header(&raw).unwrap();
        assert_eq!((hdr.width, hdr.height), (1920, 1080));
        assert_eq!(hdr.chroma, ProResChroma::FourTwoTwo);
        assert!(!hdr.alpha);
        assert_eq!(hdr.slice_count_hint, 8);
    }

    #[test]
    fn rejects_bad_magic_and_dims() {
        let mut raw = build_frame_header(1920, 1080, ProResChroma::FourTwoTwo, false, 4);
        raw[4..8].copy_from_slice(b"xxxx");
        assert!(parse_frame_header(&raw).is_err());
        let raw = build_frame_header(0, 1080, ProResChroma::FourTwoTwo, false, 4);
        assert!(parse_frame_header(&raw).is_err());
        assert!(parse_frame_header(b"short").is_err());
    }

    #[test]
    fn slice_plan_covers_frame() {
        let plan = gpu_slice_plan(1920, 1080, 8);
        assert!(!plan.is_empty());
        assert_eq!(plan.first().unwrap().0, 0);
        assert_eq!(plan.last().unwrap().1, 1080);
        for w in plan.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
    }

    #[test]
    fn prores_args_select_encoder() {
        let args = prores_ffmpeg_args(ProResProfile::Hq);
        assert!(args.contains(&"prores_ks".to_string()));
        assert!(args.contains(&"3".to_string()));
    }
}
