//! Video metadata from the native container parsers (`.mov`,
//! `.mkv`, `.avi`). No external binaries.

use crate::error::{MediaError, Result};
use std::path::Path;

#[derive(Debug, Clone, Default)]
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

/// Reads metadata for `path` with the native parsers only.
/// Returns `UnsupportedFormat` for unknown extensions and
/// `ParseError` for damaged files.
pub fn read_metadata(path: &Path) -> Result<VideoMetadata> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext.is_empty() {
        return Err(MediaError::UnsupportedFormat(
            path.to_string_lossy().to_string(),
        ));
    }
    if !path.exists() {
        return Err(MediaError::IoError(format!(
            "file not found: {}",
            path.to_string_lossy()
        )));
    }
    if crate::mov::is_mov_extension(&ext) {
        return crate::mov::read_mov_metadata(path);
    }
    if crate::mkv::is_mkv_extension(&ext) {
        return crate::mkv::read_mkv_metadata(path);
    }
    if crate::avi::is_avi_extension(&ext) {
        return crate::avi::read_avi_metadata(path);
    }
    Err(MediaError::UnsupportedFormat(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_extension() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("mediakit-meta-{}.txt", std::process::id()));
        std::fs::write(&path, b"hello").unwrap();
        let err = read_metadata(&path).unwrap_err();
        assert!(matches!(err, MediaError::UnsupportedFormat(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_is_io_error() {
        let err = read_metadata(Path::new("/definitely/missing/clip.mp4")).unwrap_err();
        assert!(matches!(err, MediaError::IoError(_)));
    }
}
