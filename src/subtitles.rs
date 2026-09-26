//! Subtitle support: SRT and WebVTT parsing + timestamp lookup.
//!
//! Pure Rust, no ffmpeg dependency. `SubtitleTrack::cue_at` returns the
//! active cue text for a playback position in seconds.

use crate::error::{MediaError, Result};

#[derive(Debug, Clone, Default)]
pub struct SubtitleCue {
    pub start_secs: f64,
    pub end_secs: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct SubtitleTrack {
    pub cues: Vec<SubtitleCue>,
    pub language: Option<String>,
}

impl SubtitleTrack {
    pub fn from_srt(text: &str) -> Result<Self> {
        let cues = parse_srt(text)?;
        Ok(Self {
            cues,
            language: None,
        })
    }

    pub fn from_vtt(text: &str) -> Result<Self> {
        let cues = parse_vtt(text)?;
        Ok(Self {
            cues,
            language: None,
        })
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).map_err(MediaError::from_io)?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "srt" => Self::from_srt(&text),
            "vtt" => Self::from_vtt(&text),
            other => Err(MediaError::SubtitleError(format!(
                "unsupported subtitle extension: {other}"
            ))),
        }
    }

    /// Active cue text at `position_secs`, if any.
    pub fn cue_at(&self, position_secs: f64) -> Option<&str> {
        self.cues
            .iter()
            .find(|c| position_secs >= c.start_secs && position_secs <= c.end_secs)
            .map(|c| c.text.as_str())
    }

    pub fn len(&self) -> usize {
        self.cues.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }
}

fn parse_timestamp(raw: &str) -> Result<f64> {
    let raw = raw.trim().replace(',', ".");
    let mut parts: Vec<&str> = raw.split(':').collect();
    if parts.len() == 2 {
        parts.insert(0, "0");
    }
    if parts.len() != 3 {
        return Err(MediaError::SubtitleError(format!("bad timestamp {raw}")));
    }
    let h: f64 = parts[0]
        .parse()
        .map_err(|_| MediaError::SubtitleError(format!("bad timestamp {raw}")))?;
    let m: f64 = parts[1]
        .parse()
        .map_err(|_| MediaError::SubtitleError(format!("bad timestamp {raw}")))?;
    let s: f64 = parts[2]
        .parse()
        .map_err(|_| MediaError::SubtitleError(format!("bad timestamp {raw}")))?;
    Ok(h * 3600.0 + m * 60.0 + s)
}

fn parse_srt(text: &str) -> Result<Vec<SubtitleCue>> {
    let mut cues = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 2 {
            continue;
        }
        let time_line = if lines[0].trim().chars().all(|c| c.is_ascii_digit()) {
            lines.get(1).copied().unwrap_or("")
        } else {
            lines[0]
        };
        let Some((start, end)) = time_line.split_once("-->") else {
            continue;
        };
        let (Ok(start_secs), Ok(end_secs)) =
            (parse_timestamp(start), parse_timestamp(end))
        else {
            continue;
        };
        let skip = if lines[0].trim().chars().all(|c| c.is_ascii_digit()) {
            2
        } else {
            1
        };
        let text = lines[skip..].join("\n").trim().to_string();
        if text.is_empty() {
            continue;
        }
        cues.push(SubtitleCue {
            start_secs,
            end_secs,
            text,
        });
    }
    cues.sort_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap());
    Ok(cues)
}

fn parse_vtt(text: &str) -> Result<Vec<SubtitleCue>> {
    let body = text.replace("\r\n", "\n");
    let mut lines = body.lines();
    let first = lines.next().unwrap_or("").trim().to_string();
    if !first.starts_with("WEBVTT") {
        return Err(MediaError::SubtitleError("missing WEBVTT header".into()));
    }
    let rest: String = body.lines().skip(1).collect::<Vec<_>>().join("\n");
    let mut cues = Vec::new();
    for block in rest.split("\n\n") {
        let block = block.trim();
        if block.is_empty() || block.starts_with("NOTE") || block.starts_with("STYLE") {
            continue;
        }
        let lines: Vec<&str> = block.lines().collect();
        if lines.is_empty() {
            continue;
        }
        let time_idx = lines
            .iter()
            .position(|l| l.contains("-->"))
            .unwrap_or(usize::MAX);
        if time_idx == usize::MAX {
            continue;
        }
        let Some((start, end)) = lines[time_idx].split_once("-->") else {
            continue;
        };
        let end = end.split_whitespace().next().unwrap_or("").trim();
        let (Ok(start_secs), Ok(end_secs)) = (parse_timestamp(start), parse_timestamp(end))
        else {
            continue;
        };
        let text = lines[time_idx + 1..].join("\n").trim().to_string();
        if text.is_empty() {
            continue;
        }
        cues.push(SubtitleCue {
            start_secs,
            end_secs,
            text,
        });
    }
    cues.sort_by(|a, b| a.start_secs.partial_cmp(&b.start_secs).unwrap());
    Ok(cues)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "1\n00:00:01,000 --> 00:00:04,000\nHello\n\n2\n00:00:05,000 --> 00:00:06,000\nWorld\n";
    const VTT: &str = "WEBVTT\n\n00:01.000 --> 00:04.000\nHello\n\n00:05.000 --> 00:06.000\nWorld\n";

    #[test]
    fn srt_lookup() {
        let track = SubtitleTrack::from_srt(SRT).unwrap();
        assert_eq!(track.len(), 2);
        assert_eq!(track.cue_at(2.0), Some("Hello"));
        assert_eq!(track.cue_at(4.5), None);
        assert_eq!(track.cue_at(5.5), Some("World"));
    }

    #[test]
    fn vtt_lookup() {
        let track = SubtitleTrack::from_vtt(VTT).unwrap();
        assert_eq!(track.len(), 2);
        assert_eq!(track.cue_at(2.0), Some("Hello"));
    }

    #[test]
    fn vtt_requires_header() {
        assert!(SubtitleTrack::from_vtt("00:00 --> 00:01\nx\n").is_err());
    }
}
