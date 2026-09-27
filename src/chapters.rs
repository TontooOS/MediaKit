//! Chapter marks / chapter navigation.
//!
//! Chapters come from ffprobe (`-show_chapters`) or are set manually by
//! apps. Navigation is pure index math so the Finder / player UI can jump
//! without touching ffmpeg.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;

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

#[derive(Debug, Deserialize)]
struct ChaptersOutput {
    #[serde(default)]
    chapters: Vec<FfChapter>,
}

#[derive(Debug, Deserialize)]
struct FfChapter {
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    start_time: Option<String>,
    #[serde(default)]
    end_time: Option<String>,
    #[serde(default)]
    tags: Option<std::collections::HashMap<String, String>>,
}

/// Reads chapters via `ffprobe -show_chapters`.
/// Valid `.mov` files return an (empty) native list when ffprobe is
/// missing so `.mov` behaves 1:1 offline.
pub fn read_chapters(path: &std::path::Path) -> Result<ChapterList> {
    let native_mov_valid = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| crate::mov::is_mov_extension(e))
        .unwrap_or(false)
        && path.exists()
        && crate::mov::read_mov_info(path).is_ok();
    let out = match Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_chapters",
            &path.to_string_lossy(),
        ])
        .output()
    {
        Ok(out) => out,
        Err(e) => {
            let io_err = MediaError::from_io(e);
            if native_mov_valid && matches!(io_err, MediaError::FfmpegMissing) {
                return Ok(ChapterList::default());
            }
            return Err(io_err);
        }
    };
    if !out.status.success() {
        return Err(MediaError::CommandFailed(
            String::from_utf8_lossy(&out.stderr).to_string(),
        ));
    }
    let parsed: ChaptersOutput = serde_json::from_slice(&out.stdout)
        .map_err(|e| MediaError::ParseError(e.to_string()))?;
    let chapters = parsed
        .chapters
        .into_iter()
        .enumerate()
        .map(|(i, c)| {
            let title = c
                .tags
                .as_ref()
                .and_then(|t| t.get("title").cloned())
                .unwrap_or_else(|| format!("Chapter {}", i + 1));
            Chapter {
                index: c.id.unwrap_or(i as u32),
                start_secs: c.start_time.as_deref().unwrap_or("0").parse().unwrap_or(0.0),
                end_secs: c.end_time.as_deref().unwrap_or("0").parse().unwrap_or(0.0),
                title,
            }
        })
        .collect();
    Ok(ChapterList::new(chapters))
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
