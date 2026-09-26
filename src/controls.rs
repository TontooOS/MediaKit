//! Player controls: SF Symbols (CoreIcon) + SF Pro text (CoreText).
//!
//! Core-only descriptors so any TontooUI app renders identical controls:
//! - Icons resolve through `coreicon::resolve_icon_path` (SF Symbol PNGs).
//! - Labels use `coretext::SF_PRO_FAMILY` (`SF Pro`) from the system paths
//!   `/usr/share/fonts/OTF/SF-Pro-Display-Regular.otf` etc.
//! - Colors follow the TontooOS contract: Dark background `#1b2022` with
//!   text `#d8d9d9`, Light background `#ffffff` with text `#272727`.

use serde::{Deserialize, Serialize};

/// TontooOS Dark mode background `#1b2022` (27, 32, 34).
pub const DARK_BACKGROUND: (u8, u8, u8) = (0x1b, 0x20, 0x22);
/// TontooOS Dark mode text `#d8d9d9` (216, 217, 217).
pub const DARK_TEXT: (u8, u8, u8) = (0xd8, 0xd9, 0xd9);
/// TontooOS Light mode background `#ffffff`.
pub const LIGHT_BACKGROUND: (u8, u8, u8) = (0xff, 0xff, 0xff);
/// TontooOS Light mode text `#272727` (39, 39, 39).
pub const LIGHT_TEXT: (u8, u8, u8) = (0x27, 0x27, 0x27);

/// SF Pro family name (mirrors `coretext::SF_PRO_FAMILY`).
pub fn sf_pro_family() -> &'static str {
    coretext::SF_PRO_FAMILY
}

/// SF Symbol names for every player control (resolved via CoreIcon).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlIcon {
    Play,
    Pause,
    Stop,
    Forward,
    Backward,
    FullscreenEnter,
    FullscreenExit,
    VolumeHigh,
    VolumeMuted,
    Captions,
    Chapters,
    Record,
    Camera,
    Snapshot,
}

impl ControlIcon {
    pub fn sf_symbol(&self) -> &'static str {
        match self {
            ControlIcon::Play => "play.fill",
            ControlIcon::Pause => "pause.fill",
            ControlIcon::Stop => "stop.fill",
            ControlIcon::Forward => "goforward",
            ControlIcon::Backward => "gobackward",
            ControlIcon::FullscreenEnter => "arrow.up.left.and.arrow.down.right",
            ControlIcon::FullscreenExit => "arrow.down.right.and.arrow.up.left",
            ControlIcon::VolumeHigh => "speaker.wave.3.fill",
            ControlIcon::VolumeMuted => "speaker.slash.fill",
            ControlIcon::Captions => "captions.bubble.fill",
            ControlIcon::Chapters => "list.number",
            ControlIcon::Record => "record.circle.fill",
            ControlIcon::Camera => "video.fill",
            ControlIcon::Snapshot => "camera.fill",
        }
    }

    /// Filesystem path of the SF Symbol PNG via CoreIcon, if staged.
    pub fn icon_path(&self) -> std::path::PathBuf {
        coreicon::resolve_icon_path(self.sf_symbol())
    }
}

/// One control row entry (icon + localized label + SF Pro style).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlDescriptor {
    pub icon: ControlIcon,
    pub label_key: &'static str,
    pub font_family: &'static str,
    pub font_size: f32,
}

impl ControlDescriptor {
    pub fn new(icon: ControlIcon, label_key: &'static str, font_size: f32) -> Self {
        Self {
            icon,
            label_key,
            font_family: sf_pro_family(),
            font_size,
        }
    }
}

/// Standard transport bar (play/pause/stop/seek/fullscreen/volume/...).
pub fn transport_bar() -> Vec<ControlDescriptor> {
    vec![
        ControlDescriptor::new(ControlIcon::Backward, "controls.back", 13.0),
        ControlDescriptor::new(ControlIcon::Play, "controls.play", 15.0),
        ControlDescriptor::new(ControlIcon::Pause, "controls.pause", 15.0),
        ControlDescriptor::new(ControlIcon::Stop, "controls.stop", 13.0),
        ControlDescriptor::new(ControlIcon::Forward, "controls.forward", 13.0),
        ControlDescriptor::new(ControlIcon::VolumeHigh, "controls.volume", 13.0),
        ControlDescriptor::new(ControlIcon::Captions, "controls.subtitles", 13.0),
        ControlDescriptor::new(ControlIcon::Chapters, "controls.chapters", 13.0),
        ControlDescriptor::new(
            ControlIcon::FullscreenEnter,
            "controls.fullscreen",
            13.0,
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sf_pro_is_default() {
        assert_eq!(sf_pro_family(), "SF Pro");
        assert_eq!(transport_bar().len(), 9);
        for c in transport_bar() {
            assert_eq!(c.font_family, "SF Pro");
            assert!(!c.icon.sf_symbol().is_empty());
        }
    }

    #[test]
    fn theme_colors_match_contract() {
        assert_eq!(DARK_BACKGROUND, (0x1b, 0x20, 0x22));
        assert_eq!(DARK_TEXT, (0xd8, 0xd9, 0xd9));
        assert_eq!(LIGHT_BACKGROUND, (0xff, 0xff, 0xff));
        assert_eq!(LIGHT_TEXT, (0x27, 0x27, 0x27));
    }
}
