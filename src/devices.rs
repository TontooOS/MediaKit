//! Device management: capture devices + video outputs.
//!
//! Cameras delegate to `capture::list_cameras`. Outputs probe `swaymsg`
//! (TontooCompositor / Wayland) with an `xrandr` fallback; headless
//! machines return an empty list instead of an error.

use crate::capture::{list_cameras, CameraDevice};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoOutput {
    pub id: String,
    pub label: String,
    pub primary: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Available cameras / capture devices.
pub fn available_cameras() -> Result<Vec<CameraDevice>> {
    list_cameras()
}

/// Available video outputs (displays / external sinks).
pub fn available_outputs() -> Vec<VideoOutput> {
    if let Some(outputs) = outputs_from_swaymsg() {
        return outputs;
    }
    outputs_from_xrandr().unwrap_or_default()
}

fn outputs_from_swaymsg() -> Option<Vec<VideoOutput>> {
    let out = Command::new("swaymsg")
        .args(["-t", "get_outputs"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let arr = json.as_array()?;
    let mut outputs = Vec::new();
    for (i, entry) in arr.iter().enumerate() {
        let name = entry
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("output")
            .to_string();
        let active = entry
            .get("active")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if !active {
            continue;
        }
        let rect = entry.get("rect").cloned().unwrap_or_default();
        outputs.push(VideoOutput {
            id: name.clone(),
            label: name,
            primary: i == 0,
            width: rect.get("width").and_then(|v| v.as_u64()).map(|v| v as u32),
            height: rect.get("height").and_then(|v| v.as_u64()).map(|v| v as u32),
        });
    }
    Some(outputs)
}

fn outputs_from_xrandr() -> Option<Vec<VideoOutput>> {
    let out = Command::new("xrandr").arg("--query").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_xrandr(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_xrandr(text: &str) -> Vec<VideoOutput> {
    let mut outputs = Vec::new();
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let name = parts.next().unwrap_or("").to_string();
        let state = parts.next().unwrap_or("");
        if name.is_empty() || state != "connected" {
            continue;
        }
        let primary = line.contains(" primary ");
        let mut width = None;
        let mut height = None;
        for token in line.split_whitespace() {
            if let Some((w, h_rest)) = token.split_once('x') {
                let h: String = h_rest
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                    width = Some(w);
                    height = Some(h);
                    break;
                }
            }
        }
        outputs.push(VideoOutput {
            id: name.clone(),
            label: name,
            primary,
            width,
            height,
        });
    }
    outputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xrandr() {
        let text = "eDP-1 connected primary 1920x1080+0+0\nHDMI-1 disconnected\nDP-1 connected 2560x1440+1920+0\n";
        let outputs = parse_xrandr(text);
        assert_eq!(outputs.len(), 2);
        assert!(outputs[0].primary);
        assert_eq!(outputs[0].width, Some(1920));
    }

    #[test]
    fn outputs_never_panic_headless() {
        let _ = available_outputs();
        let _ = available_cameras();
    }
}
