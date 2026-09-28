pub mod audio;
pub mod avi;
pub mod capture;
#[cfg(target_os = "linux")]
pub mod capture_v4l2;
pub mod chapters;
pub mod controls;
pub mod devices;
pub mod edit;
pub mod error;
pub mod format;
pub mod lang;
pub mod metadata;
pub mod mjpeg;
pub mod mkv;
pub mod mov;
pub mod playback;
pub mod png_mini;
pub mod prores;
pub mod prores_blocks;
pub mod prores_frame;
pub mod streaming;
pub mod subtitles;
pub mod thumbnails;

pub use audio::{is_system_muted, set_system_muted, set_system_volume, system_volume};
pub use avi::{avi_to_metadata, is_avi_extension, read_avi_chunks, read_avi_info, read_avi_metadata, sniff_avi, AviChunk, AviInfo};
pub use capture::{list_cameras, snapshot, CameraDevice, CameraKind, CaptureQuality};
#[cfg(target_os = "linux")]
pub use capture_v4l2::{
    capture_frame_native, capture_still_native, yuyv_to_rgb, CapturedFrame, FOURCC_MJPG,
    FOURCC_YUYV,
};
pub use chapters::{read_chapters, Chapter, ChapterList};
pub use controls::{transport_bar, ControlDescriptor, ControlIcon};
pub use devices::{available_cameras, available_outputs, VideoOutput};
pub use edit::{concat_mov_native, trim_mov_native};
pub use error::{MediaError, Result};
pub use format::{is_supported, probe_container, probe_container_native, VideoContainer, MOV_EXTENSIONS, SUPPORTED_EXTENSIONS};
pub use metadata::{read_metadata, VideoMetadata};
pub use mjpeg::{decode_jpeg, encode_jpeg_fixture, JpegError, JpegImage, JpegSampling};
pub use mkv::{is_mkv_extension, read_mkv_info, read_mkv_metadata, sniff_mkv, MkvInfo};
pub use mov::{build_raw_mov, is_mov_extension, read_mov_info, read_mov_metadata, read_mov_samples, sniff_mov, MovInfo, MovSample, RawAudioParams, RawMovParams};
pub use playback::{DisplayMode, PlaybackState, VideoPlayer, MAX_SPEED, MIN_SPEED};
pub use png_mini::{adler32, crc32, decode_png_stored, encode_png_rgb};
pub use prores::{
    detect_profile_from_mov, gpu_slice_plan, is_prores_fourcc, parse_frame_header,
    profile_from_fourcc, ProResChroma, ProResFrameHeader, ProResProfile, PRORES_FOURCCS,
};
pub use prores_blocks::{
    decode_block, dequantize_block, dct_8x8, encode_block, idct_8x8, quantize_block,
    roundtrip_block, BitReader, BitWriter, UNIFORM_QM, ZIGZAG,
};
pub use prores_frame::{
    decode_frame, encode_frame_fixture, slice_bands, yuv_to_rgb, DecodeError, YuvFrame,
    K_AC, K_DC, MAX_QUANT,
};
pub use streaming::{detect_kind, fetch_hls_playlist, fetch_progressive, fetch_segment, BufferState, HlsPlaylist, StreamEvent, StreamKind};
pub use subtitles::{SubtitleCue, SubtitleTrack};
pub use thumbnails::{decode_video_frame, extract_frame_native, NativeFrame};

/// Shared handle to the video domains (mirrors NetworkKit/AudioKit facades).
#[derive(Debug, Default)]
pub struct MediaKit {
    pub player: VideoPlayer,
}

impl MediaKit {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn player_mut(&mut self) -> &mut VideoPlayer {
        &mut self.player
    }

    pub fn player(&self) -> &VideoPlayer {
        &self.player
    }
}

/// Library version string.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

mod ffi;
