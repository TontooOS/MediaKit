//! File playback: Play / Pause / Stop / Seek + speed + fullscreen flag.
//!
//! Core-only design: `VideoPlayer` owns transport state (position, speed,
//! subtitles, chapters) and optionally spawns `mpv` (preferred) or
//! `ffplay` for actual output. All state transitions are pure Rust and
//! headless-testable; missing player binaries surface as
//! `MediaError::NotAvailable` only when `spawn_external` is used.

use crate::chapters::ChapterList;
use crate::error::{MediaError, Result};
use crate::format::probe_container;
use crate::subtitles::SubtitleTrack;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

pub const MIN_SPEED: f32 = 0.5;
pub const MAX_SPEED: f32 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Playing,
    Paused,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayMode {
    #[default]
    Windowed,
    Fullscreen,
}

#[derive(Debug, Clone)]
pub struct VideoPlayer {
    path: Option<PathBuf>,
    state: PlaybackState,
    position_secs: f64,
    duration_secs: f64,
    speed: f32,
    volume: f32,
    muted: bool,
    display: DisplayMode,
    subtitles: Option<SubtitleTrack>,
    chapters: ChapterList,
    child: Option<u32>,
}

impl Default for VideoPlayer {
    fn default() -> Self {
        Self {
            path: None,
            state: PlaybackState::Stopped,
            position_secs: 0.0,
            duration_secs: 0.0,
            speed: 1.0,
            volume: 1.0,
            muted: false,
            display: DisplayMode::Windowed,
            subtitles: None,
            chapters: ChapterList::default(),
            child: None,
        }
    }
}

impl VideoPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens `path` for playback (validates container, resets position).
    pub fn open(&mut self, path: &Path) -> Result<()> {
        probe_container(path)?;
        if !path.exists() {
            return Err(MediaError::IoError(format!(
                "file not found: {}",
                path.to_string_lossy()
            )));
        }
        self.path = Some(path.to_path_buf());
        self.state = PlaybackState::Stopped;
        self.position_secs = 0.0;
        self.duration_secs = crate::metadata::read_metadata(path)
            .map(|m| m.duration_secs)
            .unwrap_or(0.0);
        self.child = None;
        Ok(())
    }

    pub fn play(&mut self) -> Result<()> {
        if self.path.is_none() {
            return Err(MediaError::NotAvailable);
        }
        self.state = PlaybackState::Playing;
        Ok(())
    }

    pub fn pause(&mut self) -> Result<()> {
        if self.state != PlaybackState::Playing {
            return Ok(());
        }
        self.state = PlaybackState::Paused;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.state = PlaybackState::Stopped;
        self.position_secs = 0.0;
        self.kill_external();
        Ok(())
    }

    /// Seeks to `position_secs` (clamped to duration when known).
    pub fn seek(&mut self, position_secs: f64) -> Result<()> {
        if position_secs < 0.0 || !position_secs.is_finite() {
            return Err(MediaError::InvalidSeek(position_secs.to_string()));
        }
        if self.duration_secs > 0.0 && position_secs > self.duration_secs {
            return Err(MediaError::InvalidSeek(position_secs.to_string()));
        }
        self.position_secs = position_secs;
        if self.duration_secs > 0.0 && position_secs >= self.duration_secs {
            self.state = PlaybackState::Finished;
        }
        Ok(())
    }

    /// Sets playback speed, valid range 0.5x-2.0x.
    pub fn set_speed(&mut self, speed: f32) -> Result<()> {
        if !(MIN_SPEED..=MAX_SPEED).contains(&speed) || !speed.is_finite() {
            return Err(MediaError::InvalidSpeed(speed));
        }
        self.speed = speed;
        Ok(())
    }

    /// Volume 0.0-1.0. Delegated to AudioKit by apps; stored here for UI.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
    }

    /// Mute flag. Delegated to AudioKit by apps; stored here for UI.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn toggle_fullscreen(&mut self) -> DisplayMode {
        self.display = match self.display {
            DisplayMode::Windowed => DisplayMode::Fullscreen,
            DisplayMode::Fullscreen => DisplayMode::Windowed,
        };
        self.display
    }

    pub fn set_subtitles(&mut self, track: Option<SubtitleTrack>) {
        self.subtitles = track;
    }

    pub fn set_chapters(&mut self, chapters: ChapterList) {
        self.chapters = chapters;
    }

    /// Active subtitle text at the current position, if any.
    pub fn active_subtitle(&self) -> Option<&str> {
        self.subtitles
            .as_ref()
            .and_then(|t| t.cue_at(self.position_secs))
    }

    /// Advances the clock by `delta_secs * speed` while playing.
    pub fn tick(&mut self, delta_secs: f64) {
        if self.state != PlaybackState::Playing {
            return;
        }
        self.position_secs += delta_secs * self.speed as f64;
        if self.duration_secs > 0.0 && self.position_secs >= self.duration_secs {
            self.position_secs = self.duration_secs;
            self.state = PlaybackState::Finished;
        }
    }

    /// Spawns `mpv` (or `ffplay` fallback) for real output. Optional;
    /// headless tests never call this.
    pub fn spawn_external(&mut self) -> Result<()> {
        let path = self.path.clone().ok_or(MediaError::NotAvailable)?;
        let attempt = try_spawn(&path, self.speed, self.display);
        match attempt {
            Ok(child) => {
                self.child = Some(child.id());
                std::mem::forget(child);
                self.state = PlaybackState::Playing;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    fn kill_external(&mut self) {
        self.child = None;
    }

    pub fn state(&self) -> PlaybackState {
        self.state
    }
    pub fn position_secs(&self) -> f64 {
        self.position_secs
    }
    pub fn duration_secs(&self) -> f64 {
        self.duration_secs
    }
    pub fn speed(&self) -> f32 {
        self.speed
    }
    pub fn volume(&self) -> f32 {
        self.volume
    }
    pub fn is_muted(&self) -> bool {
        self.muted
    }
    pub fn display(&self) -> DisplayMode {
        self.display
    }
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }
    pub fn chapters(&self) -> &ChapterList {
        &self.chapters
    }
}

fn try_spawn(path: &Path, speed: f32, display: DisplayMode) -> Result<Child> {
    let arg = path.to_string_lossy().to_string();
    let fs = matches!(display, DisplayMode::Fullscreen);
    if let Ok(child) = Command::new("mpv")
        .args([
            arg.clone(),
            format!("--speed={}", speed),
            if fs { "--fs".to_string() } else { "--no-fs".to_string() },
        ])
        .spawn()
    {
        return Ok(child);
    }
    if let Ok(child) = Command::new("ffplay")
        .args(["-autoexit", "-noborder", &arg])
        .spawn()
    {
        return Ok(child);
    }
    Err(MediaError::NotAvailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_range_enforced() {
        let mut p = VideoPlayer::new();
        assert!(p.set_speed(1.5).is_ok());
        assert!(p.set_speed(0.1).is_err());
        assert!(p.set_speed(2.5).is_err());
    }

    #[test]
    fn seek_and_tick() {
        let mut p = VideoPlayer::new();
        p.duration_secs = 100.0;
        p.path = Some(PathBuf::from("clip.mp4"));
        p.play().unwrap();
        p.seek(10.0).unwrap();
        p.tick(5.0);
        assert!((p.position_secs() - 15.0).abs() < 0.001);
        p.set_speed(2.0).unwrap();
        p.tick(5.0);
        assert!((p.position_secs() - 25.0).abs() < 0.001);
        assert!(p.seek(-1.0).is_err());
        assert!(p.seek(200.0).is_err());
    }

    #[test]
    fn fullscreen_toggles() {
        let mut p = VideoPlayer::new();
        assert_eq!(p.toggle_fullscreen(), DisplayMode::Fullscreen);
        assert_eq!(p.toggle_fullscreen(), DisplayMode::Windowed);
    }
}
