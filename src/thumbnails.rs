//! Thumbnails / frame extraction via ffmpeg.
//!
//! `extract_frame` runs `ffmpeg -ss <at> -i <input> -vframes 1 <output>`
//! which works for MP4/WebM/MKV/AVI without extra codecs. Used by the
//! Finder for video previews.

use crate::error::{MediaError, Result};
use std::path::Path;
use std::process::Command;

/// Extracts a single frame at `at_secs` into `output` (PNG/JPG by extension).
pub fn extract_frame(input: &Path, at_secs: f64, output: &Path) -> Result<()> {
    if at_secs < 0.0 || !at_secs.is_finite() {
        return Err(MediaError::InvalidSeek(at_secs.to_string()));
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
}
