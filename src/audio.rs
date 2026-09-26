//! Audio integration: volume / mute via AudioKit, no duplication.
//!
//! MediaKit never reimplements system volume. All calls forward to
//! `audiokit::SystemVolume` (PipeWire via pactl/wpctl). The player keeps
//! its own 0.0-1.0 UI volume; these helpers bridge to the OS mixer.

use crate::error::{MediaError, Result};

/// Current system volume in percent (0-150), or `None` when headless.
pub fn system_volume() -> Result<Option<u32>> {
    audiokit::SystemVolume::get()
        .map(Some)
        .or_else(|e| match e {
            audiokit::AudioError::NotAvailable => Ok(None),
            other => Err(MediaError::CommandFailed(other.to_string())),
        })
}

/// Sets the system volume (clamped to 0-150 inside AudioKit path).
pub fn set_system_volume(percent: u32) -> Result<()> {
    audiokit::SystemVolume::set(percent)
        .map_err(|e| MediaError::CommandFailed(e.to_string()))
}

/// Mutes (`true`) or unmutes (`false`) the default sink.
pub fn set_system_muted(muted: bool) -> Result<()> {
    audiokit::SystemVolume::set_muted(muted)
        .map_err(|e| MediaError::CommandFailed(e.to_string()))
}

/// True when the default sink is muted.
pub fn is_system_muted() -> Result<bool> {
    audiokit::SystemVolume::is_muted()
        .map_err(|e| MediaError::CommandFailed(e.to_string()))
}

/// Converts a 0.0-1.0 player volume to 0-100 system percent.
pub fn player_to_system_percent(volume: f32) -> u32 {
    (volume.clamp(0.0, 1.0) * 100.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_volume() {
        assert_eq!(player_to_system_percent(0.5), 50);
        assert_eq!(player_to_system_percent(2.0), 100);
        assert_eq!(player_to_system_percent(-1.0), 0);
    }

    #[test]
    fn system_volume_never_panics_headless() {
        let _ = system_volume();
        let _ = is_system_muted().unwrap_or(false);
    }
}
