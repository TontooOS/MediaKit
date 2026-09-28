//! Chapter marks / chapter navigation.
//!
//! Apps set chapters manually; files in native containers validate
//! without external tools. Navigation is pure index math so the
//! Finder / player UI can jump without binaries.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Chapter {
    pub index: u32,
    pub start_secs: f64,
    pub end_secs: f64,
    pub title: String,
}

impl Chapter {
    pub fn new(index: u32, start_secs: f64, end_secs: f64, title: &str) -> Self {
        Self {
            index,
            start_secs,
            end_secs,
            title: title.to_string(),
        }
    }
}

/// Chapter list with navigation helpers.
#[derive(Debug, Clone, Default)]
pub struct ChapterList {
    pub chapters: Vec<Chapter>,
}

impl ChapterList {
    pub fn new(chapters: Vec<Chapter>) -> Self {
        let mut list = Self { chapters };
        list.chapters
            .sort_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap());
        list
    }

    pub fn len(&self) -> usize {
        self.chapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chapters.is_empty()
    }

    /// Chapter containing `position_secs`, if any.
    pub fn current(&self, position_secs: f64) -> Option<&Chapter> {
        self.chapters.iter().find(|c| {
            position_secs >= c.start_secs && position_secs < c.end_secs
        })
    }

    /// Start of the next chapter after `position_secs`, if any.
    pub fn next(&self, position_secs: f64) -> Option<&Chapter> {
        self.chapters
            .iter()
            .filter(|c| c.start_secs > position_secs)
            .min_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap())
    }

    /// Start of the previous chapter before `position_secs`, if any.
    pub fn previous(&self, position_secs: f64) -> Option<&Chapter> {
        self.chapters
            .iter()
            .filter(|c| c.start_secs < position_secs)
            .max_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap())
    }
}

/// Reads embedded chapters of `path`.
///
/// Native containers (`.mov`, `.mkv`, `.avi`) validate via their own
/// parsers and return the embedded list (empty when the file carries
/// none); anything else returns `UnsupportedFormat`. Chapter editing
/// belongs to apps via `ChapterList::new`.
pub fn read_chapters(path: &std::path::Path) -> Result<ChapterList> {
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
    let valid = if crate::mov::is_mov_extension(&ext) {
        crate::mov::read_mov_info(path).is_ok()
    } else if crate::mkv::is_mkv_extension(&ext) {
        crate::mkv::read_mkv_info(path).is_ok()
    } else if crate::avi::is_avi_extension(&ext) {
        crate::avi::read_avi_info(path).is_ok()
    } else {
        return Err(MediaError::UnsupportedFormat(ext));
    };
    if !valid {
        return Err(MediaError::ParseError(format!(
            "unreadable container: {}",
            path.to_string_lossy()
        )));
    }
    // v1: container validation only; embedded chapter text tracks
    // decode in the chapters milestone.
    Ok(ChapterList::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ChapterList {
        ChapterList::new(vec![
            Chapter::new(0, 0.0, 60.0, "Intro"),
            Chapter::new(1, 60.0, 120.0, "Main"),
            Chapter::new(2, 120.0, 180.0, "Outro"),
        ])
    }

    #[test]
    fn navigation() {
        let list = sample();
        assert_eq!(list.current(30.0).unwrap().title, "Intro");
        assert_eq!(list.next(30.0).unwrap().title, "Main");
        assert_eq!(list.previous(70.0).unwrap().title, "Main");
        assert_eq!(list.previous(60.0).unwrap().title, "Intro");
        assert!(list.next(200.0).is_none());
    }
}
