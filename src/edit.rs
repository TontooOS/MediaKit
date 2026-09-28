//! Native editing: lossless trim and concat of `.mov` files.
//!
//! - `trim_mov_native`: cuts `[start_secs, end_secs)` per track.
//! - `concat_mov_native`: appends clips with matching track layout.
//!
//! Pure Rust, no external binaries. Re-encoding (transcoding)
//! needs encoders and is intentionally absent: use the format
//! writers (`build_raw_mov`, ProRes encoder milestone) to produce
//! new samples instead.

use crate::error::{MediaError, Result};
use std::path::Path;

fn mov_output_ext(output: &Path) -> Result<String> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !crate::mov::is_mov_extension(&ext) {
        return Err(MediaError::UnsupportedFormat(ext));
    }
    Ok(ext)
}

/// Losslessly trims a `.mov` to `[start_secs, end_secs)` with pure
/// Rust (no ffmpeg): every video/audio track is cut independently in
/// its own timescale, sample tables and durations are rewritten,
/// frame bytes land in one chunk per track. Supports uniform or
/// indexed `stsz`, `stco`/`co64`, optional `stss`/`ctts`; edit lists,
/// compact sample tables and non-A/V tracks return `ParseError`.
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
    mov_output_ext(output)?;
    let data = std::fs::read(input).map_err(MediaError::from_io)?;
    if data.len() > crate::mov::MAX_MOV_FILE_SIZE {
        return Err(MediaError::ParseError("file too large".into()));
    }
    let trimmed = crate::mov::trim_mov_bytes(&data, start_secs, end_secs)?;
    std::fs::write(output, trimmed).map_err(MediaError::from_io)?;
    Ok(())
}

/// Losslessly concatenates `.mov` inputs (same track layout, codecs,
/// dimensions and timescales) into `output` with pure Rust.
pub fn concat_mov_native(inputs: &[&Path], output: &Path) -> Result<()> {
    if inputs.is_empty() {
        return Err(MediaError::ParseError("no inputs to concat".into()));
    }
    if inputs.len() > 1024 {
        return Err(MediaError::ParseError("too many inputs".into()));
    }
    for input in inputs {
        crate::format::probe_container(input)?;
    }
    mov_output_ext(output)?;
    let mut blobs: Vec<Vec<u8>> = Vec::with_capacity(inputs.len());
    for input in inputs {
        let data = std::fs::read(input).map_err(MediaError::from_io)?;
        if data.len() > crate::mov::MAX_MOV_FILE_SIZE {
            return Err(MediaError::ParseError("file too large".into()));
        }
        blobs.push(data);
    }
    let refs: Vec<&[u8]> = blobs.iter().map(|b| b.as_slice()).collect();
    let joined = crate::mov::concat_mov_bytes(&refs)?;
    std::fs::write(output, joined).map_err(MediaError::from_io)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mov::{build_raw_mov, RawMovParams};

    fn fixture(tag: &str, frames: u32) -> (std::path::PathBuf, Vec<u8>) {
        let params = RawMovParams {
            width: 4,
            height: 2,
            fps: 10,
            audio: None,
        };
        let body: Vec<Vec<u8>> = (0..frames).map(|f| vec![f as u8; 4 * 2 * 3]).collect();
        let file = build_raw_mov(&params, &body).unwrap();
        let dir = std::env::temp_dir();
        let path = dir.join(format!(
            "mediakit-edit-{tag}-{}-{}.mov",
            std::process::id(),
            frames
        ));
        std::fs::write(&path, &file).unwrap();
        (path, file)
    }

    #[test]
    fn trim_mov_native_validates_range() {
        let (src, _) = fixture("range", 10);
        let out = src.with_extension("trim.mov");
        assert!(trim_mov_native(&src, 10.0, 5.0, &out).is_err());
        assert!(trim_mov_native(&src, 0.0, 0.5, &Path::new("out.mp4").with_extension("xyz"))
            .is_err());
        let _ = std::fs::remove_file(&src);
    }

    #[test]
    fn trim_mov_native_roundtrip() {
        let (src, _) = fixture("roundtrip", 10);
        let out = std::env::temp_dir().join(format!("mediakit-edit-out-{}-{}.mov", "roundtrip", std::process::id()));
        trim_mov_native(&src, 0.2, 0.5, &out).unwrap();
        let meta = crate::read_mov_metadata(&out).unwrap();
        assert!((meta.duration_secs - 0.3).abs() < 0.001);
        let _ = std::fs::remove_file(&src);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn concat_mov_native_joins() {
        let (a, _) = fixture("conca", 4);
        let (b, _) = fixture("concb", 6);
        let out = std::env::temp_dir().join(format!("mediakit-concat-out-{}.mov", std::process::id()));
        concat_mov_native(&[a.as_path(), b.as_path()], &out).unwrap();
        let meta = crate::read_mov_metadata(&out).unwrap();
        assert!((meta.duration_secs - 1.0).abs() < 0.001);
        assert!(concat_mov_native(&[], &out).is_err());
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
        let _ = std::fs::remove_file(&out);
    }
}
