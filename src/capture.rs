//! Camera capture: device access, live preview info and still
//! snapshots. No external binaries for capture itself.
//!
//! Listing reads `/sys/class/video4linux` first with a `v4l2-ctl
//! --list-devices` fallback. Stills capture natively on Linux
//! (`capture_v4l2`); continuous recording needs an encoder and is
//! intentionally absent until the native encoder milestone.

use crate::error::{MediaError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

/// Lists `/dev/video*` capture devices.
///
/// sysfs (`/sys/class/video4linux`) first for friendly labels, then a
/// direct `/dev/video0..63` scan so containers without sysfs still
/// work. No external tools.
pub fn list_cameras() -> Result<Vec<CameraDevice>> {
    let mut devices = list_from_sysfs();
    if devices.is_empty() {
        devices = list_from_dev_scan();
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

/// Direct `/dev/video0..63` scan (no sysfs, no tools).
/// Labels come from sysfs when readable, else fall back to the id.
fn list_from_dev_scan() -> Vec<CameraDevice> {
    let mut out = Vec::new();
    for i in 0..64 {
        let id = format!("video{i}");
        let node = PathBuf::from(format!("/dev/{id}"));
        if !node.exists() {
            continue;
        }
        let label = std::fs::read_to_string(format!("/sys/class/video4linux/{id}/name"))
            .map(|s| s.trim().to_string())
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| id.clone());
        out.push(CameraDevice {
            id: id.clone(),
            kind: classify_label(&label),
            label,
            node,
        });
    }
    out
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

/// Takes a single still snapshot from `device` into `output` (PNG).
/// Native path on Linux via `capture_v4l2`; other platforms return
/// `NotAvailable` until their native backends land.
pub fn snapshot(device: &CameraDevice, output: &Path) -> Result<()> {
    let ext = output
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ext != "png" {
        return Err(MediaError::UnsupportedFormat(format!(
            "native snapshots write .png, got .{ext}"
        )));
    }
    #[cfg(target_os = "linux")]
    {
        let frame = crate::capture_v4l2::capture_still_native(&device.node)?;
        let png = crate::png_mini::encode_png_rgb(frame.width, frame.height, &frame.rgb)
            .map_err(MediaError::ParseError)?;
        std::fs::write(output, png).map_err(MediaError::from_io)?;
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = device;
        Err(MediaError::NotAvailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_scan_never_panics_headless() {
        // No /dev/video* on CI: empty list, no error, no tools.
        let _ = list_from_dev_scan();
        let _ = list_cameras();
    }

    #[test]
    fn quality_sizes() {
        assert_eq!(CaptureQuality::Low.size(), (640, 480));
        assert_eq!(CaptureQuality::Medium.size(), (1280, 720));
    }

    #[test]
    fn snapshot_rejects_non_png() {
        let device = CameraDevice {
            id: "video9".to_string(),
            label: "test".to_string(),
            node: PathBuf::from("/dev/video9"),
            kind: CameraKind::Unknown,
        };
        let err = snapshot(&device, Path::new("still.jpg")).unwrap_err();
        assert!(matches!(err, MediaError::UnsupportedFormat(_)));
    }

    #[test]
    fn list_never_panics_headless() {
        let _ = list_cameras();
    }
}
