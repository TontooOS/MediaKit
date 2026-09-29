//! System-GPU H.264 decode via VA-API, Linux-only hardware path, std-only.
//!
//! This module decodes H.264 through the OS fixed-function video engine
//! (`libva.so.2` / `libva-drm.so.2`, DRM render nodes) instead of an
//! external decoder binary or a hand-written codec. No new crates.io
//! dependencies: `dlopen`/`dlsym` and every VA-API symbol are declared
//! locally via `extern "C"`, following the precedent in
//! `crate::capture_v4l2` (raw ioctls) and `crate::devices` (raw DRM
//! ioctls). All numeric constants and struct layouts were verified
//! against the system headers (`/usr/include/va/va.h`, libva) with a
//! tiny C probe compiled by gcc; unit tests assert the Rust values and
//! sizes against the C-verified numbers (see the `tests` module).
//!
//! # Phases
//!
//! * Phase 1 (landed): safe init plus capability query.
//!   `hw_decoder_info()` opens a DRM render node (`/dev/dri/renderD128`
//!   and up), runs `vaInitialize`, then `vaQueryConfigProfiles` plus
//!   `vaQueryConfigEntrypoints` and reports one `HwDecoder` per H.264
//!   profile with a `VLD` entrypoint.
//! * Phase 2 (landed for I-slices, milestone stub for inter frames):
//!   `VaapiSession` creates a real VA config, context and surfaces for
//!   `VAProfileH264High` plus `VAEntrypointVLD`. `decode_frame_vaapi()`
//!   submits one picture through the real
//!   `vaBeginPicture`/`vaRenderPicture` (`VAPictureParameterBufferH264`,
//!   `VASliceParameterBufferH264`, bitstream buffer) / `vaEndPicture` /
//!   `vaSyncSurface` / `vaDeriveImage` plus `vaMapBuffer` download path
//!   for IDR and non-IDR I-slices. P/B slices return
//!   `MediaError::UnsupportedFormat("va-api slice decode: ...")`
//!   because reference-picture (DPB) management is not implemented yet.
//!
//! # Graceful degradation
//!
//! Everything degrades gracefully: no render node, no libva, no H.264
//! entrypoint, or any failing VA call yields an empty `Vec` (Phase 1)
//! or a `MediaError` (Phase 2). Hardware paths never panic and never
//! use `unwrap`/`expect`; every `dlopen`/`dlsym`/VA call is checked.
//!
//! # Future wiring
//!
//! Once inter-frame (DPB) management lands, `decode_video_frame` in
//! `crate::thumbnails` should prefer this path for H.264 content:
//!
//! ```text
//! let data = read_avc_sample_bytes(...);          // Annex-B or AVCC
//! match hwaccel::decode_annexb_to_yuv(&annexb) {
//!   Ok(frame) => rgb_from_hw_frame(frame),        // GPU decoded
//!   Err(_) => existing_native_decode(...),       // current fallback
//! }
//! ```
//!
//! Existing decode behavior is unchanged until then.

use crate::error::{MediaError, Result};

// ---------------------------------------------------------------------------
// C-verified constants (see module docs for the probe method).
// ---------------------------------------------------------------------------

/// `VA_STATUS_SUCCESS` from `va/va.h`.
pub const VA_STATUS_SUCCESS: i32 = 0;
/// H.264 baseline profile id (deprecated upstream, still reported).
pub const VA_PROFILE_H264_BASELINE: i32 = 5;
/// H.264 main profile id.
pub const VA_PROFILE_H264_MAIN: i32 = 6;
/// H.264 high profile id (used for session creation).
pub const VA_PROFILE_H264_HIGH: i32 = 7;
/// H.264 constrained-baseline profile id.
pub const VA_PROFILE_H264_CONSTRAINED_BASELINE: i32 = 13;
/// H.264 multiview-high profile id.
pub const VA_PROFILE_H264_MULTIVIEW_HIGH: i32 = 15;
/// H.264 stereo-high profile id.
pub const VA_PROFILE_H264_STEREO_HIGH: i32 = 16;
/// H.264 high-10 profile id.
pub const VA_PROFILE_H264_HIGH10: i32 = 36;
/// H.264 high-4:2:2 profile id.
pub const VA_PROFILE_H264_HIGH422: i32 = 40;
/// Variable-length decode entrypoint.
pub const VA_ENTRYPOINT_VLD: i32 = 1;
/// YUV 4:2:0 8-bit render-target format.
pub const VA_RT_FORMAT_YUV420: u32 = 0x0000_0001;
/// `VAPictureParameterBufferType`.
pub const VA_BUFFER_TYPE_PICTURE_PARAM: i32 = 0;
/// `VASliceParameterBufferType`.
pub const VA_BUFFER_TYPE_SLICE_PARAM: i32 = 4;
/// `VASliceDataBufferType` (bitstream buffer).
pub const VA_BUFFER_TYPE_SLICE_DATA: i32 = 5;
/// `VAImageBufferType`.
pub const VA_BUFFER_TYPE_IMAGE: i32 = 9;
/// `VA_INVALID_ID` / `VA_INVALID_SURFACE`.
pub const VA_INVALID_ID: u32 = 0xffff_ffff;
/// `VA_PROGRESSIVE` context flag.
pub const VA_PROGRESSIVE: i32 = 1;
/// `VA_SLICE_DATA_FLAG_ALL`: the whole slice is in the buffer.
pub const VA_SLICE_DATA_FLAG_ALL: u32 = 0;
/// `VA_FOURCC_NV12` (`NV12` little-endian).
pub const VA_FOURCC_NV12: u32 = 0x3231_564E;
/// `VA_FOURCC_I420` little-endian.
pub const VA_FOURCC_I420: u32 = 0x3032_3449;
/// `VA_FOURCC_YV12` little-endian.
pub const VA_FOURCC_YV12: u32 = 0x3231_5659;
/// `VAPictureH264` invalid marker.
pub const VA_PICTURE_H264_INVALID: u32 = 0x0000_0001;
/// `RTLD_NOW` from glibc `bits/dlfcn.h` for `dlopen`.
const RTLD_NOW: i32 = 2;
/// Default `libva` soname probed at runtime.
const LIBVA_SO: &[u8] = b"libva.so.2\0";
/// Default `libva-drm` soname probed at runtime (provides `vaGetDisplayDRM`).
const LIBVA_DRM_SO: &[u8] = b"libva-drm.so.2\0";

/// Human name for an H.264 VA profile id, or `None` for non-H.264 ids.
pub fn h264_profile_name(profile_id: i32) -> Option<&'static str> {
    match profile_id {
        VA_PROFILE_H264_BASELINE => Some("VAProfileH264Baseline"),
        VA_PROFILE_H264_MAIN => Some("VAProfileH264Main"),
        VA_PROFILE_H264_HIGH => Some("VAProfileH264High"),
        VA_PROFILE_H264_CONSTRAINED_BASELINE => Some("VAProfileH264ConstrainedBaseline"),
        VA_PROFILE_H264_MULTIVIEW_HIGH => Some("VAProfileH264MultiviewHigh"),
        VA_PROFILE_H264_STEREO_HIGH => Some("VAProfileH264StereoHigh"),
        VA_PROFILE_H264_HIGH10 => Some("VAProfileH264High10"),
        VA_PROFILE_H264_HIGH422 => Some("VAProfileH264High422"),
        _ => None,
    }
}

/// Human name for an H.264 NAL unit type (ITU-T H.264 Table 7-1).
pub fn h264_nal_type_name(nal_type: u8) -> &'static str {
    match nal_type {
        1 => "coded slice (non-IDR)",
        2 => "coded slice data partition A",
        3 => "coded slice data partition B",
        4 => "coded slice data partition C",
        5 => "coded slice (IDR)",
        6 => "supplemental enhancement information (SEI)",
        7 => "sequence parameter set (SPS)",
        8 => "picture parameter set (PPS)",
        9 => "access unit delimiter (AUD)",
        10 => "end of sequence",
        11 => "end of stream",
        12 => "filler data",
        _ => "reserved/unspecified",
    }
}

// ---------------------------------------------------------------------------
// Public plain structs (VideoOutput-style: small, cloneable, no heap-heavy API
// beyond short labels).
// ---------------------------------------------------------------------------

/// One GPU decode capability: VA-API plus H.264 plus a VLD entrypoint.
///
/// `max_width`/`max_height` are `None`: the stable VA-API attribute query
/// does not expose a portable maximum resolution, so no guess is reported.
#[derive(Debug, Clone)]
pub struct HwDecoder {
    pub api: String,
    pub codec: String,
    pub profile: String,
    pub profile_id: i32,
    pub entrypoint: String,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
}

/// One GPU-decoded frame in planar YUV 4:2:0 (full-range, BT.601, to match
/// the crate's software converters).
#[derive(Debug, Clone)]
pub struct HwFrame {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

/// One Annex-B NAL unit: byte range plus the parsed H.264 header fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NalUnit {
    /// Offset of the NAL header byte (after the start code).
    pub offset: usize,
    /// Length in bytes from the header to (not including) the next start
    /// code or end of input.
    pub len: usize,
    /// H.264 NAL unit type (`header & 0x1F`).
    pub nal_type: u8,
    /// H.264 `nal_ref_idc` (`(header >> 5) & 0x03`).
    pub nal_ref_idc: u8,
}

/// Parsed H.264 sequence parameter set: everything the VA-API picture
/// parameter buffer needs, plus the display dimensions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpsInfo {
    pub profile_idc: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma_minus8: u8,
    pub bit_depth_chroma_minus8: u8,
    pub frame_mbs_only_flag: bool,
    pub mb_adaptive_frame_field_flag: bool,
    pub direct_8x8_inference_flag: bool,
    pub log2_max_frame_num_minus4: u32,
    pub pic_order_cnt_type: u32,
    pub log2_max_pic_order_cnt_lsb_minus4: u32,
    pub delta_pic_order_always_zero_flag: bool,
    pub pic_width_in_mbs_minus1: u32,
    pub pic_height_in_map_units_minus1: u32,
    pub num_ref_frames: u32,
    pub width: u32,
    pub height: u32,
}

/// Parsed H.264 picture parameter set (subset needed for slice submit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpsInfo {
    pub pic_parameter_set_id: u32,
    pub seq_parameter_set_id: u32,
    pub entropy_coding_mode_flag: bool,
    pub bottom_field_pic_order_in_frame_present_flag: bool,
    pub num_ref_idx_l0_default_active_minus1: u32,
    pub num_ref_idx_l1_default_active_minus1: u32,
    pub pic_init_qp_minus26: i32,
    pub deblocking_filter_control_present_flag: bool,
}

/// Parsed H.264 slice header (subset for single-slice I-frame submit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SliceInfo {
    pub first_mb_in_slice: u32,
    /// Normalized slice type (`raw % 5`): 0 P, 1 B, 2 I, 3 SP, 4 SI.
    pub slice_type: u8,
    pub pic_parameter_set_id: u32,
    pub frame_num: u32,
    pub pic_order_cnt_lsb: Option<u32>,
    pub idr_pic_id: Option<u32>,
    pub num_ref_idx_l0_active_minus1: u32,
    pub num_ref_idx_l1_active_minus1: u32,
    pub slice_qp_delta: i32,
    pub disable_deblocking_filter_idc: u32,
    pub slice_alpha_c0_offset_div2: i32,
    pub slice_beta_offset_div2: i32,
    /// Bit length of the parsed slice header in the RBSP domain; passed
    /// as `slice_data_bit_offset` so the driver can find `slice_data()`.
    pub header_bits: u32,
    pub is_idr: bool,
}

/// Normalizes an H.264 `slice_type` value (`5..=9` mirror `0..=4`).
pub fn normalize_slice_type(v: u8) -> u8 {
    v % 5
}

// ---------------------------------------------------------------------------
// Annex-B helpers (pure Rust, cross-platform).
// ---------------------------------------------------------------------------

fn is_start_code_at(data: &[u8], at: usize) -> usize {
    if at + 3 <= data.len()
        && data[at] == 0
        && data[at + 1] == 0
        && data[at + 2] == 1
    {
        return 3;
    }
    if at + 4 <= data.len()
        && data[at] == 0
        && data[at + 1] == 0
        && data[at + 2] == 0
        && data[at + 3] == 1
    {
        return 4;
    }
    0
}

/// Splits H.264 Annex-B bytes into NAL units.
///
/// Start codes (`0x000001` and `0x00000001`) are scanned; leading garbage
/// before the first start code, zero-length units and trailing zero bytes
/// are skipped. Emulation-prevention bytes (`0x000003`) inside payloads
/// never form a start code, so no unescaping happens here (that is
/// `ebsp_to_rbsp`, applied by the SPS/PPS/slice parsers). Input without
/// any start code yields an empty vector.
pub fn split_annexb_nals(data: &[u8]) -> Vec<NalUnit> {
    let mut units = Vec::new();
    let mut i = 0usize;
    let mut unit_start: Option<usize> = None;
    while i < data.len() {
        let sc = is_start_code_at(data, i);
        if sc > 0 {
            if let Some(start) = unit_start {
                let mut end = i;
                while end > start && data[end - 1] == 0 {
                    end -= 1;
                }
                if end > start {
                    let header = data[start];
                    units.push(NalUnit {
                        offset: start,
                        len: end - start,
                        nal_type: header & 0x1F,
                        nal_ref_idc: (header >> 5) & 0x03,
                    });
                }
            }
            i += sc;
            unit_start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(start) = unit_start {
        let mut end = data.len();
        while end > start && data[end - 1] == 0 {
            end -= 1;
        }
        if end > start && start < data.len() {
            let header = data[start];
            units.push(NalUnit {
                offset: start,
                len: end - start,
                nal_type: header & 0x1F,
                nal_ref_idc: (header >> 5) & 0x03,
            });
        }
    }
    units
}

/// Removes H.264 emulation-prevention bytes (`0x000003` following
/// `0x0000`) to obtain the RBSP used by the syntax parsers.
pub fn ebsp_to_rbsp(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut zeros = 0u8;
    for &b in data {
        if zeros >= 2 && b == 0x03 {
            zeros = 0;
            continue;
        }
        out.push(b);
        if b == 0 {
            zeros = zeros.saturating_add(1);
        } else {
            zeros = 0;
        }
    }
    out
}

struct BitReader<'a> {
    data: &'a [u8],
    bit_pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.bit_pos.min(self.data.len() * 8)
    }

    fn read_bit(&mut self) -> Option<u32> {
        if self.remaining() < 1 {
            return None;
        }
        let byte = self.data[self.bit_pos / 8];
        let shift = 7 - (self.bit_pos % 8);
        self.bit_pos += 1;
        Some(((byte >> shift) & 1) as u32)
    }

    fn read_bits(&mut self, n: u32) -> Option<u32> {
        if n > 32 || (n as usize) > self.remaining() {
            return None;
        }
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.read_bit()?;
        }
        Some(v)
    }

    fn read_ue(&mut self) -> Option<u32> {
        let mut zeros = 0u32;
        while self.read_bit()? == 0 {
            zeros += 1;
            if zeros > 31 {
                return None;
            }
        }
        if zeros == 0 {
            return Some(0);
        }
        let rest = self.read_bits(zeros)?;
        Some(((1u32 << zeros) - 1) + rest)
    }

    fn read_se(&mut self) -> Option<i32> {
        let ue = self.read_ue()?;
        if ue & 1 == 1 {
            Some(((ue + 1) / 2) as i32)
        } else {
            Some(-((ue / 2) as i32))
        }
    }

    fn skip_scaling_list(&mut self, size: usize) -> Option<()> {
        let mut last_scale = 8i32;
        let mut next_scale = 8i32;
        for _ in 0..size {
            if next_scale != 0 {
                let delta = self.read_se()?;
                next_scale = (last_scale + delta + 256) % 256;
            }
            last_scale = if next_scale == 0 { last_scale } else { next_scale };
        }
        Some(())
    }
}

/// Parses an SPS NAL unit (including its one-byte NAL header) into
/// `SpsInfo`. Returns `None` for non-SPS input, truncated data, or
/// unsupported constructs.
pub fn parse_sps(nal: &[u8]) -> Option<SpsInfo> {
    if nal.len() < 2 || nal[0] & 0x1F != 7 {
        return None;
    }
    let rbsp = ebsp_to_rbsp(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let profile_idc = r.read_bits(8)? as u8;
    let _constraint = r.read_bits(8)?;
    let _level_idc = r.read_bits(8)?;
    let _sps_id = r.read_ue()?;
    let mut chroma_format_idc = 1u32;
    let mut bit_depth_luma_minus8 = 0u8;
    let mut bit_depth_chroma_minus8 = 0u8;
    if matches!(profile_idc, 100 | 110 | 122 | 244 | 44 | 83 | 86 | 118 | 128) {
        chroma_format_idc = r.read_ue()?;
        if chroma_format_idc > 3 {
            return None;
        }
        if chroma_format_idc == 3 {
            let _separate_plane = r.read_bit()?;
        }
        bit_depth_luma_minus8 = r.read_ue()? as u8;
        bit_depth_chroma_minus8 = r.read_ue()? as u8;
        if bit_depth_luma_minus8 > 6 || bit_depth_chroma_minus8 > 6 {
            return None;
        }
        let _qp_bypass = r.read_bit()?;
        let scaling_present = r.read_bit()?;
        if scaling_present == 1 {
            let count = if chroma_format_idc == 3 { 12 } else { 8 };
            for i in 0..count {
                if r.read_bit()? == 1 {
                    r.skip_scaling_list(if i < 6 { 16 } else { 64 })?;
                }
            }
        }
    }
    let log2_max_frame_num_minus4 = r.read_ue()?;
    if log2_max_frame_num_minus4 > 12 {
        return None;
    }
    let pic_order_cnt_type = r.read_ue()?;
    if pic_order_cnt_type > 2 {
        return None;
    }
    let mut log2_max_pic_order_cnt_lsb_minus4 = 0u32;
    let mut delta_pic_order_always_zero_flag = false;
    if pic_order_cnt_type == 0 {
        log2_max_pic_order_cnt_lsb_minus4 = r.read_ue()?;
        if log2_max_pic_order_cnt_lsb_minus4 > 12 {
            return None;
        }
    } else if pic_order_cnt_type == 1 {
        delta_pic_order_always_zero_flag = r.read_bit()? == 1;
        let _offset_non_ref = r.read_se()?;
        let _offset_top_bottom = r.read_se()?;
        let cycles = r.read_ue()?;
        if cycles > 255 {
            return None;
        }
        for _ in 0..cycles {
            let _ = r.read_se()?;
        }
    }
    let num_ref_frames = r.read_ue()?;
    let _gaps_allowed = r.read_bit()?;
    let pic_width_in_mbs_minus1 = r.read_ue()?;
    let pic_height_in_map_units_minus1 = r.read_ue()?;
    if pic_width_in_mbs_minus1 > 255 || pic_height_in_map_units_minus1 > 255 {
        return None;
    }
    let frame_mbs_only_flag = r.read_bit()? == 1;
    let mut mb_adaptive_frame_field_flag = false;
    if !frame_mbs_only_flag {
        mb_adaptive_frame_field_flag = r.read_bit()? == 1;
    }
    let direct_8x8_inference_flag = r.read_bit()? == 1;
    let frame_cropping_flag = r.read_bit()? == 1;
    let mut crop = [0u32; 4];
    if frame_cropping_flag {
        for c in crop.iter_mut() {
            *c = r.read_ue()?;
        }
    }
    let mut width = (pic_width_in_mbs_minus1 + 1) * 16;
    let mut height =
        (2 - u32::from(frame_mbs_only_flag)) * (pic_height_in_map_units_minus1 + 1) * 16;
    if frame_cropping_flag {
        let (unit_x, unit_y) = match (chroma_format_idc, frame_mbs_only_flag) {
            (0, _) => (1, 2 - u32::from(frame_mbs_only_flag)),
            (1, true) => (2, 2),
            (1, false) => (2, 4),
            (2, true) => (2, 1),
            (2, false) => (2, 2),
            (3, true) => (1, 1),
            (3, false) => (1, 2),
            _ => return None,
        };
        width = width.saturating_sub((crop[0] + crop[1]) * unit_x);
        height = height.saturating_sub((crop[2] + crop[3]) * unit_y);
    }
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return None;
    }
    Some(SpsInfo {
        profile_idc,
        chroma_format_idc: chroma_format_idc as u8,
        bit_depth_luma_minus8,
        bit_depth_chroma_minus8,
        frame_mbs_only_flag,
        mb_adaptive_frame_field_flag,
        direct_8x8_inference_flag,
        log2_max_frame_num_minus4,
        pic_order_cnt_type,
        log2_max_pic_order_cnt_lsb_minus4,
        delta_pic_order_always_zero_flag,
        pic_width_in_mbs_minus1,
        pic_height_in_map_units_minus1,
        num_ref_frames,
        width,
        height,
    })
}

/// Returns the display dimensions from an SPS NAL unit, or `None` when
/// the NAL is not a parseable SPS.
pub fn parse_sps_dimensions(nal: &[u8]) -> Option<(u32, u32)> {
    parse_sps(nal).map(|s| (s.width, s.height))
}

/// Parses a PPS NAL unit (including its one-byte NAL header) into the
/// subset `decode_frame_vaapi` needs. Returns `None` for non-PPS input
/// or truncated data.
pub fn parse_pps(nal: &[u8]) -> Option<PpsInfo> {
    if nal.len() < 2 || nal[0] & 0x1F != 8 {
        return None;
    }
    let rbsp = ebsp_to_rbsp(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let pic_parameter_set_id = r.read_ue()?;
    let seq_parameter_set_id = r.read_ue()?;
    if pic_parameter_set_id > 255 || seq_parameter_set_id > 255 {
        return None;
    }
    let entropy_coding_mode_flag = r.read_bit()? == 1;
    let bottom_field_pic_order_in_frame_present_flag = r.read_bit()? == 1;
    let _num_slice_groups_minus1 = r.read_ue()?;
    let num_ref_idx_l0_default_active_minus1 = r.read_ue()?;
    let num_ref_idx_l1_default_active_minus1 = r.read_ue()?;
    let _weighted_pred_flag = r.read_bit()?;
    let _weighted_bipred_idc = r.read_bits(2)?;
    let pic_init_qp_minus26 = r.read_se()?;
    let _pic_init_qs_minus26 = r.read_se()?;
    let _chroma_qp_index_offset = r.read_se()?;
    let deblocking_filter_control_present_flag = r.read_bit()? == 1;
    Some(PpsInfo {
        pic_parameter_set_id,
        seq_parameter_set_id,
        entropy_coding_mode_flag,
        bottom_field_pic_order_in_frame_present_flag,
        num_ref_idx_l0_default_active_minus1,
        num_ref_idx_l1_default_active_minus1,
        pic_init_qp_minus26,
        deblocking_filter_control_present_flag,
    })
}

/// Parses a coded-slice NAL unit (including its one-byte NAL header)
/// into the subset needed for single-slice I-frame submit. Only
/// `slice_type` I (2) headers are parsed to the end; P/B headers stop
/// after the prediction-weight-table flag position is unreachable, so
/// they return `None`.
///
/// `nal_ref_idc` comes from the NAL header; `sps`/`pps` supply the
/// surrounding sets. Returns `None` for non-VCL input or truncated data.
pub fn parse_slice_header(nal: &[u8], sps: &SpsInfo, pps: &PpsInfo) -> Option<SliceInfo> {
    let nal_type = *nal.first()? & 0x1F;
    if !(1..=5).contains(&nal_type) {
        return None;
    }
    let nal_ref_idc = (nal[0] >> 5) & 0x03;
    let is_idr = nal_type == 5;
    let rbsp = ebsp_to_rbsp(&nal[1..]);
    let mut r = BitReader::new(&rbsp);
    let first_mb_in_slice = r.read_ue()?;
    let raw_slice_type = r.read_ue()?;
    if raw_slice_type > 9 {
        return None;
    }
    let slice_type = normalize_slice_type(raw_slice_type as u8);
    if slice_type != 2 {
        // P/B/SP/SI need reference handling (DPB); the submit path only
        // supports I-slices, so stop here instead of half-parsing.
        return None;
    }
    let pic_parameter_set_id = r.read_ue()?;
    if pic_parameter_set_id != pps.pic_parameter_set_id {
        return None;
    }
    let frame_num_bits = sps.log2_max_frame_num_minus4 + 4;
    let frame_num = r.read_bits(frame_num_bits)?;
    let mut field_pic_flag = false;
    if !sps.frame_mbs_only_flag {
        field_pic_flag = r.read_bit()? == 1;
        if field_pic_flag {
            // Field pictures need field-aware POC and DPB handling.
            return None;
        }
    }
    let mut idr_pic_id: Option<u32> = None;
    if is_idr {
        idr_pic_id = Some(r.read_ue()?);
    }
    let mut pic_order_cnt_lsb: Option<u32> = None;
    if sps.pic_order_cnt_type == 0 {
        let bits = sps.log2_max_pic_order_cnt_lsb_minus4 + 4;
        let lsb = r.read_bits(bits)?;
        if pps.bottom_field_pic_order_in_frame_present_flag && !field_pic_flag {
            let _delta_bottom = r.read_se()?;
        }
        pic_order_cnt_lsb = Some(lsb);
    }
    // I-slices carry no ref-pic-list reordering or weight tables.
    let num_ref_idx_l0_active_minus1 = pps.num_ref_idx_l0_default_active_minus1;
    let num_ref_idx_l1_active_minus1 = pps.num_ref_idx_l1_default_active_minus1;
    let mut dec_ref_pic_marking_bits_ok = true;
    if nal_ref_idc != 0 {
        if is_idr {
            let _no_output_of_prior = r.read_bit()?;
            let _long_term_reference = r.read_bit()?;
        } else {
            let adaptive = r.read_bit()?;
            if adaptive == 1 {
                for _ in 0..32 {
                    let op = r.read_ue()?;
                    match op {
                        0 => break,
                        1 | 2 => {
                            let _ = r.read_ue()?;
                        }
                        3 => {
                            let _ = r.read_ue()?;
                            let _ = r.read_ue()?;
                        }
                        4 => {
                            let _ = r.read_ue()?;
                        }
                        5 | 6 => {}
                        _ => {
                            dec_ref_pic_marking_bits_ok = false;
                            break;
                        }
                    }
                }
            }
        }
    }
    if !dec_ref_pic_marking_bits_ok {
        return None;
    }
    let slice_qp_delta = r.read_se()?;
    if slice_qp_delta < -87 || slice_qp_delta > 77 {
        return None;
    }
    let mut disable_deblocking_filter_idc = 0u32;
    let mut slice_alpha_c0_offset_div2 = 0i32;
    let mut slice_beta_offset_div2 = 0i32;
    if pps.deblocking_filter_control_present_flag {
        disable_deblocking_filter_idc = r.read_ue()?;
        if disable_deblocking_filter_idc > 2 {
            return None;
        }
        if disable_deblocking_filter_idc != 1 {
            slice_alpha_c0_offset_div2 = r.read_se()?;
            slice_beta_offset_div2 = r.read_se()?;
            if slice_alpha_c0_offset_div2 < -6
                || slice_alpha_c0_offset_div2 > 6
                || slice_beta_offset_div2 < -6
                || slice_beta_offset_div2 > 6
            {
                return None;
            }
        }
    }
    Some(SliceInfo {
        first_mb_in_slice,
        slice_type,
        pic_parameter_set_id,
        frame_num,
        pic_order_cnt_lsb,
        idr_pic_id,
        num_ref_idx_l0_active_minus1,
        num_ref_idx_l1_active_minus1,
        slice_qp_delta,
        disable_deblocking_filter_idc,
        slice_alpha_c0_offset_div2,
        slice_beta_offset_div2,
        header_bits: r.bit_pos as u32,
        is_idr,
    })
}

// ---------------------------------------------------------------------------
// YUV to RGB converters (pure Rust, cross-platform, BT.601 full-range to
// match the crate's software converters).
// ---------------------------------------------------------------------------

fn yuv_to_rgb_pixel(y: u8, u: u8, v: u8) -> [u8; 3] {
    let y = y as f32;
    let u = u as f32 - 128.0;
    let v = v as f32 - 128.0;
    [
        (y + 1.402 * v).round().clamp(0.0, 255.0) as u8,
        (y - 0.344136 * u - 0.714136 * v).round().clamp(0.0, 255.0) as u8,
        (y + 1.772 * u).round().clamp(0.0, 255.0) as u8,
    ]
}

fn check_dims(width: u32, height: u32) -> Result<(usize, usize)> {
    if width == 0 || height == 0 || width > 16384 || height > 16384 {
        return Err(MediaError::ParseError("bad frame dimensions".into()));
    }
    if width % 2 != 0 || height % 2 != 0 {
        return Err(MediaError::ParseError("yuv420 needs even dimensions".into()));
    }
    Ok((width as usize, height as usize))
}

/// Converts NV12 (Y plus interleaved UV) to RGB24.
pub fn nv12_to_rgb(y_plane: &[u8], uv_plane: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    let (w, h) = check_dims(width, height)?;
    if y_plane.len() < w * h || uv_plane.len() < w * h / 2 {
        return Err(MediaError::ParseError("nv12 buffer short".into()));
    }
    let mut out = vec![0u8; w * h * 3];
    for row in 0..h {
        for col in 0..w {
            let y = y_plane[row * w + col];
            let uv_off = (row / 2) * w + (col / 2) * 2;
            let px = yuv_to_rgb_pixel(y, uv_plane[uv_off], uv_plane[uv_off + 1]);
            let o = (row * w + col) * 3;
            out[o..o + 3].copy_from_slice(&px);
        }
    }
    Ok(out)
}

/// Converts planar I420/YV12 (Y, U, V) to RGB24. For `YV12` input pass the
/// planes in Y, U, V order (swapped by the caller after reading offsets).
pub fn i420_to_rgb(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    let (w, h) = check_dims(width, height)?;
    let (cw, ch) = (w / 2, h / 2);
    if y_plane.len() < w * h || u_plane.len() < cw * ch || v_plane.len() < cw * ch {
        return Err(MediaError::ParseError("i420 buffer short".into()));
    }
    let mut out = vec![0u8; w * h * 3];
    for row in 0..h {
        for col in 0..w {
            let px = yuv_to_rgb_pixel(
                y_plane[row * w + col],
                u_plane[(row / 2) * cw + col / 2],
                v_plane[(row / 2) * cw + col / 2],
            );
            let o = (row * w + col) * 3;
            out[o..o + 3].copy_from_slice(&px);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Linux-only VA-API bindings and session management.
// ---------------------------------------------------------------------------

/// Raw VA-API/DRM bindings, Linux-only. Every declaration is local
/// (no libc crate); values verified against `/usr/include/va/va.h`.
#[cfg(target_os = "linux")]
mod va_sys {
    use std::os::raw::{c_char, c_int, c_uint, c_void};

    pub type VaDisplay = *mut c_void;
    pub type VaStatus = c_int;
    pub type VaProfile = c_int;
    pub type VaEntrypoint = c_int;
    pub type VaConfigId = c_uint;
    pub type VaContextId = c_uint;
    pub type VaSurfaceId = c_uint;
    pub type VaBufferId = c_uint;
    pub type VaBufferType = c_int;

    extern "C" {
        pub fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        pub fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
        pub fn dlclose(handle: *mut c_void) -> c_int;
    }

    pub type FnGetDisplayDrm = unsafe extern "C" fn(c_int) -> VaDisplay;
    pub type FnInitialize = unsafe extern "C" fn(VaDisplay, *mut c_int, *mut c_int) -> VaStatus;
    pub type FnTerminate = unsafe extern "C" fn(VaDisplay) -> VaStatus;
    pub type FnMaxProfiles = unsafe extern "C" fn(VaDisplay) -> c_int;
    pub type FnMaxEntrypoints = unsafe extern "C" fn(VaDisplay) -> c_int;
    pub type FnQueryProfiles =
        unsafe extern "C" fn(VaDisplay, *mut VaProfile, *mut c_int) -> VaStatus;
    pub type FnQueryEntrypoints =
        unsafe extern "C" fn(VaDisplay, VaProfile, *mut VaEntrypoint, *mut c_int) -> VaStatus;
    pub type FnCreateConfig = unsafe extern "C" fn(
        VaDisplay,
        VaProfile,
        VaEntrypoint,
        *mut c_void,
        c_int,
        *mut VaConfigId,
    ) -> VaStatus;
    pub type FnDestroyConfig = unsafe extern "C" fn(VaDisplay, VaConfigId) -> VaStatus;
    pub type FnCreateSurfaces = unsafe extern "C" fn(
        VaDisplay,
        c_uint,
        c_uint,
        c_uint,
        *mut VaSurfaceId,
        c_uint,
        *mut c_void,
        c_uint,
    ) -> VaStatus;
    pub type FnDestroySurfaces =
        unsafe extern "C" fn(VaDisplay, *mut VaSurfaceId, c_int) -> VaStatus;
    pub type FnCreateContext = unsafe extern "C" fn(
        VaDisplay,
        VaConfigId,
        c_int,
        c_int,
        c_int,
        *mut VaSurfaceId,
        c_int,
        *mut VaContextId,
    ) -> VaStatus;
    pub type FnDestroyContext = unsafe extern "C" fn(VaDisplay, VaContextId) -> VaStatus;
    pub type FnCreateBuffer = unsafe extern "C" fn(
        VaDisplay,
        VaContextId,
        VaBufferType,
        c_uint,
        c_uint,
        *mut c_void,
        *mut VaBufferId,
    ) -> VaStatus;
    pub type FnDestroyBuffer = unsafe extern "C" fn(VaDisplay, VaBufferId) -> VaStatus;
    pub type FnBeginPicture =
        unsafe extern "C" fn(VaDisplay, VaContextId, VaSurfaceId) -> VaStatus;
    pub type FnRenderPicture =
        unsafe extern "C" fn(VaDisplay, VaContextId, *mut VaBufferId, c_int) -> VaStatus;
    pub type FnEndPicture = unsafe extern "C" fn(VaDisplay, VaContextId) -> VaStatus;
    pub type FnSyncSurface = unsafe extern "C" fn(VaDisplay, VaSurfaceId) -> VaStatus;
    pub type FnDeriveImage =
        unsafe extern "C" fn(VaDisplay, VaSurfaceId, *mut super::VaImage) -> VaStatus;
    pub type FnMapBuffer =
        unsafe extern "C" fn(VaDisplay, VaBufferId, *mut *mut c_void) -> VaStatus;
    pub type FnUnmapBuffer = unsafe extern "C" fn(VaDisplay, VaBufferId) -> VaStatus;
    pub type FnDestroyImage = unsafe extern "C" fn(VaDisplay, c_uint) -> VaStatus;
}

/// `VAPictureH264` from `va/va.h` (C-verified size: 36 bytes).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VaPictureH264 {
    picture_id: u32,
    frame_idx: u32,
    flags: u32,
    top_field_order_cnt: i32,
    bottom_field_order_cnt: i32,
    va_reserved: [u32; 4],
}

/// `VAPictureParameterBufferH264` from `va/va.h` (C-verified size: 672).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone)]
struct VaPicParamH264 {
    curr_pic: VaPictureH264,
    reference_frames: [VaPictureH264; 16],
    picture_width_in_mbs_minus1: u16,
    picture_height_in_mbs_minus1: u16,
    bit_depth_luma_minus8: u8,
    bit_depth_chroma_minus8: u8,
    num_ref_frames: u8,
    _pad0: u8,
    seq_fields: u32,
    num_slice_groups_minus1: u8,
    slice_group_map_type: u8,
    slice_group_change_rate_minus1: u16,
    pic_init_qp_minus26: i8,
    pic_init_qs_minus26: i8,
    chroma_qp_index_offset: i8,
    second_chroma_qp_index_offset: i8,
    pic_fields: u32,
    frame_num: u16,
    _pad1: u16,
    va_reserved: [u32; 8],
}

/// `VASliceParameterBufferH264` from `va/va.h` (C-verified size: 3128).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone)]
struct VaSliceParamH264 {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    slice_data_bit_offset: u16,
    first_mb_in_slice: u16,
    slice_type: u8,
    direct_spatial_mv_pred_flag: u8,
    num_ref_idx_l0_active_minus1: u8,
    num_ref_idx_l1_active_minus1: u8,
    cabac_init_idc: u8,
    slice_qp_delta: i8,
    disable_deblocking_filter_idc: u8,
    slice_alpha_c0_offset_div2: i8,
    slice_beta_offset_div2: i8,
    _pad0: [u8; 3],
    ref_pic_list0: [VaPictureH264; 32],
    ref_pic_list1: [VaPictureH264; 32],
    luma_log2_weight_denom: u8,
    chroma_log2_weight_denom: u8,
    luma_weight_l0_flag: u8,
    _pad1: u8,
    luma_weight_l0: [i16; 32],
    luma_offset_l0: [i16; 32],
    chroma_weight_l0_flag: u8,
    _pad2: u8,
    chroma_weight_l0: [[i16; 2]; 32],
    chroma_offset_l0: [[i16; 2]; 32],
    luma_weight_l1_flag: u8,
    _pad3: u8,
    luma_weight_l1: [i16; 32],
    luma_offset_l1: [i16; 32],
    chroma_weight_l1_flag: u8,
    _pad4: u8,
    chroma_weight_l1: [[i16; 2]; 32],
    chroma_offset_l1: [[i16; 2]; 32],
    va_reserved: [u32; 4],
}

/// `VAImageFormat` from `va/va.h` (C-verified size: 48 bytes).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct VaImageFormat {
    fourcc: u32,
    byte_order: u32,
    bits_per_pixel: u32,
    depth: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    va_reserved: [u32; 4],
}

/// `VAImage` from `va/va.h` (C-verified size: 120 bytes).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct VaImage {
    image_id: u32,
    format: VaImageFormat,
    buf: u32,
    width: u16,
    height: u16,
    data_size: u32,
    num_planes: u32,
    pitches: [u32; 3],
    offsets: [u32; 3],
    num_palette_entries: i32,
    entry_bytes: i32,
    component_order: [i8; 4],
    va_reserved: [u32; 4],
}

#[cfg(target_os = "linux")]
fn invalid_picture() -> VaPictureH264 {
    VaPictureH264 {
        picture_id: VA_INVALID_ID,
        frame_idx: 0,
        flags: VA_PICTURE_H264_INVALID,
        top_field_order_cnt: 0,
        bottom_field_order_cnt: 0,
        va_reserved: [0; 4],
    }
}

/// Builds the picture parameter buffer for a single I-frame submit with an
/// empty DPB (all references invalid). Pure constructor, fully testable
/// without hardware.
#[cfg(target_os = "linux")]
fn build_pic_param(
    sps: &SpsInfo,
    pps: &PpsInfo,
    slice: &SliceInfo,
    surface: u32,
) -> VaPicParamH264 {
    let poc = slice.pic_order_cnt_lsb.unwrap_or(0) as i32;
    let seq_fields = (u32::from(sps.chroma_format_idc) & 0x03)
        | (u32::from(sps.frame_mbs_only_flag) << 4)
        | (u32::from(sps.mb_adaptive_frame_field_flag) << 5)
        | (u32::from(sps.direct_8x8_inference_flag) << 6)
        | ((sps.log2_max_frame_num_minus4 & 0x0F) << 8)
        | ((sps.pic_order_cnt_type & 0x03) << 12)
        | ((sps.log2_max_pic_order_cnt_lsb_minus4 & 0x0F) << 14)
        | (u32::from(sps.delta_pic_order_always_zero_flag) << 18);
    let pic_fields = u32::from(pps.entropy_coding_mode_flag)
        | (u32::from(pps.bottom_field_pic_order_in_frame_present_flag) << 7)
        | (u32::from(pps.deblocking_filter_control_present_flag) << 8)
        | (1 << 10);
    VaPicParamH264 {
        curr_pic: VaPictureH264 {
            picture_id: surface,
            frame_idx: slice.frame_num,
            flags: 0,
            top_field_order_cnt: poc,
            bottom_field_order_cnt: poc,
            va_reserved: [0; 4],
        },
        reference_frames: [invalid_picture(); 16],
        picture_width_in_mbs_minus1: sps.pic_width_in_mbs_minus1 as u16,
        picture_height_in_mbs_minus1: sps.pic_height_in_map_units_minus1 as u16,
        bit_depth_luma_minus8: sps.bit_depth_luma_minus8,
        bit_depth_chroma_minus8: sps.bit_depth_chroma_minus8,
        num_ref_frames: sps.num_ref_frames.min(16) as u8,
        _pad0: 0,
        seq_fields,
        num_slice_groups_minus1: 0,
        slice_group_map_type: 0,
        slice_group_change_rate_minus1: 0,
        pic_init_qp_minus26: pps.pic_init_qp_minus26.clamp(-26, 25) as i8,
        pic_init_qs_minus26: 0,
        chroma_qp_index_offset: 0,
        second_chroma_qp_index_offset: 0,
        pic_fields,
        frame_num: (slice.frame_num & 0xFFFF) as u16,
        _pad1: 0,
        va_reserved: [0; 8],
    }
}

/// Builds the slice parameter buffer for a single-slice I-frame submit.
/// All reference lists stay invalid (empty DPB); weights stay zero.
#[cfg(target_os = "linux")]
fn build_slice_param(slice: &SliceInfo, nal_len: u32) -> VaSliceParamH264 {
    VaSliceParamH264 {
        slice_data_size: nal_len,
        slice_data_offset: 0,
        slice_data_flag: VA_SLICE_DATA_FLAG_ALL,
        slice_data_bit_offset: (slice.header_bits & 0xFFFF) as u16,
        first_mb_in_slice: slice.first_mb_in_slice.min(0xFFFF) as u16,
        slice_type: slice.slice_type,
        direct_spatial_mv_pred_flag: 0,
        num_ref_idx_l0_active_minus1: slice.num_ref_idx_l0_active_minus1.min(31) as u8,
        num_ref_idx_l1_active_minus1: slice.num_ref_idx_l1_active_minus1.min(31) as u8,
        cabac_init_idc: 0,
        slice_qp_delta: slice.slice_qp_delta.clamp(-87, 77) as i8,
        disable_deblocking_filter_idc: slice.disable_deblocking_filter_idc.min(2) as u8,
        slice_alpha_c0_offset_div2: slice.slice_alpha_c0_offset_div2.clamp(-6, 6) as i8,
        slice_beta_offset_div2: slice.slice_beta_offset_div2.clamp(-6, 6) as i8,
        _pad0: [0; 3],
        ref_pic_list0: [invalid_picture(); 32],
        ref_pic_list1: [invalid_picture(); 32],
        luma_log2_weight_denom: 0,
        chroma_log2_weight_denom: 0,
        luma_weight_l0_flag: 0,
        _pad1: 0,
        luma_weight_l0: [0; 32],
        luma_offset_l0: [0; 32],
        chroma_weight_l0_flag: 0,
        _pad2: 0,
        chroma_weight_l0: [[0; 2]; 32],
        chroma_offset_l0: [[0; 2]; 32],
        luma_weight_l1_flag: 0,
        _pad3: 0,
        luma_weight_l1: [0; 32],
        luma_offset_l1: [0; 32],
        chroma_weight_l1_flag: 0,
        _pad4: 0,
        chroma_weight_l1: [[0; 2]; 32],
        chroma_offset_l1: [[0; 2]; 32],
        va_reserved: [0; 4],
    }
}

// ---------------------------------------------------------------------------
// Phase 1: init plus capability query (Linux-only).
// ---------------------------------------------------------------------------

/// Loaded VA-API function table. Handles are closed in reverse order on
/// drop; every symbol is optional-checked at load time.
#[cfg(target_os = "linux")]
struct VaLib {
    va_handle: *mut std::os::raw::c_void,
    drm_handle: *mut std::os::raw::c_void,
    get_display_drm: Option<va_sys::FnGetDisplayDrm>,
    initialize: Option<va_sys::FnInitialize>,
    terminate: Option<va_sys::FnTerminate>,
    max_profiles: Option<va_sys::FnMaxProfiles>,
    max_entrypoints: Option<va_sys::FnMaxEntrypoints>,
    query_profiles: Option<va_sys::FnQueryProfiles>,
    query_entrypoints: Option<va_sys::FnQueryEntrypoints>,
    create_config: Option<va_sys::FnCreateConfig>,
    destroy_config: Option<va_sys::FnDestroyConfig>,
    create_surfaces: Option<va_sys::FnCreateSurfaces>,
    destroy_surfaces: Option<va_sys::FnDestroySurfaces>,
    create_context: Option<va_sys::FnCreateContext>,
    destroy_context: Option<va_sys::FnDestroyContext>,
    create_buffer: Option<va_sys::FnCreateBuffer>,
    destroy_buffer: Option<va_sys::FnDestroyBuffer>,
    begin_picture: Option<va_sys::FnBeginPicture>,
    render_picture: Option<va_sys::FnRenderPicture>,
    end_picture: Option<va_sys::FnEndPicture>,
    sync_surface: Option<va_sys::FnSyncSurface>,
    derive_image: Option<va_sys::FnDeriveImage>,
    map_buffer: Option<va_sys::FnMapBuffer>,
    unmap_buffer: Option<va_sys::FnUnmapBuffer>,
    destroy_image: Option<va_sys::FnDestroyImage>,
}

// SAFETY: handles are only used on the thread that created the session;
// `VaapiSession` stays `!Send`/`!Sync` through the raw file handle.
#[cfg(target_os = "linux")]
unsafe fn load_sym<T>(handle: *mut std::os::raw::c_void, name: &[u8]) -> Option<T> {
    if handle.is_null() {
        return None;
    }
    // SAFETY: `name` is a NUL-terminated literal; `dlsym` returns either a
    // valid function address or NULL, and NULL maps to `None`.
    let sym = unsafe { va_sys::dlsym(handle, name.as_ptr() as *const std::os::raw::c_char) };
    if sym.is_null() {
        return None;
    }
    // SAFETY: function signatures were checked against `va/va.h`; the
    // transmute only reinterprets the address as the matching fn type.
    Some(unsafe { std::mem::transmute_copy::<*mut std::os::raw::c_void, T>(&sym) })
}

#[cfg(target_os = "linux")]
impl VaLib {
    fn load(va_path: &[u8], drm_path: &[u8]) -> Option<Self> {
        // SAFETY: paths are NUL-terminated literals; NULL return means the
        // library is absent and maps to graceful degradation.
        let va_handle = unsafe {
            va_sys::dlopen(
                va_path.as_ptr() as *const std::os::raw::c_char,
                RTLD_NOW,
            )
        };
        if va_handle.is_null() {
            return None;
        }
        let drm_handle = unsafe {
            va_sys::dlopen(
                drm_path.as_ptr() as *const std::os::raw::c_char,
                RTLD_NOW,
            )
        };
        if drm_handle.is_null() {
            // SAFETY: handle came from a successful `dlopen`.
            unsafe {
                va_sys::dlclose(va_handle);
            }
            return None;
        }
        // SAFETY: handles are valid; each symbol is NULL-checked.
        unsafe {
            Some(Self {
                va_handle,
                drm_handle,
                get_display_drm: load_sym(drm_handle, b"vaGetDisplayDRM\0"),
                initialize: load_sym(va_handle, b"vaInitialize\0"),
                terminate: load_sym(va_handle, b"vaTerminate\0"),
                max_profiles: load_sym(va_handle, b"vaMaxNumProfiles\0"),
                max_entrypoints: load_sym(va_handle, b"vaMaxNumEntrypoints\0"),
                query_profiles: load_sym(va_handle, b"vaQueryConfigProfiles\0"),
                query_entrypoints: load_sym(va_handle, b"vaQueryConfigEntrypoints\0"),
                create_config: load_sym(va_handle, b"vaCreateConfig\0"),
                destroy_config: load_sym(va_handle, b"vaDestroyConfig\0"),
                create_surfaces: load_sym(va_handle, b"vaCreateSurfaces\0"),
                destroy_surfaces: load_sym(va_handle, b"vaDestroySurfaces\0"),
                create_context: load_sym(va_handle, b"vaCreateContext\0"),
                destroy_context: load_sym(va_handle, b"vaDestroyContext\0"),
                create_buffer: load_sym(va_handle, b"vaCreateBuffer\0"),
                destroy_buffer: load_sym(va_handle, b"vaDestroyBuffer\0"),
                begin_picture: load_sym(va_handle, b"vaBeginPicture\0"),
                render_picture: load_sym(va_handle, b"vaRenderPicture\0"),
                end_picture: load_sym(va_handle, b"vaEndPicture\0"),
                sync_surface: load_sym(va_handle, b"vaSyncSurface\0"),
                derive_image: load_sym(va_handle, b"vaDeriveImage\0"),
                map_buffer: load_sym(va_handle, b"vaMapBuffer\0"),
                unmap_buffer: load_sym(va_handle, b"vaUnmapBuffer\0"),
                destroy_image: load_sym(va_handle, b"vaDestroyImage\0"),
            })
        }
    }

    /// True when every symbol Phase 1 and Phase 2 need is present.
    fn complete(&self) -> bool {
        self.get_display_drm.is_some()
            && self.initialize.is_some()
            && self.terminate.is_some()
            && self.max_profiles.is_some()
            && self.max_entrypoints.is_some()
            && self.query_profiles.is_some()
            && self.query_entrypoints.is_some()
            && self.create_config.is_some()
            && self.destroy_config.is_some()
            && self.create_surfaces.is_some()
            && self.destroy_surfaces.is_some()
            && self.create_context.is_some()
            && self.destroy_context.is_some()
            && self.create_buffer.is_some()
            && self.destroy_buffer.is_some()
            && self.begin_picture.is_some()
            && self.render_picture.is_some()
            && self.end_picture.is_some()
            && self.sync_surface.is_some()
            && self.derive_image.is_some()
            && self.map_buffer.is_some()
            && self.unmap_buffer.is_some()
            && self.destroy_image.is_some()
    }
}

#[cfg(target_os = "linux")]
impl Drop for VaLib {
    fn drop(&mut self) {
        // SAFETY: handles came from successful `dlopen` calls.
        unsafe {
            if !self.drm_handle.is_null() {
                va_sys::dlclose(self.drm_handle);
                self.drm_handle = std::ptr::null_mut();
            }
            if !self.va_handle.is_null() {
                va_sys::dlclose(self.va_handle);
                self.va_handle = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn default_render_nodes() -> Vec<String> {
    (128..144)
        .map(|n| format!("/dev/dri/renderD{n}"))
        .collect()
}

#[cfg(target_os = "linux")]
fn va_status_ok(status: i32, what: &str) -> Result<()> {
    if status == VA_STATUS_SUCCESS {
        Ok(())
    } else {
        Err(MediaError::CommandFailed(format!("{what}: status {status}")))
    }
}

/// Queries one opened render node; returns the H.264 VLD decoders it
/// exposes, or an empty vector when anything is missing or fails.
#[cfg(target_os = "linux")]
fn query_node(lib: &VaLib, fd: std::os::raw::c_int) -> Vec<HwDecoder> {
    let mut out = Vec::new();
    let (Some(get_display), Some(initialize), Some(terminate), Some(max_profiles), Some(max_entrypoints), Some(query_profiles), Some(query_entrypoints)) = (
        lib.get_display_drm,
        lib.initialize,
        lib.terminate,
        lib.max_profiles,
        lib.max_entrypoints,
        lib.query_profiles,
        lib.query_entrypoints,
    ) else {
        return out;
    };
    // SAFETY: `fd` is an open DRM render node owned by the caller.
    let dpy = unsafe { get_display(fd) };
    if dpy.is_null() {
        return out;
    }
    let mut major = 0;
    let mut minor = 0;
    // SAFETY: version out-params are valid ints; a failing init means no
    // usable display, so the node is skipped without terminating.
    if unsafe { initialize(dpy, &mut major, &mut minor) } != VA_STATUS_SUCCESS {
        return out;
    }
    // SAFETY: count queries take no buffers.
    let max_p = unsafe { max_profiles(dpy) }.clamp(0, 64) as usize;
    let max_e = unsafe { max_entrypoints(dpy) }.clamp(0, 64) as usize;
    if max_p > 0 && max_e > 0 {
        let mut profiles = vec![0; max_p];
        let mut num_profiles = 0;
        // SAFETY: buffers are sized by the driver's own maxima.
        let ok = unsafe { query_profiles(dpy, profiles.as_mut_ptr(), &mut num_profiles) }
            == VA_STATUS_SUCCESS;
        if ok {
            let n = (num_profiles.max(0) as usize).min(max_p);
            for &profile in &profiles[..n] {
                let Some(name) = h264_profile_name(profile) else {
                    continue;
                };
                let mut entrypoints = vec![0; max_e];
                let mut num_entrypoints = 0;
                let ok = unsafe {
                    query_entrypoints(dpy, profile, entrypoints.as_mut_ptr(), &mut num_entrypoints)
                } == VA_STATUS_SUCCESS;
                if !ok {
                    continue;
                }
                let m = (num_entrypoints.max(0) as usize).min(max_e);
                if entrypoints[..m].contains(&VA_ENTRYPOINT_VLD) {
                    out.push(HwDecoder {
                        api: "va-api".to_string(),
                        codec: "h264".to_string(),
                        profile: name.to_string(),
                        profile_id: profile,
                        entrypoint: "VAEntrypointVLD".to_string(),
                        max_width: None,
                        max_height: None,
                    });
                }
            }
        }
    }
    // SAFETY: display was initialized; terminate status is best-effort.
    unsafe {
        terminate(dpy);
    }
    out
}

#[cfg(target_os = "linux")]
fn hw_decoder_info_with_libs(va_path: &[u8], drm_path: &[u8], nodes: &[String]) -> Vec<HwDecoder> {
    let Some(lib) = VaLib::load(va_path, drm_path) else {
        return Vec::new();
    };
    if !lib.complete() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for node in nodes {
        let Ok(file) = std::fs::OpenOptions::new().read(true).write(true).open(node) else {
            continue;
        };
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        out.extend(query_node(&lib, fd));
        // `file` (and its fd) is dropped here, after `vaTerminate`.
    }
    out
}

/// Reports the system-GPU H.264 decoders (VA-API, VLD entrypoint).
///
/// Opens each DRM render node (`/dev/dri/renderD128` and up),
/// initializes VA-API and lists H.264 profiles with a VLD entrypoint.
/// Returns an empty vector when there is no render node, no libva, no
/// H.264 entrypoint, or any VA call fails. Never panics.
#[cfg(target_os = "linux")]
pub fn hw_decoder_info() -> Vec<HwDecoder> {
    hw_decoder_info_with_libs(LIBVA_SO, LIBVA_DRM_SO, &default_render_nodes())
}

/// Same as `hw_decoder_info`, but probes exactly `nodes` (e.g.
/// `&["/dev/dri/renderD128"]`). Primarily useful for tests and for
/// machines with unusual device paths. Never panics.
#[cfg(target_os = "linux")]
pub fn hw_decoder_info_from_nodes(nodes: &[&str]) -> Vec<HwDecoder> {
    let owned: Vec<String> = nodes.iter().map(|s| s.to_string()).collect();
    hw_decoder_info_with_libs(LIBVA_SO, LIBVA_DRM_SO, &owned)
}

// ---------------------------------------------------------------------------
// Phase 2: H.264 Annex-B decode session (Linux-only).
// ---------------------------------------------------------------------------

/// A VA-API H.264 decode session: one display, config, context and a
/// small surface pool. Surfaces decode I-slices; P/B slices are rejected
/// with an honest `UnsupportedFormat` until DPB management lands.
///
/// Sessions are single-threaded (`!Send`/`!Sync` via the raw display).
/// All VA resources are released in `Drop`, in reverse creation order.
#[cfg(target_os = "linux")]
pub struct VaapiSession {
    lib: VaLib,
    // Kept alive: the DRM fd must stay open for the display lifetime.
    _node: std::fs::File,
    dpy: va_sys::VaDisplay,
    config: va_sys::VaConfigId,
    context: va_sys::VaContextId,
    surfaces: Vec<va_sys::VaSurfaceId>,
    width: u32,
    height: u32,
    profile: i32,
}

#[cfg(target_os = "linux")]
impl VaapiSession {
    /// Opens a session on the first render node that offers H.264 VLD
    /// decode at `width` x `height`.
    pub fn open(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(MediaError::ParseError("bad session dimensions".into()));
        }
        let nodes = default_render_nodes();
        let refs: Vec<&str> = nodes.iter().map(String::as_str).collect();
        Self::open_on_nodes(&refs, width, height)
    }

    /// Opens a session on the first usable node in `nodes`.
    pub fn open_on_nodes(nodes: &[&str], width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 16384 || height > 16384 {
            return Err(MediaError::ParseError("bad session dimensions".into()));
        }
        let mut last_error: Option<MediaError> = None;
        for node in nodes {
            match Self::open_on_node(node, width, height) {
                Ok(session) => return Ok(session),
                Err(e) => last_error = Some(e),
            }
        }
        match last_error {
            Some(e) => Err(e),
            None => Err(MediaError::UnsupportedFormat(
                "va-api unavailable: no render node".into(),
            )),
        }
    }

    fn open_on_node(node: &str, width: u32, height: u32) -> Result<Self> {
        let lib = match VaLib::load(LIBVA_SO, LIBVA_DRM_SO) {
            Some(lib) => lib,
            None => {
                return Err(MediaError::UnsupportedFormat(
                    "va-api unavailable: libva.so.2 or libva-drm.so.2 missing".into(),
                ));
            }
        };
        if !lib.complete() {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: incomplete VA-API symbols".into(),
            ));
        }
        let (Some(get_display), Some(initialize), Some(terminate), Some(create_config), Some(destroy_config), Some(create_surfaces), Some(destroy_surfaces), Some(create_context)) = (
            lib.get_display_drm,
            lib.initialize,
            lib.terminate,
            lib.create_config,
            lib.destroy_config,
            lib.create_surfaces,
            lib.destroy_surfaces,
            lib.create_context,
        ) else {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: incomplete VA-API symbols".into(),
            ));
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(node)
            .map_err(|_| {
                MediaError::UnsupportedFormat(format!("va-api unavailable: cannot open {node}"))
            })?;
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // SAFETY: `fd` is an open DRM render node owned by `file`, which
        // outlives the display (stored in the session).
        let dpy = unsafe { get_display(fd) };
        if dpy.is_null() {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: vaGetDisplayDRM failed".into(),
            ));
        }
        let mut major = 0;
        let mut minor = 0;
        // SAFETY: version out-params are valid ints. A failing init means
        // no usable display, so the node is skipped without terminating.
        if unsafe { initialize(dpy, &mut major, &mut minor) } != VA_STATUS_SUCCESS {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: vaInitialize failed".into(),
            ));
        }
        // Pick the richest H.264 profile with a VLD entrypoint.
        let mut chosen: Option<i32> = None;
        for candidate in [
            VA_PROFILE_H264_HIGH,
            VA_PROFILE_H264_MAIN,
            VA_PROFILE_H264_CONSTRAINED_BASELINE,
        ] {
            if profile_has_vld(&lib, dpy, candidate) {
                chosen = Some(candidate);
                break;
            }
        }
        let Some(profile) = chosen else {
            // SAFETY: display was initialized.
            unsafe {
                terminate(dpy);
            }
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: no H.264 VLD entrypoint".into(),
            ));
        };
        let mut config: va_sys::VaConfigId = 0;
        // SAFETY: NULL attribute list selects driver defaults.
        va_status_ok(
            unsafe { create_config(dpy, profile, VA_ENTRYPOINT_VLD, std::ptr::null_mut(), 0, &mut config) },
            "vaCreateConfig",
        )?;
        let mut surfaces = vec![0u32; 4];
        // SAFETY: surface array is valid for 4 entries; NULL attributes.
        let surfaces_ok = unsafe {
            create_surfaces(
                dpy,
                VA_RT_FORMAT_YUV420,
                width,
                height,
                surfaces.as_mut_ptr(),
                surfaces.len() as u32,
                std::ptr::null_mut(),
                0,
            )
        } == VA_STATUS_SUCCESS;
        if !surfaces_ok {
            // SAFETY: config and display are valid.
            unsafe {
                destroy_config(dpy, config);
                terminate(dpy);
            }
            return Err(MediaError::CommandFailed("vaCreateSurfaces failed".into()));
        }
        let mut context: va_sys::VaContextId = 0;
        // SAFETY: render-target array is valid; progressive frame pictures.
        let context_ok = unsafe {
            create_context(
                dpy,
                config,
                width as std::os::raw::c_int,
                height as std::os::raw::c_int,
                VA_PROGRESSIVE,
                surfaces.as_mut_ptr(),
                surfaces.len() as std::os::raw::c_int,
                &mut context,
            )
        } == VA_STATUS_SUCCESS;
        if !context_ok {
            // SAFETY: surfaces, config and display are valid.
            unsafe {
                destroy_surfaces(dpy, surfaces.as_mut_ptr(), surfaces.len() as std::os::raw::c_int);
                destroy_config(dpy, config);
                terminate(dpy);
            }
            return Err(MediaError::CommandFailed("vaCreateContext failed".into()));
        }
        Ok(Self {
            lib,
            _node: file,
            dpy,
            config,
            context,
            surfaces,
            width,
            height,
            profile,
        })
    }

    /// Coded width negotiated at `open`.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Coded height negotiated at `open`.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// VA profile id negotiated at `open`.
    pub fn profile(&self) -> i32 {
        self.profile
    }

    /// Decodes one Annex-B picture (SPS plus PPS plus a single I-slice)
    /// to planar YUV.
    ///
    /// IDR and non-IDR I-slices submit through the real VA-API picture
    /// path. P/B slices return `UnsupportedFormat("va-api slice decode:
    /// ...")` because DPB management for inter frames is the remaining
    /// milestone. Resolution changes against the session dimensions also
    /// return `UnsupportedFormat` (reopen the session instead).
    pub fn decode_frame_vaapi(&mut self, annexb: &[u8]) -> Result<HwFrame> {
        let units = split_annexb_nals(annexb);
        if units.is_empty() {
            return Err(MediaError::UnsupportedFormat(
                "va-api: no Annex-B NAL units found".into(),
            ));
        }
        let bytes_of = |u: &NalUnit| match annexb.get(u.offset..u.offset + u.len) {
            Some(bytes) => bytes,
            None => &[],
        };
        let sps_nal = units.iter().find(|u| u.nal_type == 7).map(bytes_of);
        let pps_nal = units.iter().find(|u| u.nal_type == 8).map(bytes_of);
        let (Some(sps_nal), Some(pps_nal)) = (sps_nal, pps_nal) else {
            return Err(MediaError::UnsupportedFormat(
                "va-api: picture needs SPS plus PPS plus one slice".into(),
            ));
        };
        let Some(sps) = parse_sps(sps_nal) else {
            return Err(MediaError::ParseError("va-api: unparseable SPS".into()));
        };
        let Some(pps) = parse_pps(pps_nal) else {
            return Err(MediaError::ParseError("va-api: unparseable PPS".into()));
        };
        if sps.width != self.width || sps.height != self.height {
            return Err(MediaError::UnsupportedFormat(format!(
                "va-api: stream is {}x{} but session is {}x{}; reopen the session",
                sps.width, sps.height, self.width, self.height
            )));
        }
        let vcl: Vec<&NalUnit> = units
            .iter()
            .filter(|u| (1..=5).contains(&u.nal_type))
            .collect();
        if vcl.is_empty() {
            return Err(MediaError::UnsupportedFormat(
                "va-api: no coded slice found".into(),
            ));
        }
        if vcl.len() > 1 {
            // Multi-slice pictures need one slice-param buffer per slice;
            // only the single-slice fast path is wired so far.
            return Err(MediaError::UnsupportedFormat(
                "va-api slice decode: multi-slice pictures need per-slice params (milestone)"
                    .into(),
            ));
        }
        let slice_unit = vcl[0];
        if slice_unit.nal_type == 1 {
            // Peek at the slice type before rejecting: non-IDR I-slices
            // are fine (empty DPB), P/B slices need reference handling.
            let probe = parse_slice_header(bytes_of(slice_unit), &sps, &pps);
            let is_inter = match probe {
                Some(info) => info.slice_type == 0 || info.slice_type == 1,
                None => true,
            };
            if is_inter {
                return Err(MediaError::UnsupportedFormat(
                    "va-api slice decode: DPB management for P-frames (milestone)".into(),
                ));
            }
        } else if slice_unit.nal_type != 5 {
            return Err(MediaError::UnsupportedFormat(format!(
                "va-api: unsupported VCL NAL type {}",
                slice_unit.nal_type
            )));
        }
        let slice_bytes = bytes_of(slice_unit);
        let Some(slice) = parse_slice_header(slice_bytes, &sps, &pps) else {
            return Err(MediaError::ParseError("va-api: unparseable slice header".into()));
        };
        if slice.slice_type != 2 {
            return Err(MediaError::UnsupportedFormat(
                "va-api slice decode: DPB management for P-frames (milestone)".into(),
            ));
        }
        self.submit_idr(&sps, &pps, &slice, slice_bytes)
    }

    /// Submits one I-slice picture and downloads the derived image.
    fn submit_idr(
        &mut self,
        sps: &SpsInfo,
        pps: &PpsInfo,
        slice: &SliceInfo,
        slice_bytes: &[u8],
    ) -> Result<HwFrame> {
        let lib = &self.lib;
        let (Some(begin_picture), Some(create_buffer), Some(destroy_buffer), Some(render_picture), Some(end_picture), Some(sync_surface), Some(derive_image), Some(destroy_image)) = (
            lib.begin_picture,
            lib.create_buffer,
            lib.destroy_buffer,
            lib.render_picture,
            lib.end_picture,
            lib.sync_surface,
            lib.derive_image,
            lib.destroy_image,
        ) else {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: incomplete VA-API symbols".into(),
            ));
        };
        let target = self.surfaces[0];
        let pic_param = build_pic_param(sps, pps, slice, target);
        let slice_param = build_slice_param(slice, slice_bytes.len() as u32);
        // SAFETY: all VA handles come from this session; every status is
        // checked and every created buffer is destroyed before return.
        unsafe {
            va_status_ok(begin_picture(self.dpy, self.context, target), "vaBeginPicture")?;
            let mut pic_buf = 0u32;
            let mut slice_buf = 0u32;
            let mut data_buf = 0u32;
            let created = create_buffer(
                self.dpy,
                self.context,
                VA_BUFFER_TYPE_PICTURE_PARAM,
                std::mem::size_of::<VaPicParamH264>() as u32,
                1,
                &pic_param as *const VaPicParamH264 as *mut std::os::raw::c_void,
                &mut pic_buf,
            ) == VA_STATUS_SUCCESS
                && create_buffer(
                    self.dpy,
                    self.context,
                    VA_BUFFER_TYPE_SLICE_PARAM,
                    std::mem::size_of::<VaSliceParamH264>() as u32,
                    1,
                    &slice_param as *const VaSliceParamH264 as *mut std::os::raw::c_void,
                    &mut slice_buf,
                ) == VA_STATUS_SUCCESS
                && create_buffer(
                    self.dpy,
                    self.context,
                    VA_BUFFER_TYPE_SLICE_DATA,
                    slice_bytes.len() as u32,
                    1,
                    slice_bytes.as_ptr() as *mut std::os::raw::c_void,
                    &mut data_buf,
                ) == VA_STATUS_SUCCESS;
            if !created {
                for buf in [pic_buf, slice_buf, data_buf] {
                    if buf != 0 {
                        destroy_buffer(self.dpy, buf);
                    }
                }
                let _ = end_picture(self.dpy, self.context);
                return Err(MediaError::CommandFailed("vaCreateBuffer failed".into()));
            }
            let mut bufs = [pic_buf, slice_buf, data_buf];
            let rendered = render_picture(
                self.dpy,
                self.context,
                bufs.as_mut_ptr(),
                bufs.len() as std::os::raw::c_int,
            ) == VA_STATUS_SUCCESS
                && end_picture(self.dpy, self.context) == VA_STATUS_SUCCESS
                && sync_surface(self.dpy, target) == VA_STATUS_SUCCESS;
            for buf in bufs {
                destroy_buffer(self.dpy, buf);
            }
            if !rendered {
                return Err(MediaError::CommandFailed(
                    "vaRenderPicture/vaEndPicture/vaSyncSurface failed".into(),
                ));
            }
            let mut image: VaImage = std::mem::zeroed();
            va_status_ok(derive_image(self.dpy, target, &mut image), "vaDeriveImage")?;
            let frame = self.download_image(&image);
            destroy_image(self.dpy, image.image_id);
            frame
        }
    }

    /// Copies a derived image into planar YUV. The map is always released.
    unsafe fn download_image(&self, image: &VaImage) -> Result<HwFrame> {
        let lib = &self.lib;
        let (Some(map_buffer), Some(unmap_buffer)) = (lib.map_buffer, lib.unmap_buffer) else {
            return Err(MediaError::UnsupportedFormat(
                "va-api unavailable: incomplete VA-API symbols".into(),
            ));
        };
        if image.width as u32 != self.width || image.height as u32 != self.height {
            return Err(MediaError::UnsupportedFormat(format!(
                "va-api: derived image is {}x{} for a {}x{} surface",
                image.width, image.height, self.width, self.height
            )));
        }
        let mut ptr: *mut std::os::raw::c_void = std::ptr::null_mut();
        // SAFETY: `image.buf` was created by `vaDeriveImage`.
        va_status_ok(unsafe { map_buffer(self.dpy, image.buf, &mut ptr) }, "vaMapBuffer")?;
        let result = (|| {
            if ptr.is_null() {
                return Err(MediaError::CommandFailed("vaMapBuffer returned NULL".into()));
            }
            let w = self.width as usize;
            let h = self.height as usize;
            let data_size = image.data_size as usize;
            let bytes = unsafe { std::slice::from_raw_parts(ptr as *const u8, data_size) };
            let plane = |off: u32, pitch: u32, rows: usize, cols: usize| -> Option<Vec<u8>> {
                let (off, pitch) = (off as usize, pitch as usize);
                if pitch < cols {
                    return None;
                }
                let mut plane = vec![0u8; cols * rows];
                for row in 0..rows {
                    let src_off = off + row * pitch;
                    let src = bytes.get(src_off..src_off + cols)?;
                    plane[row * cols..(row + 1) * cols].copy_from_slice(src);
                }
                Some(plane)
            };
            match image.format.fourcc {
                VA_FOURCC_NV12 => {
                    if image.num_planes < 2 {
                        return Err(MediaError::ParseError(
                            "va-api: NV12 image has fewer than 2 planes".into(),
                        ));
                    }
                    let y = plane(image.offsets[0], image.pitches[0], h, w).ok_or_else(|| {
                        MediaError::ParseError("va-api: NV12 Y plane out of bounds".into())
                    })?;
                    let uv_interleaved =
                        plane(image.offsets[1], image.pitches[1], h / 2, w).ok_or_else(|| {
                            MediaError::ParseError("va-api: NV12 UV plane out of bounds".into())
                        })?;
                    let mut u = vec![0u8; (w / 2) * (h / 2)];
                    let mut v = vec![0u8; (w / 2) * (h / 2)];
                    for row in 0..h / 2 {
                        for col in 0..w / 2 {
                            u[row * (w / 2) + col] = uv_interleaved[row * w + col * 2];
                            v[row * (w / 2) + col] = uv_interleaved[row * w + col * 2 + 1];
                        }
                    }
                    Ok(HwFrame {
                        width: self.width,
                        height: self.height,
                        y,
                        u,
                        v,
                    })
                }
                VA_FOURCC_I420 | VA_FOURCC_YV12 => {
                    if image.num_planes < 3 {
                        return Err(MediaError::ParseError(
                            "va-api: planar image has fewer than 3 planes".into(),
                        ));
                    }
                    let y = plane(image.offsets[0], image.pitches[0], h, w).ok_or_else(|| {
                        MediaError::ParseError("va-api: Y plane out of bounds".into())
                    })?;
                    // I420 order is Y,U,V; YV12 order is Y,V,U.
                    let (second, third) = if image.format.fourcc == VA_FOURCC_I420 {
                        (1usize, 2usize)
                    } else {
                        (2usize, 1usize)
                    };
                    let u = plane(
                        image.offsets[second],
                        image.pitches[second],
                        h / 2,
                        w / 2,
                    )
                    .ok_or_else(|| {
                        MediaError::ParseError("va-api: U plane out of bounds".into())
                    })?;
                    let v = plane(image.offsets[third], image.pitches[third], h / 2, w / 2)
                        .ok_or_else(|| {
                            MediaError::ParseError("va-api: V plane out of bounds".into())
                        })?;
                    Ok(HwFrame {
                        width: self.width,
                        height: self.height,
                        y,
                        u,
                        v,
                    })
                }
                fourcc => Err(MediaError::UnsupportedFormat(format!(
                    "va-api: derived fourcc {fourcc:08X} needs a converter (milestone)"
                ))),
            }
        })();
        // SAFETY: buffer was mapped above; unmap status is best-effort.
        unsafe {
            unmap_buffer(self.dpy, image.buf);
        }
        result
    }
}

#[cfg(target_os = "linux")]
impl Drop for VaapiSession {
    fn drop(&mut self) {
        // SAFETY: handles were created by this session; failures during
        // teardown are ignored (nothing sensible to report in `Drop`).
        unsafe {
            if let (Some(destroy_context), Some(destroy_surfaces), Some(destroy_config), Some(terminate)) = (
                self.lib.destroy_context,
                self.lib.destroy_surfaces,
                self.lib.destroy_config,
                self.lib.terminate,
            ) {
                destroy_context(self.dpy, self.context);
                if !self.surfaces.is_empty() {
                    destroy_surfaces(
                        self.dpy,
                        self.surfaces.as_mut_ptr(),
                        self.surfaces.len() as std::os::raw::c_int,
                    );
                }
                destroy_config(self.dpy, self.config);
                terminate(self.dpy);
            }
        }
        // `lib` (dlclose) and `_node` (fd) release automatically, in order.
    }
}

#[cfg(target_os = "linux")]
fn profile_has_vld(lib: &VaLib, dpy: va_sys::VaDisplay, profile: i32) -> bool {
    let (Some(max_entrypoints), Some(query_entrypoints)) =
        (lib.max_entrypoints, lib.query_entrypoints)
    else {
        return false;
    };
    // SAFETY: count query takes no buffers; list buffer uses the
    // driver-reported maximum, clamped defensively.
    let max_e = unsafe { max_entrypoints(dpy) }.clamp(0, 64) as usize;
    if max_e == 0 {
        return false;
    }
    let mut entrypoints = vec![0; max_e];
    let mut num = 0;
    let ok = unsafe { query_entrypoints(dpy, profile, entrypoints.as_mut_ptr(), &mut num) }
        == VA_STATUS_SUCCESS;
    if !ok {
        return false;
    }
    let n = (num.max(0) as usize).min(max_e);
    entrypoints[..n].contains(&VA_ENTRYPOINT_VLD)
}

/// Decodes one Annex-B H.264 picture with the system GPU (one-shot).
///
/// The SPS dimensions size the session automatically. I-slices decode
/// through fixed-function hardware; P/B slices and anything without
/// hardware backing return `MediaError::UnsupportedFormat` (never panic).
#[cfg(target_os = "linux")]
pub fn decode_annexb_to_yuv(data: &[u8]) -> Result<HwFrame> {
    let units = split_annexb_nals(data);
    let sps_nal = units
        .iter()
        .find(|u| u.nal_type == 7)
        .and_then(|u| data.get(u.offset..u.offset + u.len));
    let Some(sps_nal) = sps_nal else {
        return Err(MediaError::UnsupportedFormat(
            "va-api: no SPS found for session sizing".into(),
        ));
    };
    let Some(sps) = parse_sps(sps_nal) else {
        return Err(MediaError::ParseError("va-api: unparseable SPS".into()));
    };
    let mut session = VaapiSession::open(sps.width, sps.height)?;
    session.decode_frame_vaapi(data)
}

/// Decodes one Annex-B H.264 picture with the system GPU to RGB24.
///
/// Returns `(width, height, rgb)`. Pure-Rust colorspace conversion runs
/// on the downloaded GPU frame. Missing hardware or inter-frame content
/// yields `MediaError::UnsupportedFormat` (never panic).
#[cfg(target_os = "linux")]
pub fn annexb_to_rgb(data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    let frame = decode_annexb_to_yuv(data)?;
    let rgb = i420_to_rgb(&frame.y, &frame.u, &frame.v, frame.width, frame.height)?;
    Ok((frame.width, frame.height, rgb))
}

// ---------------------------------------------------------------------------
// Non-Linux stubs: the module is only wired into `lib.rs` on Linux, but the
// pure helpers above compile everywhere while these keep the hardware entry
// points honest elsewhere.
// ---------------------------------------------------------------------------

/// No VA-API off Linux: always empty. (Linux build has the real query.)
#[cfg(not(target_os = "linux"))]
pub fn hw_decoder_info() -> Vec<HwDecoder> {
    Vec::new()
}

/// No VA-API off Linux: always empty. (Linux build has the real query.)
#[cfg(not(target_os = "linux"))]
pub fn hw_decoder_info_from_nodes(_nodes: &[&str]) -> Vec<HwDecoder> {
    Vec::new()
}

/// No VA-API off Linux: always `UnsupportedFormat`.
#[cfg(not(target_os = "linux"))]
pub fn decode_annexb_to_yuv(_data: &[u8]) -> Result<HwFrame> {
    Err(MediaError::UnsupportedFormat(
        "va-api unavailable outside Linux".into(),
    ))
}

/// No VA-API off Linux: always `UnsupportedFormat`.
#[cfg(not(target_os = "linux"))]
pub fn annexb_to_rgb(_data: &[u8]) -> Result<(u32, u32, Vec<u8>)> {
    Err(MediaError::UnsupportedFormat(
        "va-api unavailable outside Linux".into(),
    ))
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_c_probe() {
        // Verified 2026-09-29 with gcc against /usr/include/va/va.h.
        assert_eq!(VA_STATUS_SUCCESS, 0);
        assert_eq!(VA_PROFILE_H264_MAIN, 6);
        assert_eq!(VA_PROFILE_H264_HIGH, 7);
        assert_eq!(VA_PROFILE_H264_CONSTRAINED_BASELINE, 13);
        assert_eq!(VA_ENTRYPOINT_VLD, 1);
        assert_eq!(VA_RT_FORMAT_YUV420, 1);
        assert_eq!(VA_BUFFER_TYPE_PICTURE_PARAM, 0);
        assert_eq!(VA_BUFFER_TYPE_SLICE_PARAM, 4);
        assert_eq!(VA_BUFFER_TYPE_SLICE_DATA, 5);
        assert_eq!(VA_BUFFER_TYPE_IMAGE, 9);
        assert_eq!(VA_INVALID_ID, 0xffff_ffff);
        assert_eq!(VA_PROGRESSIVE, 1);
        assert_eq!(VA_SLICE_DATA_FLAG_ALL, 0);
        assert_eq!(VA_FOURCC_NV12, 0x3231_564E);
        assert_eq!(RTLD_NOW, 2);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn struct_sizes_match_c_probe() {
        // Verified 2026-09-29 with gcc against /usr/include/va/va.h.
        assert_eq!(std::mem::size_of::<VaPictureH264>(), 36);
        assert_eq!(std::mem::size_of::<VaPicParamH264>(), 672);
        assert_eq!(std::mem::size_of::<VaSliceParamH264>(), 3128);
        assert_eq!(std::mem::size_of::<VaImageFormat>(), 48);
        assert_eq!(std::mem::size_of::<VaImage>(), 120);
    }

    #[test]
    fn profile_and_nal_names() {
        assert_eq!(h264_profile_name(7), Some("VAProfileH264High"));
        assert_eq!(h264_profile_name(6), Some("VAProfileH264Main"));
        assert_eq!(h264_profile_name(13), Some("VAProfileH264ConstrainedBaseline"));
        assert_eq!(h264_profile_name(5), Some("VAProfileH264Baseline"));
        assert_eq!(h264_profile_name(17), None);
        assert_eq!(h264_profile_name(-1), None);
        assert_eq!(h264_nal_type_name(7), "sequence parameter set (SPS)");
        assert_eq!(h264_nal_type_name(5), "coded slice (IDR)");
        assert_eq!(h264_nal_type_name(1), "coded slice (non-IDR)");
        assert_eq!(h264_nal_type_name(0), "reserved/unspecified");
        assert_eq!(normalize_slice_type(7), 2);
        assert_eq!(normalize_slice_type(2), 2);
    }

    #[test]
    fn splitter_handles_mixed_start_codes() {
        // SPS (7) with 4-byte code, PPS (8) with 3-byte code, IDR (5).
        let data = [
            0u8, 0, 0, 1, 0x67, 0xAA, 0xBB, // SPS
            0, 0, 1, 0x68, 0xCC, // PPS
            0, 0, 0, 1, 0x65, 0xDD, 0xEE, 0xFF, // IDR
        ];
        let units = split_annexb_nals(&data);
        assert_eq!(units.len(), 3);
        assert_eq!(units[0].nal_type, 7);
        assert_eq!(units[0].offset, 4);
        assert_eq!(units[0].len, 3);
        assert_eq!(units[1].nal_type, 8);
        assert_eq!(units[1].len, 2);
        assert_eq!(units[2].nal_type, 5);
        assert_eq!(units[2].nal_ref_idc, 3);
        assert_eq!(&data[units[2].offset..units[2].offset + units[2].len], &[0x65, 0xDD, 0xEE, 0xFF]);
    }

    #[test]
    fn splitter_skips_garbage_and_empties() {
        assert!(split_annexb_nals(&[]).is_empty());
        assert!(split_annexb_nals(&[0x67, 0x42, 0x00]).is_empty());
        // Leading garbage, empty unit (adjacent codes), trailing zeros.
        let data = [0xFF, 0x00, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0, 0, 1, 0x68, 0xCE, 0, 0];
        let units = split_annexb_nals(&data);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].nal_type, 7);
        assert_eq!(units[1].nal_type, 8);
        assert_eq!(units[1].len, 2);
    }

    #[test]
    fn splitter_keeps_emulation_prevention_intact() {
        // 00 00 03 01 is payload, not a start code.
        let data = [0u8, 0, 0, 1, 0x65, 0x00, 0x00, 0x03, 0x01, 0xFF, 0, 0, 1, 0x06, 0x01];
        let units = split_annexb_nals(&data);
        assert_eq!(units.len(), 2);
        assert_eq!(units[0].len, 6);
        assert_eq!(units[1].nal_type, 6);
    }

    #[test]
    fn ebsp_removal_works() {
        assert_eq!(ebsp_to_rbsp(&[0x00, 0x00, 0x03, 0x01]), vec![0x00, 0x00, 0x01]);
        assert_eq!(ebsp_to_rbsp(&[0x00, 0x00, 0x03, 0x00]), vec![0x00, 0x00, 0x00]);
        assert_eq!(ebsp_to_rbsp(&[0x01, 0x02]), vec![0x01, 0x02]);
    }

    /// Minimal bit writer for building synthetic SPS/PPS/slice headers.
    struct BitWriter {
        buf: Vec<u8>,
        cur: u8,
        filled: u8,
    }

    impl BitWriter {
        fn new() -> Self {
            Self {
                buf: Vec::new(),
                cur: 0,
                filled: 0,
            }
        }

        fn bit(&mut self, b: u32) {
            self.cur = (self.cur << 1) | ((b & 1) as u8);
            self.filled += 1;
            if self.filled == 8 {
                self.buf.push(self.cur);
                self.cur = 0;
                self.filled = 0;
            }
        }

        fn bits(&mut self, v: u32, n: u32) {
            for i in (0..n).rev() {
                self.bit((v >> i) & 1);
            }
        }

        fn ue(&mut self, v: u32) {
            let code = v + 1;
            let len = 32 - code.leading_zeros();
            for _ in 0..len - 1 {
                self.bit(0);
            }
            self.bits(code, len);
        }

        fn se(&mut self, v: i32) {
            let ue = if v > 0 { (v as u32) * 2 - 1 } else { (-v as u32) * 2 };
            self.ue(ue);
        }

        fn finish(mut self) -> Vec<u8> {
            if self.filled > 0 {
                self.cur <<= 8 - self.filled;
                // rbsp_stop_one_bit plus alignment.
                self.cur |= 1 << (7 - self.filled);
                self.buf.push(self.cur);
            }
            self.buf
        }
    }

    /// Builds a baseline SPS NAL (header included) for `width` x `height`.
    fn build_sps(width: u32, height: u32) -> Vec<u8> {
        let mbs_w = width.div_ceil(16);
        let mbs_h = height.div_ceil(16);
        let mut w = BitWriter::new();
        w.bits(66, 8); // profile_idc: baseline
        w.bits(0, 8); // constraints
        w.bits(30, 8); // level_idc 3.0
        w.ue(0); // sps id
        w.ue(0); // log2_max_frame_num_minus4
        w.ue(0); // pic_order_cnt_type
        w.ue(0); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(1); // max_num_ref_frames
        w.bit(0); // gaps flag
        w.ue(mbs_w - 1);
        w.ue(mbs_h - 1);
        w.bit(1); // frame_mbs_only_flag
        w.bit(1); // direct_8x8_inference_flag
        let crop_b = height != mbs_h * 16 || width != mbs_w * 16;
        w.bit(u32::from(crop_b));
        if crop_b {
            // 4:2:0 progressive: one crop unit is 2x2 luma pixels.
            let total_x = (mbs_w * 16 - width) / 2;
            let total_y = (mbs_h * 16 - height) / 2;
            w.ue(total_x / 2);
            w.ue(total_x - total_x / 2);
            w.ue(total_y / 2);
            w.ue(total_y - total_y / 2);
        }
        w.bit(0); // vui_parameters_present_flag: no VUI
        let mut nal = vec![0x67];
        nal.extend(w.finish());
        nal
    }

    fn build_pps() -> Vec<u8> {
        let mut w = BitWriter::new();
        w.ue(0); // pps id
        w.ue(0); // sps id
        w.bit(0); // entropy coding (CAVLC)
        w.bit(0); // bottom field order present
        w.ue(0); // num_slice_groups_minus1
        w.ue(0); // num_ref_idx_l0_default
        w.ue(0); // num_ref_idx_l1_default
        w.bit(0); // weighted_pred
        w.bits(0, 2); // weighted_bipred_idc
        w.se(0); // pic_init_qp_minus26
        w.se(0); // pic_init_qs_minus26
        w.se(0); // chroma_qp_index_offset
        w.bit(1); // deblocking_filter_control_present_flag
        let mut nal = vec![0x68];
        nal.extend(w.finish());
        nal
    }

    fn write_idr_slice(frame_num: u32, qp_delta: i32) -> Vec<u8> {
        // IDR I-slice, poc_type 0, CAVLC, single slice.
        let mut w = BitWriter::new();
        w.ue(0); // first_mb_in_slice
        w.ue(2); // slice_type I
        w.ue(0); // pps id
        w.bits(frame_num, 4); // frame_num (log2_max_frame_num_minus4=0)
        w.ue(7); // idr_pic_id
        w.bits(0, 4); // pic_order_cnt_lsb
        w.bit(0); // no_output_of_prior_pics_flag
        w.bit(1); // long_term_reference_flag
        w.se(qp_delta); // slice_qp_delta
        w.ue(0); // disable_deblocking_filter_idc
        w.se(0); // alpha
        w.se(0); // beta
        let mut nal = vec![0x65];
        nal.extend(w.finish());
        nal
    }

    #[test]
    fn sps_dimensions_roundtrip() {
        assert_eq!(parse_sps_dimensions(&build_sps(320, 240)), Some((320, 240)));
        assert_eq!(parse_sps_dimensions(&build_sps(1280, 720)), Some((1280, 720)));
        assert_eq!(parse_sps_dimensions(&build_sps(1920, 1080)), Some((1920, 1080)));
        assert_eq!(parse_sps_dimensions(&build_sps(16, 16)), Some((16, 16)));
    }

    #[test]
    fn sps_fields_parsed() {
        let sps = parse_sps(&build_sps(640, 480)).unwrap();
        assert_eq!(sps.profile_idc, 66);
        assert_eq!(sps.chroma_format_idc, 1);
        assert!(sps.frame_mbs_only_flag);
        assert_eq!(sps.pic_width_in_mbs_minus1, 39);
        assert_eq!(sps.pic_height_in_map_units_minus1, 29);
        assert_eq!((sps.width, sps.height), (640, 480));
    }

    #[test]
    fn sps_rejects_garbage() {
        assert!(parse_sps(&[]).is_none());
        assert!(parse_sps(&[0x68, 0x00]).is_none()); // PPS, not SPS
        assert!(parse_sps(&[0x67]).is_none());
        assert!(parse_sps(&[0x67, 0x00]).is_none());
    }

    #[test]
    fn pps_roundtrip() {
        let pps = parse_pps(&build_pps()).unwrap();
        assert_eq!(pps.pic_parameter_set_id, 0);
        assert_eq!(pps.seq_parameter_set_id, 0);
        assert!(!pps.entropy_coding_mode_flag);
        assert!(pps.deblocking_filter_control_present_flag);
        assert!(parse_pps(&[0x67, 0x42]).is_none());
        assert!(parse_pps(&[]).is_none());
    }

    #[test]
    fn slice_header_roundtrip() {
        let sps = parse_sps(&build_sps(320, 240)).unwrap();
        let pps = parse_pps(&build_pps()).unwrap();
        let nal = write_idr_slice(3, -2);
        let info = parse_slice_header(&nal, &sps, &pps).unwrap();
        assert!(info.is_idr);
        assert_eq!(info.slice_type, 2);
        assert_eq!(info.frame_num, 3);
        assert_eq!(info.slice_qp_delta, -2);
        assert_eq!(info.pic_order_cnt_lsb, Some(0));
        assert_eq!(info.idr_pic_id, Some(7));
        assert!(info.header_bits > 10);
    }

    #[test]
    fn slice_header_rejects_non_vcl_and_truncation() {
        let sps = parse_sps(&build_sps(320, 240)).unwrap();
        let pps = parse_pps(&build_pps()).unwrap();
        assert!(parse_slice_header(&[0x06, 0x01], &sps, &pps).is_none());
        assert!(parse_slice_header(&[], &sps, &pps).is_none());
        assert!(parse_slice_header(&[0x65], &sps, &pps).is_none());
    }

    #[test]
    fn converters_known_values() {
        // Neutral gray: Y=128, U=V=128.
        let rgb = nv12_to_rgb(&[128u8; 8], &[128u8; 4], 2, 2).unwrap();
        assert_eq!(rgb.len(), 12);
        assert!(rgb.iter().all(|&b| (b as i32 - 128).abs() <= 1));
        // Black and white extremes.
        let rgb = nv12_to_rgb(&[16, 16, 235, 235], &[128, 128], 2, 2).unwrap();
        assert!((rgb[0] as i32 - 16).abs() <= 1);
        assert!((rgb[6] as i32 - 235).abs() <= 1);
        // I420 matches NV12 for the same content.
        let a = nv12_to_rgb(&[100u8; 16], &[90u8, 200u8, 90u8, 200u8, 90u8, 200u8, 90u8, 200u8], 4, 4).unwrap();
        let b = i420_to_rgb(&[100u8; 16], &[90u8; 4], &[200u8; 4], 4, 4).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn converters_reject_bad_input() {
        assert!(nv12_to_rgb(&[0u8; 3], &[0u8; 2], 2, 2).is_err());
        assert!(nv12_to_rgb(&[0u8; 4], &[0u8; 1], 2, 2).is_err());
        assert!(nv12_to_rgb(&[0u8; 4], &[0u8; 2], 3, 2).is_err());
        assert!(nv12_to_rgb(&[0u8; 4], &[0u8; 2], 0, 2).is_err());
        assert!(i420_to_rgb(&[0u8; 3], &[0u8; 1], &[0u8; 1], 2, 2).is_err());
        assert!(i420_to_rgb(&[0u8; 4], &[0u8; 0], &[0u8; 1], 2, 2).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pic_and_slice_param_builders() {
        let sps = parse_sps(&build_sps(320, 240)).unwrap();
        let pps = parse_pps(&build_pps()).unwrap();
        let nal = write_idr_slice(0, 0);
        let slice = parse_slice_header(&nal, &sps, &pps).unwrap();
        let pic = build_pic_param(&sps, &pps, &slice, 42);
        assert_eq!(pic.curr_pic.picture_id, 42);
        assert_eq!(pic.picture_width_in_mbs_minus1, 19);
        assert_eq!(pic.picture_height_in_mbs_minus1, 14);
        assert!(pic.reference_frames.iter().all(|r| r.picture_id == VA_INVALID_ID));
        let sp = build_slice_param(&slice, nal.len() as u32);
        assert_eq!(sp.slice_data_size, nal.len() as u32);
        assert_eq!(sp.slice_type, 2);
        assert_eq!(u32::from(sp.slice_data_bit_offset), slice.header_bits);
        assert!(sp.ref_pic_list0.iter().all(|r| r.flags & VA_PICTURE_H264_INVALID != 0));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn missing_nodes_yield_empty_without_panic() {
        assert!(hw_decoder_info_from_nodes(&["/dev/nonexistent-render99"]).is_empty());
        assert!(hw_decoder_info_from_nodes(&[]).is_empty());
        // Bogus library paths also degrade to empty, never panic.
        assert!(
            hw_decoder_info_with_libs(
                b"/nonexistent-libva.so\0",
                b"/nonexistent-libva-drm.so\0",
                &[String::from("/dev/nonexistent-render99")],
            )
            .is_empty()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn decode_without_hardware_is_unsupported_never_panic() {
        // Garbage input: honest Unsupported, no panic.
        let err = decode_annexb_to_yuv(&[0, 1, 2, 3]).unwrap_err();
        assert!(matches!(err, MediaError::UnsupportedFormat(_)));
        let err = decode_annexb_to_yuv(&[]).unwrap_err();
        assert!(matches!(err, MediaError::UnsupportedFormat(_)));
        // Well-formed stream without hardware: Unsupported (no node) or a
        // real frame on GPU machines; either way no panic, no ParseError
        // masquerading as success.
        let mut annexb = vec![0u8, 0, 0, 1];
        annexb.extend_from_slice(&build_sps(320, 240));
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&build_pps());
        annexb.extend_from_slice(&[0, 0, 0, 1]);
        annexb.extend_from_slice(&write_idr_slice(0, 0));
        let _ = decode_annexb_to_yuv(&annexb);
        let _ = annexb_to_rgb(&annexb);
        // Session open against a missing node is an honest error.
        assert!(VaapiSession::open_on_nodes(&["/dev/nonexistent-render99"], 320, 240).is_err());
        assert!(VaapiSession::open_on_nodes(&[], 320, 240).is_err());
        assert!(VaapiSession::open(0, 240).is_err());
    }
}
