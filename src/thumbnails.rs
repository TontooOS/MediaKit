//! Thumbnails / frame extraction.
//!
//! `extract_frame` runs `ffmpeg -ss <at> -i <input> -vframes 1 <output>`
//! which works for MP4/WebM/MKV/AVI/MOV without extra codecs.
//! `extract_frame_native` needs no binaries for natively decodable
//! tracks: `.mov` raw/TDC-1/MJPEG and `.avi` MJPEG/RGB.
//! Used by the Finder for video previews.

use crate::error::{MediaError, Result};
use std::path::Path;
use std::process::Command;

/// Extracts a single frame at `at_secs` into `output` (PNG/JPG by extension).
pub fn extract_frame(input: &Path, at_secs: f64, output: &Path) -> Result<()> {
    if at_secs < 0.0 || !at_secs.is_finite() {
        return Err(MediaError::InvalidSeek(at_secs.to_string()));
    }
    crate::format::probe_container(input)?;
    if !input.exists() {
        return Err(MediaError::IoError(format!(
            "file not found: {}",
            input.to_string_lossy()
        )));
    }
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-ss",
            &at_secs.to_string(),
            "-i",
            &input.to_string_lossy(),
            "-vframes",
            "1",
            &output.to_string_lossy(),
        ])
        .status()
        .map_err(MediaError::from_io)?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::CommandFailed(format!(
            "ffmpeg thumbnail failed for {}",
            input.to_string_lossy()
        )))
    }
}

/// Builds the ffmpeg args for frame extraction (testable without binaries).
pub fn extract_args(input: &Path, at_secs: f64, output: &Path) -> Vec<String> {
    vec![
        "-y".into(),
        "-ss".into(),
        at_secs.to_string(),
        "-i".into(),
        input.to_string_lossy().to_string(),
        "-vframes".into(),
        "1".into(),
        output.to_string_lossy().to_string(),
    ]
}

fn parse_error(msg: &str) -> MediaError {
    MediaError::ParseError(msg.into())
}

/// Extracts a single frame at `at_secs` into a PNG with pure Rust
/// (no ffmpeg). Supports `.mov`/`.mp4`/`.m4v` with raw, TDC-1
/// (`icpf`) or MJPEG video tracks, and `.avi` with MJPEG (`dc`) or
/// raw RGB (`db`) chunks. `output` must end in `.png`.
pub fn extract_frame_native(input: &Path, at_secs: f64, output: &Path) -> Result<()> {
    require_png(output)?;
    let frame = decode_video_frame(input, at_secs)?;
    let png = crate::png_mini::encode_png_rgb(frame.width, frame.height, &frame.rgb)
        .map_err(MediaError::ParseError)?;
    std::fs::write(output, png).map_err(MediaError::from_io)?;
    Ok(())
}

fn require_png(output: &Path) -> Result<()> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext == "png" {
        Ok(())
    } else {
        Err(MediaError::UnsupportedFormat(format!(
            "native thumbnails write .png, got .{ext}"
        )))
    }
}

fn read_sample_bytes(input: &Path, offset: u64, size: u32) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(input).map_err(MediaError::from_io)?;
    f.seek(SeekFrom::Start(offset)).map_err(MediaError::from_io)?;
    let mut buf = vec![0u8; size as usize];
    f.read_exact(&mut buf).map_err(MediaError::from_io)?;
    Ok(buf)
}

/// Decodes one `.mov` sample to RGB24 using the native codecs.
fn decode_mov_sample(
    fourcc: &str,
    bytes: &[u8],
    width: u32,
    height: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    let f = fourcc.trim().to_ascii_lowercase();
    if f == "raw " || f == "raw" {
        let expect = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(3))
            .ok_or_else(|| parse_error("raw dims overflow"))?;
        if bytes.len() != expect {
            return Err(parse_error("raw sample size mismatch"));
        }
        return Ok((width, height, bytes.to_vec()));
    }
    if bytes.len() >= 8 && &bytes[4..8] == b"icpf" {
        // TDC-1 native frame.
        let frame = crate::prores_frame::decode_frame(bytes)
            .map_err(|e| MediaError::ParseError(e.to_string()))?;
        let rgb = crate::prores_frame::yuv_to_rgb(&frame);
        return Ok((frame.width, frame.height, rgb));
    }
    if f == "mjpa" || f == "mjpg" || f == "jpeg" {
        let img =
            crate::mjpeg::decode_jpeg(bytes).map_err(|e| MediaError::ParseError(e.to_string()))?;
        return Ok((img.width, img.height, img.rgb));
    }
    Err(MediaError::UnsupportedFormat(format!(
        "no native decoder for {fourcc}"
    )))
}

/// One natively decoded video frame (CPU-side RGB24).
/// Apps upload this to a WGPU texture; no player binary involved.
#[derive(Debug, Clone)]
pub struct NativeFrame {
    pub width: u32,
    pub height: u32,
    pub pts_secs: f64,
    pub rgb: Vec<u8>,
}

/// Decodes the video frame at `at_secs` to RGB24 with pure Rust.
/// Supports `.mov`/`.mp4`/`.m4v` (raw, TDC-1, MJPEG) and `.avi`
/// (MJPEG, raw RGB). Backs `extract_frame_native` and the native
/// playback path (`VideoPlayer::frame_at`).
pub fn decode_video_frame(input: &Path, at_secs: f64) -> Result<NativeFrame> {
    if at_secs < 0.0 || !at_secs.is_finite() {
        return Err(MediaError::InvalidSeek(at_secs.to_string()));
    }
    crate::format::probe_container(input)?;
    if !input.exists() {
        return Err(MediaError::IoError(format!(
            "file not found: {}",
            input.to_string_lossy()
        )));
    }
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if crate::mov::is_mov_extension(&ext) {
        let info = crate::mov::read_mov_info(input)?;
        let samples = crate::mov::read_mov_samples(input)?;
        if samples.is_empty() {
            return Err(parse_error("no video samples"));
        }
        let mut pick = &samples[0];
        for s in &samples {
            if s.pts_secs <= at_secs {
                pick = s;
            } else {
                break;
            }
        }
        let bytes = read_sample_bytes(input, pick.offset, pick.size)?;
        let (w, h, rgb) = decode_mov_sample(&pick.codec_fourcc, &bytes, info.width, info.height)?;
        return Ok(NativeFrame {
            width: w,
            height: h,
            pts_secs: pick.pts_secs,
            rgb,
        });
    }
    if ext == "avi" {
        let info = crate::avi::read_avi_info(input)?;
        let chunks = crate::avi::read_avi_chunks(input, 1_000_000)?;
        let video: Vec<_> = chunks
            .iter()
            .filter(|c| c.kind == "dc" || c.kind == "db")
            .collect();
        if video.is_empty() {
            return Err(parse_error("no video chunks"));
        }
        let frac = if info.duration_secs > 0.0 {
            (at_secs / info.duration_secs).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let pick = video[((video.len() as f64 * frac) as usize).min(video.len() - 1)];
        let bytes = read_sample_bytes(input, pick.offset, pick.size)?;
        let pts_secs = frac * info.duration_secs;
        if pick.kind == "db"
            && bytes.len() == (info.width as usize) * (info.height as usize) * 3
        {
            return Ok(NativeFrame {
                width: info.width,
                height: info.height,
                pts_secs,
                rgb: bytes,
            });
        }
        let img = crate::mjpeg::decode_jpeg(&bytes)
            .map_err(|e| MediaError::ParseError(e.to_string()))?;
        return Ok(NativeFrame {
            width: img.width,
            height: img.height,
            pts_secs,
            rgb: img.rgb,
        });
    }
    Err(MediaError::UnsupportedFormat(format!(
        "no native decode for .{ext}"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn builds_args() {
        let args = extract_args(
            &PathBuf::from("in.mp4"),
            5.0,
            &PathBuf::from("out.png"),
        );
        assert!(args.contains(&"-ss".to_string()));
        assert!(args.contains(&"in.mp4".to_string()));
        assert!(args.contains(&"out.png".to_string()));
    }

    #[test]
    fn rejects_negative() {
        let err = extract_frame(
            &PathBuf::from("in.mp4"),
            -1.0,
            &PathBuf::from("out.png"),
        )
        .unwrap_err();
        assert!(matches!(err, MediaError::InvalidSeek(_)));
    }

    #[test]
    fn native_rejects_non_png() {
        let err = extract_frame_native(
            &PathBuf::from("in.mov"),
            1.0,
            &PathBuf::from("out.jpg"),
        )
        .unwrap_err();
        assert!(matches!(err, MediaError::UnsupportedFormat(_)));
    }

    #[test]
    fn native_thumbnail_from_mjpeg_avi() {
        use crate::mjpeg::{encode_jpeg_fixture, JpegSampling};
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
        let gray: Vec<u8> = (0..256u32).flat_map(|i| [i as u8, i as u8, i as u8]).collect();
        let jpeg = encode_jpeg_fixture(&gray, 16, 16, JpegSampling::Gray, 4.0).unwrap();
        let mut avih = vec![0u8; 56];
        avih[0..4].copy_from_slice(&100_000u32.to_le_bytes());
        avih[16..20].copy_from_slice(&1u32.to_le_bytes());
        avih[24..28].copy_from_slice(&1u32.to_le_bytes());
        avih[32..36].copy_from_slice(&16u32.to_le_bytes());
        avih[36..40].copy_from_slice(&16u32.to_le_bytes());
        let mut strh = vec![0u8; 56];
        strh[0..4].copy_from_slice(b"vids");
        strh[4..8].copy_from_slice(b"MJPG");
        strh[20..24].copy_from_slice(&1u32.to_le_bytes());
        strh[24..28].copy_from_slice(&10u32.to_le_bytes());
        strh[32..36].copy_from_slice(&1u32.to_le_bytes());
        let mut strf = vec![0u8; 40];
        strf[0..4].copy_from_slice(&40u32.to_le_bytes());
        strf[4..8].copy_from_slice(&16i32.to_le_bytes());
        strf[8..12].copy_from_slice(&16i32.to_le_bytes());
        strf[12..14].copy_from_slice(&1u16.to_le_bytes());
        strf[14..16].copy_from_slice(&24u16.to_le_bytes());
        strf[16..20].copy_from_slice(b"MJPG");
        let strl = list(b"strl", &[chunk(b"strh", &strh), chunk(b"strf", &strf)].concat());
        let hdrl = list(b"hdrl", &[chunk(b"avih", &avih), strl].concat());
        let movi = list(b"movi", &chunk(b"00dc", &jpeg));
        let riff_body: Vec<u8> = [b"AVI ".as_slice(), &hdrl, &movi].concat();
        let mut file = Vec::new();
        file.extend_from_slice(b"RIFF");
        file.extend_from_slice(&(riff_body.len() as u32).to_le_bytes());
        file.extend_from_slice(&riff_body);
        let dir = std::env::temp_dir();
        let src = dir.join(format!("mediakit-mjpgavi-{}.avi", std::process::id()));
        let dst = dir.join(format!("mediakit-mjpgavi-{}.png", std::process::id()));
        std::fs::write(&src, &file).unwrap();
        extract_frame_native(&src, 0.0, &dst).unwrap();
        let png = std::fs::read(&dst).unwrap();
        let (w, h, rgb) = crate::png_mini::decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (16, 16));
        let max = gray
            .iter()
            .zip(rgb.iter())
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap();
        assert!(max <= 20, "max drift {max}");
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }

    #[test]
    fn native_thumbnail_from_raw_mov() {
        use crate::mov::{build_raw_mov, RawMovParams};
        let params = RawMovParams {
            width: 8,
            height: 4,
            fps: 10,
            audio: None,
        };
        let body: Vec<Vec<u8>> = (0..10).map(|f| vec![f as u8; 8 * 4 * 3]).collect();
        let file = build_raw_mov(&params, &body).unwrap();
        let dir = std::env::temp_dir();
        let src = dir.join(format!("mediakit-thumb-{}.mov", std::process::id()));
        let dst = dir.join(format!("mediakit-thumb-{}.png", std::process::id()));
        std::fs::write(&src, &file).unwrap();
        // 0.25s -> frame 2 (all bytes 0x02).
        extract_frame_native(&src, 0.25, &dst).unwrap();
        let png = std::fs::read(&dst).unwrap();
        let (w, h, rgb) = crate::png_mini::decode_png_stored(&png).unwrap();
        assert_eq!((w, h), (8, 4));
        assert!(rgb.iter().all(|&b| b == 2));
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&dst);
    }
}
