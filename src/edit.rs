//! Basic editing: trim, concat and transcode.
//!
//! - Trim: `ffmpeg -ss <start> -to <end> -c copy` (fast) or re-encode.
//! - Concat: ffmpeg concat demuxer with a temp file list.
//! - Transcode: container/codec switch between MP4/WebM/MKV/MOV.
//! - ProRes: `transcode_prores` writes ProRes 422 family targets
//!   (`prores_ks`); native GPU encode follows without API churn.
//!
//! All inputs validate via `format::probe_container` first so `.mov`
//! behaves 1:1 offline; execution stays on ffmpeg for now.

use crate::error::{MediaError, Result};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ExportPreset {
    #[default]
    Copy,
    H264Fast,
    WebM,
}

impl ExportPreset {
    pub fn ffmpeg_args(&self) -> Vec<String> {
        match self {
            ExportPreset::Copy => vec!["-c".into(), "copy".into()],
            ExportPreset::H264Fast => vec![
                "-c:v".into(),
                "libx264".into(),
                "-preset".into(),
                "fast".into(),
                "-c:a".into(),
                "aac".into(),
            ],
            ExportPreset::WebM => vec![
                "-c:v".into(),
                "libvpx-vp9".into(),
                "-c:a".into(),
                "libopus".into(),
            ],
        }
    }
}

/// Trims `input` to `[start_secs, end_secs]` and writes `output`.
pub fn trim(
    input: &Path,
    start_secs: f64,
    end_secs: f64,
    output: &Path,
    preset: ExportPreset,
) -> Result<()> {
    crate::format::probe_container(input)?;
    crate::format::probe_container(output).ok();
    if start_secs < 0.0 || end_secs <= start_secs {
        return Err(MediaError::InvalidSeek(format!(
            "{start_secs}-{end_secs}"
        )));
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-ss", &start_secs.to_string(), "-i"]);
    cmd.arg(input);
    cmd.args(["-to", &end_secs.to_string()]);
    for arg in preset.ffmpeg_args() {
        cmd.arg(arg);
    }
    cmd.arg(output);
    let status = cmd.status().map_err(MediaError::from_io)?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::CommandFailed("ffmpeg trim failed".into()))
    }
}

/// Concatenates `inputs` (same codec) into `output` via concat demuxer.
pub fn concat(inputs: &[&Path], output: &Path) -> Result<()> {
    if inputs.is_empty() {
        return Err(MediaError::ParseError("no inputs to concat".into()));
    }
    for input in inputs {
        crate::format::probe_container(input)?;
    }
    let list_file = std::env::temp_dir().join(format!("mediakit-concat-{}.txt", std::process::id()));
    let mut list = String::new();
    for input in inputs {
        list.push_str(&format!("file '{}'\n", input.to_string_lossy().replace('\'', "'\\''")));
    }
    std::fs::write(&list_file, list).map_err(MediaError::from_io)?;
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &list_file.to_string_lossy(),
            "-c",
            "copy",
            &output.to_string_lossy(),
        ])
        .status()
        .map_err(MediaError::from_io)?;
    let _ = std::fs::remove_file(&list_file);
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::CommandFailed("ffmpeg concat failed".into()))
    }
}

/// Transcodes `input` into `output` with `preset`.
pub fn transcode(input: &Path, output: &Path, preset: ExportPreset) -> Result<()> {
    crate::format::probe_container(input)?;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i"]);
    cmd.arg(input);
    for arg in preset.ffmpeg_args() {
        cmd.arg(arg);
    }
    cmd.arg(output);
    let status = cmd.status().map_err(MediaError::from_io)?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::CommandFailed(
            "ffmpeg transcode failed".into(),
        ))
    }
}

/// Transcodes `input` into a ProRes `.mov` target.
/// Validates natively, executes via ffmpeg `prores_ks` for now.
pub fn transcode_prores(
    input: &Path,
    output: &Path,
    profile: crate::prores::ProResProfile,
) -> Result<()> {
    crate::format::probe_container(input)?;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-i"]);
    cmd.arg(input);
    for arg in crate::prores::prores_ffmpeg_args(profile) {
        cmd.arg(arg);
    }
    cmd.arg(output);
    let status = cmd.status().map_err(MediaError::from_io)?;
    if status.success() {
        Ok(())
    } else {
        Err(MediaError::CommandFailed(
            "ffmpeg prores transcode failed".into(),
        ))
    }
}

/// Losslessly trims a single-track `.mov` to `[start_secs, end_secs)`
/// with pure Rust (no ffmpeg): sample tables and durations are
/// rewritten, frame bytes are copied. Supports uniform or indexed
/// `stsz`, `stco`/`co64`, optional `stss`/`ctts`; multi-track files,
/// edit lists and compact sample tables return `ParseError`.
pub fn trim_mov_native(
    input: &Path,
    start_secs: f64,
    end_secs: f64,
    output: &Path,
) -> Result<()> {
    if start_secs < 0.0 || end_secs <= start_secs {
        return Err(MediaError::InvalidSeek(format!(
            "{start_secs}-{end_secs}"
        )));
    }
    crate::format::probe_container(input)?;
    let out_ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !crate::mov::is_mov_extension(&out_ext) {
        return Err(MediaError::UnsupportedFormat(out_ext));
    }
    let data = std::fs::read(input).map_err(MediaError::from_io)?;
    let trimmed = crate::mov::trim_mov_bytes(&data, start_secs, end_secs)?;
    std::fs::write(output, trimmed).map_err(MediaError::from_io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_args() {
        assert_eq!(
            ExportPreset::Copy.ffmpeg_args(),
            vec!["-c".to_string(), "copy".to_string()]
        );
        assert!(ExportPreset::H264Fast
            .ffmpeg_args()
            .contains(&"libx264".to_string()));
    }

    #[test]
    fn trim_validates_range() {
        let err = trim(
            Path::new("in.mp4"),
            10.0,
            5.0,
            Path::new("out.mp4"),
            ExportPreset::Copy,
        )
        .unwrap_err();
        assert!(matches!(err, MediaError::InvalidSeek(_)));
    }

    #[test]
    fn concat_needs_inputs() {
        assert!(concat(&[], Path::new("out.mp4")).is_err());
    }
}
