pub mod audio;
pub mod capture;
pub mod chapters;
pub mod controls;
pub mod devices;
pub mod edit;
pub mod error;
pub mod format;
pub mod lang;
pub mod metadata;
pub mod mkv;
pub mod mov;
pub mod playback;
pub mod prores;
pub mod prores_blocks;
pub mod prores_frame;
pub mod streaming;
pub mod subtitles;
pub mod thumbnails;

pub use audio::{is_system_muted, set_system_muted, set_system_volume, system_volume};
pub use capture::{list_cameras, snapshot, start_capture, CameraDevice, CameraKind, CaptureFormat, CaptureQuality};
pub use chapters::{read_chapters, Chapter, ChapterList};
pub use controls::{transport_bar, ControlDescriptor, ControlIcon};
pub use devices::{available_cameras, available_outputs, VideoOutput};
pub use edit::{concat, transcode, transcode_prores, trim, trim_mov_native, ExportPreset};
pub use error::{MediaError, Result};
pub use format::{is_supported, probe_container, probe_container_native, VideoContainer, MOV_EXTENSIONS, SUPPORTED_EXTENSIONS};
pub use metadata::{read_metadata, VideoMetadata};
pub use mkv::{is_mkv_extension, read_mkv_info, read_mkv_metadata, sniff_mkv, MkvInfo};
pub use mov::{build_raw_mov, is_mov_extension, read_mov_info, read_mov_metadata, sniff_mov, MovInfo, RawMovParams};
pub use playback::{DisplayMode, PlaybackState, VideoPlayer, MAX_SPEED, MIN_SPEED};
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
pub use thumbnails::{extract_args, extract_frame};

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
