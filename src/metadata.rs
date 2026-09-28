//! Video metadata: native `.mov` parser first, ffprobe fallback.
//!
//! `read_metadata` tries `crate::mov::read_mov_metadata` for
//! `.mov`/`.mp4`/`.m4v` so those behave 1:1 without binaries, then
//! shells to `ffprobe -v quiet -print_format json -show_format
//! -show_streams`. When ffprobe is missing it returns
//! `MediaError::FfmpegMissing`; unparsable output is `ParseError`.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct VideoMetadata {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub framerate: f64,
    pub container: String,
    pub size_bytes: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    #[serde(default)]
    format: FfprobeFormat,
    #[serde(default)]
    streams: Vec<FfprobeStream>,
}

#[derive(Debug, Default, Deserialize)]
struct FfprobeFormat {
    #[serde(default)]
    duration: Option<String>,
    #[serde(default)]
    format_name: Option<String>,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct FfprobeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    codec_name: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    avg_frame_rate: Option<String>,
}

/// Reads metadata for `path`: native MOV/MKV first, ffprobe fallback.
pub fn read_metadata(path: &Path) -> Result<VideoMetadata> {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if path.exists() {
            if crate::mov::is_mov_extension(ext) {
                if let Ok(meta) = crate::mov::read_mov_metadata(path) {
                    return Ok(meta);
                }
                // Fall through to ffprobe for damaged/partial files.
            } else if crate::mkv::is_mkv_extension(ext) {
                if let Ok(meta) = crate::mkv::read_mkv_metadata(path) {
                    return Ok(meta);
                }
                // Fall through to ffprobe for damaged/partial files.
            }
        }
    }
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            &path.to_string_lossy(),
        ])
        .output()
        .map_err(MediaError::from_io)?;
    if !out.status.success() {
        return Err(MediaError::CommandFailed(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    parse_ffprobe_json(&out.stdout, path)
}

fn parse_ffprobe_json(raw: &[u8], path: &Path) -> Result<VideoMetadata> {
    let parsed: FfprobeOutput = serde_json::from_slice(raw)
        .map_err(|e| MediaError::ParseError(e.to_string()))?;
    let duration_secs = parsed
        .format
        .duration
        .as_deref()
        .unwrap_or("0")
        .parse::<f64>()
        .unwrap_or(0.0);
    let container = parsed.format.format_name.unwrap_or_default();
    let size_bytes = parsed.format.size.and_then(|s| s.parse::<u64>().ok());
    let video = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("video"));
    let audio = parsed
        .streams
        .iter()
        .find(|s| s.codec_type.as_deref() == Some("audio"));
    let (width, height, video_codec, framerate) = match video {
        Some(v) => (
            v.width.unwrap_or(0),
            v.height.unwrap_or(0),
            v.codec_name.clone().unwrap_or_default(),
            v.avg_frame_rate
                .as_deref()
                .map(parse_framerate)
                .unwrap_or(0.0),
        ),
        None => (0, 0, String::new(), 0.0),
    };
    let _ = path;
    Ok(VideoMetadata {
        duration_secs,
        width,
        height,
        video_codec,
        audio_codec: audio.and_then(|a| a.codec_name.clone()),
        framerate,
        container,
        size_bytes,
    })
}

/// Parses ffprobe `avg_frame_rate` values like `"30000/1001"` or `"25/1"`.
pub fn parse_framerate(raw: &str) -> f64 {
    let raw = raw.trim();
    if raw.is_empty() || raw == "0/0" {
        return 0.0;
    }
    if let Some((num, den)) = raw.split_once('/') {
        let n: f64 = num.parse().unwrap_or(0.0);
        let d: f64 = den.parse().unwrap_or(0.0);
        if d == 0.0 {
            return 0.0;
        }
        return n / d;
    }
    raw.parse().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_framerate() {
        assert!((parse_framerate("30000/1001") - 29.97).abs() < 0.01);
        assert_eq!(parse_framerate("25/1"), 25.0);
        assert_eq!(parse_framerate("0/0"), 0.0);
    }

    #[test]
    fn parses_ffprobe_json() {
        let raw = br#"{
            "format": {"duration": "12.5", "format_name": "mov,mp4", "size": "1024"},
            "streams": [
                {"codec_type": "video", "codec_name": "h264", "width": 1920, "height": 1080, "avg_frame_rate": "30/1"},
                {"codec_type": "audio", "codec_name": "aac"}
            ]
        }"#;
        let meta = parse_ffprobe_json(raw, Path::new("clip.mp4")).unwrap();
        assert_eq!(meta.duration_secs, 12.5);
        assert_eq!((meta.width, meta.height), (1920, 1080));
        assert_eq!(meta.video_codec, "h264");
        assert_eq!(meta.audio_codec.as_deref(), Some("aac"));
        assert_eq!(meta.framerate, 30.0);
    }

    #[test]
    fn missing_ffprobe_is_graceful() {
        let result = read_metadata(Path::new("/definitely/missing/clip.mp4"));
        assert!(result.is_err());
    }
}
