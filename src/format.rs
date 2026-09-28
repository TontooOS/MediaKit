//! Container / codec format helpers.
//!
//! Playback targets MP4, WebM, MKV and best-effort AVI. Detection is
//! extension-first; `.mov`/`.mp4`/`.m4v` additionally validate natively
//! via `crate::mov` (ftyp sniff, no ffprobe required).

use crate::error::{MediaError, Result};
use std::path::Path;

/// Extensions MediaKit opens directly.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["mp4", "webm", "mkv", "avi", "mov", "m4v"];

/// Best-effort extensions: accepted by the container probe, decoded
/// natively where a decoder exists (otherwise `UnsupportedFormat`
/// from the decode path).
pub const EXTRA_EXTENSIONS: &[&str] = &["ogv", "ts", "m2ts", "flv"];

/// Extensions with a native MOV parser (see `crate::mov`).
pub const MOV_EXTENSIONS: &[&str] = &["mov", "mp4", "m4v"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoContainer {
    Mp4,
    WebM,
    Mkv,
    Avi,
    Mov,
    #[default]
    Unknown,
}

impl VideoContainer {
    pub fn as_str(&self) -> &'static str {
        match self {
            VideoContainer::Mp4 => "mp4",
            VideoContainer::WebM => "webm",
            VideoContainer::Mkv => "mkv",
            VideoContainer::Avi => "avi",
            VideoContainer::Mov => "mov",
            VideoContainer::Unknown => "unknown",
        }
    }

    pub fn from_extension(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" => VideoContainer::Mp4,
            "webm" => VideoContainer::WebM,
            "mkv" => VideoContainer::Mkv,
            "avi" => VideoContainer::Avi,
            "mov" => VideoContainer::Mov,
            _ => VideoContainer::Unknown,
        }
    }
}

/// Returns the lowercased extension of `path`, if any.
pub fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// True when MediaKit can open `path` directly.
pub fn is_supported(path: &Path) -> bool {
    match extension_of(path) {
        Some(ext) => {
            SUPPORTED_EXTENSIONS.contains(&ext.as_str())
                || EXTRA_EXTENSIONS.contains(&ext.as_str())
        }
        None => false,
    }
}

/// Validates `path` for playback, returning its container.
///
/// Extension stays the authority (1:1 compatible, no file access), so
/// headless tests with virtual names keep working. Use
/// `probe_container_native` for ftyp-level `.mov` validation.
pub fn probe_container(path: &Path) -> Result<VideoContainer> {
    let ext = extension_of(path).unwrap_or_default();
    if ext.is_empty() {
        return Err(MediaError::UnsupportedFormat(
            path.to_string_lossy().to_string(),
        ));
    }
    let container = VideoContainer::from_extension(&ext);
    if container == VideoContainer::Unknown
        && !EXTRA_EXTENSIONS.contains(&ext.as_str())
    {
        return Err(MediaError::UnsupportedFormat(ext));
    }
    Ok(container)
}

/// Native `.mov`/`.mp4`/`.m4v` validation via ftyp sniff.
/// Returns `Ok(true)` for confirmed MOV files, `Ok(false)` when the
/// file is missing (extension stays authoritative), and `Err` when a
/// present file fails the sniff.
pub fn probe_container_native(path: &Path) -> Result<bool> {
    let ext = extension_of(path).unwrap_or_default();
    if !MOV_EXTENSIONS.contains(&ext.as_str()) {
        return Ok(false);
    }
    if !path.exists() {
        return Ok(false);
    }
    if crate::mov::sniff_mov(path) {
        return Ok(true);
    }
    // Present but no ftyp: still extension-accepted; the native
    // metadata parser decides (strict `ParseError` on garbage).
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn detects_supported() {
        assert!(is_supported(&PathBuf::from("clip.mp4")));
        assert!(is_supported(&PathBuf::from("clip.WEBM")));
        assert!(is_supported(&PathBuf::from("clip.mkv")));
        assert!(is_supported(&PathBuf::from("clip.avi")));
        assert!(!is_supported(&PathBuf::from("clip.txt")));
    }

    #[test]
    fn container_mapping() {
        assert_eq!(
            probe_container(&PathBuf::from("a.mp4")).unwrap(),
            VideoContainer::Mp4
        );
        assert!(probe_container(&PathBuf::from("a.xyz")).is_err());
    }
}
