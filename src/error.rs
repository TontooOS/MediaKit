use crate::lang;
use std::fmt;
use std::io;

#[derive(Debug)]
pub enum MediaError {
    NotAvailable,
    PermissionDenied,
    Timeout,
    IoError(String),
    CommandFailed(String),
    ParseError(String),
    UnsupportedFormat(String),
    DeviceNotFound(String),
    StreamNotFound(u64),
    InvalidSeek(String),
    InvalidSpeed(f32),
    SubtitleError(String),
    NetworkError(String),
    FfmpegMissing,
}

impl MediaError {
    pub fn from_io(err: io::Error) -> Self {
        match err.kind() {
            io::ErrorKind::PermissionDenied => MediaError::PermissionDenied,
            io::ErrorKind::TimedOut => MediaError::Timeout,
            io::ErrorKind::NotFound => MediaError::FfmpegMissing,
            _ => MediaError::IoError(err.to_string()),
        }
    }
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MediaError::NotAvailable => write!(f, "{}", lang::t("not_available")),
            MediaError::PermissionDenied => write!(f, "{}", lang::t("permission_denied")),
            MediaError::Timeout => write!(f, "{}", lang::t("timeout")),
            MediaError::IoError(e) => write!(f, "{}", lang::t_fmt("io_error", e)),
            MediaError::CommandFailed(c) => write!(f, "{}", lang::t_fmt("command_failed", c)),
            MediaError::ParseError(e) => write!(f, "{}", lang::t_fmt("parse_error", e)),
            MediaError::UnsupportedFormat(e) => {
                write!(f, "{}", lang::t_fmt("unsupported_format", e))
            }
            MediaError::DeviceNotFound(e) => {
                write!(f, "{}", lang::t_fmt("device_not_found", e))
            }
            MediaError::StreamNotFound(id) => {
                write!(f, "{}", lang::t_fmt("stream_not_found", &id.to_string()))
            }
            MediaError::InvalidSeek(e) => write!(f, "{}", lang::t_fmt("invalid_seek", e)),
            MediaError::InvalidSpeed(v) => {
                write!(f, "{}", lang::t_fmt("invalid_speed", &v.to_string()))
            }
            MediaError::SubtitleError(e) => write!(f, "{}", lang::t_fmt("subtitle_error", e)),
            MediaError::NetworkError(e) => write!(f, "{}", lang::t_fmt("network_error", e)),
            MediaError::FfmpegMissing => write!(f, "{}", lang::t("ffmpeg_missing")),
        }
    }
}

impl std::error::Error for MediaError {}

impl From<io::Error> for MediaError {
    fn from(err: io::Error) -> Self {
        MediaError::from_io(err)
    }
}

pub type Result<T> = std::result::Result<T, MediaError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_resolve() {
        assert_eq!(
            MediaError::StreamNotFound(3).to_string(),
            lang::t_fmt("stream_not_found", "3")
        );
        assert_eq!(
            MediaError::InvalidSpeed(3.0).to_string(),
            lang::t_fmt("invalid_speed", "3")
        );
    }

    #[test]
    fn maps_not_found_to_ffmpeg_missing() {
        let err = MediaError::from_io(io::Error::new(io::ErrorKind::NotFound, "nope"));
        assert!(matches!(err, MediaError::FfmpegMissing));
    }
}
