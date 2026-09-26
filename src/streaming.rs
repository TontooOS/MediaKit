//! Network streaming: HTTP progressive + HLS via NetworkKit.
//!
//! All HTTP goes through `networkkit::http` (single stack, no second
//! client). HLS (`*.m3u8`) playlists are parsed pure-Rust; segments are
//! fetched with the same client. Buffering state is reported as events so
//! player UIs can show loading spinners.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum StreamKind {
    #[default]
    Progressive,
    Hls,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BufferState {
    #[default]
    Idle,
    Buffering,
    Ready,
    Stalled,
    Finished,
    Failed,
}

#[derive(Debug, Clone)]
pub struct StreamEvent {
    pub state: BufferState,
    pub buffered_secs: f64,
    pub message: String,
}

impl StreamEvent {
    pub fn new(state: BufferState, buffered_secs: f64, message: &str) -> Self {
        Self {
            state,
            buffered_secs,
            message: message.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct HlsPlaylist {
    pub segments: Vec<String>,
    pub target_duration_secs: f64,
    pub live: bool,
}

impl HlsPlaylist {
    pub fn parse(base_url: &str, text: &str) -> Result<Self> {
        if !text.contains("#EXTM3U") {
            return Err(MediaError::ParseError("missing #EXTM3U header".into()));
        }
        let mut segments = Vec::new();
        let mut target_duration_secs = 0.0;
        let mut live = true;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("#EXT-X-TARGETDURATION:") {
                target_duration_secs = line
                    .trim_start_matches("#EXT-X-TARGETDURATION:")
                    .trim()
                    .parse()
                    .unwrap_or(0.0);
            } else if line == "#EXT-X-ENDLIST" {
                live = false;
            } else if line.is_empty() || line.starts_with('#') {
                continue;
            } else {
                segments.push(join_url(base_url, line));
            }
        }
        if segments.is_empty() {
            return Err(MediaError::ParseError("no HLS segments found".into()));
        }
        Ok(Self {
            segments,
            target_duration_secs,
            live,
        })
    }
}

fn join_url(base: &str, part: &str) -> String {
    if part.starts_with("http://") || part.starts_with("https://") {
        return part.to_string();
    }
    let base = base.trim_end_matches('/');
    let part = part.trim_start_matches('/');
    format!("{base}/{part}")
}

/// Detects stream kind from URL (`.m3u8` => HLS, else progressive).
pub fn detect_kind(url: &str) -> StreamKind {
    let lower = url.to_ascii_lowercase();
    let path = lower.split(['?', '#']).next().unwrap_or("");
    if path.ends_with(".m3u8") {
        StreamKind::Hls
    } else {
        StreamKind::Progressive
    }
}

/// Blocking fetch of a progressive stream head (status + content length).
/// Uses NetworkKit so apps share one HTTP stack.
pub fn fetch_progressive(url: &str) -> Result<(u16, Option<u64>)> {
    validate_http_url(url)?;
    let resp =
        networkkit::http::get(url).map_err(|e| MediaError::NetworkError(e.to_string()))?;
    let len = resp
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok());
    Ok((resp.status, len))
}

/// Blocking fetch + parse of an HLS playlist via NetworkKit.
pub fn fetch_hls_playlist(url: &str) -> Result<HlsPlaylist> {
    validate_http_url(url)?;
    let resp =
        networkkit::http::get(url).map_err(|e| MediaError::NetworkError(e.to_string()))?;
    if !(200..300).contains(&resp.status) {
        return Err(MediaError::NetworkError(format!("HTTP {}", resp.status)));
    }
    let text = resp
        .text()
        .map_err(|e| MediaError::NetworkError(e.to_string()))?;
    HlsPlaylist::parse(url, &text)
}

/// Downloads one HLS segment to memory via NetworkKit.
pub fn fetch_segment(url: &str) -> Result<Vec<u8>> {
    validate_http_url(url)?;
    let resp =
        networkkit::http::get(url).map_err(|e| MediaError::NetworkError(e.to_string()))?;
    if !(200..300).contains(&resp.status) {
        return Err(MediaError::NetworkError(format!("HTTP {}", resp.status)));
    }
    Ok(resp.body)
}

fn validate_http_url(url: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(())
    } else {
        Err(MediaError::NetworkError(format!("invalid URL: {url}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST: &str = "#EXTM3U\n#EXT-X-TARGETDURATION:6\n#EXTINF:6.0,\nseg0.ts\n#EXTINF:6.0,\nseg1.ts\n#EXT-X-ENDLIST\n";

    #[test]
    fn detects_hls() {
        assert_eq!(detect_kind("https://cdn/x/stream.m3u8"), StreamKind::Hls);
        assert_eq!(
            detect_kind("https://cdn/x/clip.mp4"),
            StreamKind::Progressive
        );
    }

    #[test]
    fn parses_playlist() {
        let list =
            HlsPlaylist::parse("https://cdn/x/hls", PLAYLIST).unwrap();
        assert_eq!(list.segments.len(), 2);
        assert_eq!(list.segments[0], "https://cdn/x/hls/seg0.ts");
        assert!(!list.live);
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(fetch_progressive("ftp://x/y").is_err());
        assert!(HlsPlaylist::parse("https://x", "nope").is_err());
    }
}
