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

/// Extracts a single frame at `at_secs` into a PNG with pure Rust
/// (no ffmpeg). Supports `.mov`/`.mp4`/`.m4v` with raw, TDC-1
/// (`icpf`) or MJPEG video tracks, and `.avi` with MJPEG (`dc`) or
/// raw RGB (`db`) chunks. `output` must end in `.png`.
pub fn extract_frame_native(input: &Path, at_secs: f64, output: &Path) -> Result<()> {
    if at_secs < 0.0 || !at_secs.is_finite() {
        return Err(MediaError::InvalidSeek(at_secs.to_string()));
    }
    require_png(output)?;
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
    let (w, h, rgb) = if crate::mov::is_mov_extension(&ext) {
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
        decode_mov_sample(&pick.codec_fourcc, &bytes, info.width, info.height)?
    } else if ext == "avi" {
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
        if pick.kind == "db"
            && bytes.len() == (info.width as usize) * (info.height as usize) * 3
        {
            (info.width, info.height, bytes)
        } else {
            let img = crate::mjpeg::decode_jpeg(&bytes)
                .map_err(|e| MediaError::ParseError(e.to_string()))?;
            (img.width, img.height, img.rgb)
        }
    } else {
        return Err(MediaError::UnsupportedFormat(format!(
            "no native thumbnail for .{ext}"
        )));
    };
    let png =
        crate::png_mini::encode_png_rgb(w, h, &rgb).map_err(MediaError::ParseError)?;
    std::fs::write(output, png).map_err(MediaError::from_io)?;
    Ok(())
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
