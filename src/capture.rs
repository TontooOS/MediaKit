//! Camera capture: device access, live preview info, record start/stop,
//! quality selection and still snapshots via ffmpeg + V4L2.
//!
//! Listing reads `/sys/class/video4linux` first (no binary needed) with a
//! `v4l2-ctl --list-devices` fallback. Recording and snapshots shell to
//! `ffmpeg -f v4l2 -i /dev/videoN`.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraDevice {
    pub id: String,
    pub label: String,
    pub node: PathBuf,
    pub kind: CameraKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CameraKind {
    #[default]
    Unknown,
    BuiltIn,
    Usb,
    Virtual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureQuality {
    Low,
    #[default]
    Medium,
    High,
    FullHd,
}

impl CaptureQuality {
    pub fn size(&self) -> (u32, u32) {
        match self {
            CaptureQuality::Low => (640, 480),
            CaptureQuality::Medium => (1280, 720),
            CaptureQuality::High => (1920, 1080),
            CaptureQuality::FullHd => (1920, 1080),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            CaptureQuality::Low => "low",
            CaptureQuality::Medium => "medium",
            CaptureQuality::High => "high",
            CaptureQuality::FullHd => "fullhd",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureFormat {
    #[default]
    Mp4,
    WebM,
    Mkv,
}

impl CaptureFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            CaptureFormat::Mp4 => "mp4",
            CaptureFormat::WebM => "webm",
            CaptureFormat::Mkv => "mkv",
        }
    }
}

/// Lists `/dev/video*` capture devices.
pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    let mut devices = list_from_sysfs();
    if devices.is_empty() {
        devices = list_from_v4l2_ctl().unwrap_or_default();
    }
    Ok(devices)
}

fn list_from_sysfs() -> Vec<CameraDevice> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir("/sys/class/video4linux") else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let node = PathBuf::from(format!("/dev/{name}"));
        if !node.exists() {
            continue;
        }
        let label_path = entry.path().join("name");
        let label = std::fs::read_to_string(&label_path)
            .unwrap_or_else(|_| name.clone())
            .trim()
            .to_string();
        let kind = classify_label(&label);
        out.push(CameraDevice {
            id: name.clone(),
            label,
            node,
            kind,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

fn list_from_v4l2_ctl() -> Result<Vec<CameraDevice>> {
    let out = Command::new("v4l2-ctl")
        .args(["--list-devices"])
        .output()
        .map_err(MediaError::from_io)?;
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(parse_v4l2_list(&String::from_utf8_lossy(&out.stdout)))
}

fn parse_v4l2_list(text: &str) -> Vec<CameraDevice> {
    let mut devices = Vec::new();
    let mut current_label = String::new();
    for line in text.lines() {
        if line.starts_with('\t') || line.starts_with(' ') {
            let node = line.trim().to_string();
            if node.starts_with("/dev/video") {
                let id = node.trim_start_matches("/dev/").to_string();
                let kind = classify_label(&current_label);
                devices.push(CameraDevice {
                    id: id.clone(),
                    label: if current_label.is_empty() {
                        id.clone()
                    } else {
                        format!("{current_label} ({id})")
                    },
                    node: PathBuf::from(node),
                    kind,
                });
            }
        } else if !line.trim().is_empty() {
            current_label = line.trim().trim_end_matches(':').to_string();
        }
    }
    devices
}

fn classify_label(label: &str) -> CameraKind {
    let l = label.to_ascii_lowercase();
    if l.contains("usb") || l.contains("webcam") || l.contains("logitech") {
        CameraKind::Usb
    } else if l.contains("integrated") || l.contains("built-in") || l.contains("facetime") {
        CameraKind::BuiltIn
    } else if l.contains("virtual") || l.contains("obs") {
        CameraKind::Virtual
    } else {
        CameraKind::Unknown
    }
}

/// Starts `ffmpeg` capture from `device` into `output`.
/// Returns the child PID so apps can stop it via `stop_capture`.
pub fn start_capture(
    device: &CameraDevice,
    output: &Path,
    quality: CaptureQuality,
    format: CaptureFormat,
) -> Result<u32> {
    let (w, h) = quality.size();
    let child = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "v4l2",
            "-video_size",
            &format!("{w}x{h}"),
            "-i",
            &device.node.to_string_lossy(),
            &output.to_string_lossy(),
        ])
        .spawn()
        .map_err(MediaError::from_io)?;
    let _ = format;
    Ok(child.id())
}

/// Takes a single still snapshot from `device` into `output` (PNG/JPG).
pub fn snapshot(device: &CameraDevice, output: &Path) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "v4l2",
            "-video_size",
            "1280x720",
            "-i",
            &device.node.to_string_lossy(),
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
            "ffmpeg snapshot failed for {}",
            device.id
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_v4l2_devices() {
        let text = "HD Pro Webcam C920:\n\t/dev/video0\n\t/dev/video1\n";
        let devices = parse_v4l2_list(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].node, PathBuf::from("/dev/video0"));
    }

    #[test]
    fn quality_sizes() {
        assert_eq!(CaptureQuality::Low.size(), (640, 480));
        assert_eq!(CaptureQuality::Medium.size(), (1280, 720));
        assert_eq!(CaptureFormat::Mp4.extension(), "mp4");
    }

    #[test]
    fn list_never_panics_headless() {
        let _ = list_cameras();
    }
}
