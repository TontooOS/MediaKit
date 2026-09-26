use serde::Deserialize;
use std::sync::OnceLock;

const EN_US: &str = include_str!("../lang/en_us.json");
const DE_DE: &str = include_str!("../lang/de_de.json");

#[derive(Deserialize)]
struct Messages {
    not_available: String,
    permission_denied: String,
    timeout: String,
    io_error: String,
    command_failed: String,
    parse_error: String,
    unsupported_format: String,
    device_not_found: String,
    stream_not_found: String,
    invalid_seek: String,
    invalid_speed: String,
    subtitle_error: String,
    network_error: String,
    ffmpeg_missing: String,
    #[serde(rename = "controls.play", default)]
    controls_play: String,
    #[serde(rename = "controls.pause", default)]
    controls_pause: String,
    #[serde(rename = "controls.stop", default)]
    controls_stop: String,
    #[serde(rename = "controls.back", default)]
    controls_back: String,
    #[serde(rename = "controls.forward", default)]
    controls_forward: String,
    #[serde(rename = "controls.volume", default)]
    controls_volume: String,
    #[serde(rename = "controls.subtitles", default)]
    controls_subtitles: String,
    #[serde(rename = "controls.chapters", default)]
    controls_chapters: String,
    #[serde(rename = "controls.fullscreen", default)]
    controls_fullscreen: String,
}

impl Messages {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            "not_available" => Some(&self.not_available),
            "permission_denied" => Some(&self.permission_denied),
            "timeout" => Some(&self.timeout),
            "io_error" => Some(&self.io_error),
            "command_failed" => Some(&self.command_failed),
            "parse_error" => Some(&self.parse_error),
            "unsupported_format" => Some(&self.unsupported_format),
            "device_not_found" => Some(&self.device_not_found),
            "stream_not_found" => Some(&self.stream_not_found),
            "invalid_seek" => Some(&self.invalid_seek),
            "invalid_speed" => Some(&self.invalid_speed),
            "subtitle_error" => Some(&self.subtitle_error),
            "network_error" => Some(&self.network_error),
            "ffmpeg_missing" => Some(&self.ffmpeg_missing),
            "controls.play" => Some(&self.controls_play),
            "controls.pause" => Some(&self.controls_pause),
            "controls.stop" => Some(&self.controls_stop),
            "controls.back" => Some(&self.controls_back),
            "controls.forward" => Some(&self.controls_forward),
            "controls.volume" => Some(&self.controls_volume),
            "controls.subtitles" => Some(&self.controls_subtitles),
            "controls.chapters" => Some(&self.controls_chapters),
            "controls.fullscreen" => Some(&self.controls_fullscreen),
            _ => None,
        }
    }
}

static MESSAGES: OnceLock<Messages> = OnceLock::new();

pub fn current_locale() -> &'static str {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if lang.to_lowercase().starts_with("de") {
        "de_de"
    } else {
        "en_us"
    }
}

fn messages() -> &'static Messages {
    MESSAGES.get_or_init(|| {
        let raw = match current_locale() {
            "de_de" => DE_DE,
            _ => EN_US,
        };
        serde_json::from_str(raw).expect("built-in language file is invalid")
    })
}

pub fn t(key: &str) -> String {
    match messages().get(key) {
        Some(msg) => msg.to_string(),
        None => key.to_string(),
    }
}

pub fn t_fmt(key: &str, arg: &str) -> String {
    t(key).replacen("{}", arg, 1)
}
