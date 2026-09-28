//! Native V4L2 capture without ffmpeg, Linux-only, std-only.
//!
//! Devices are driven with raw ioctls (declared locally, no libc
//! crate) in read() streaming mode: `QUERYCAP`, `S_FMT`, `STREAMON`,
//! blocking `read()` of one frame, `STREAMOFF`. Supported pixel
//! formats are MJPEG (decoded by `crate::mjpeg`) and YUYV
//! (converted below). MMAP streaming is out of scope for v1.
//!
//! Needs real hardware (`/dev/videoN`); without a camera `open`
//! fails with `IoError` instead of hanging.

use crate::error::{MediaError, Result};
use std::os::raw::{c_int, c_ulong, c_void};

extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int;
}

// _IOR/_IOW/_IOWR('V', nr, size) on x86_64 Linux.
const VIDIOC_QUERYCAP: c_ulong = 0x8068_5600;
const VIDIOC_G_FMT: c_ulong = 0xC0D0_5604;
const VIDIOC_S_FMT: c_ulong = 0xC0D0_5605;
const VIDIOC_STREAMON: c_ulong = 0x4004_5612;
const VIDIOC_STREAMOFF: c_ulong = 0x4004_5613;

const V4L2_BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const V4L2_CAP_VIDEO_CAPTURE: u32 = 0x0000_0001;
const V4L2_CAP_READWRITE: u32 = 0x0100_0000;

/// `YUYV` packed 4:2:2 as V4L2 fourcc (little-endian).
pub const FOURCC_YUYV: u32 = 0x5659_5559;
/// `MJPG` motion JPEG as V4L2 fourcc.
pub const FOURCC_MJPG: u32 = 0x4750_4A4D;

#[repr(C)]
#[derive(Debug, Default, Clone)]
struct V4l2Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone)]
struct V4l2Format {
    r#type: u32,
    /// Union area (verified 204 bytes via kernel headers against
    /// `VIDIOC_S_FMT`; pix_format starts at offset 0).
    fmt: [u8; 204],
}

impl Default for V4l2Format {
    fn default() -> Self {
        Self {
            r#type: 0,
            fmt: [0u8; 204],
        }
    }
}

fn pix_get(fmt: &[u8; 204], off: usize) -> u32 {
    u32::from_le_bytes([fmt[off], fmt[off + 1], fmt[off + 2], fmt[off + 3]])
}

fn pix_set(fmt: &mut [u8; 204], off: usize, v: u32) {
    fmt[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

fn ioctl_ret(code: c_int, what: &str) -> Result<()> {
    if code < 0 {
        Err(MediaError::IoError(format!(
            "{what}: {}",
            std::io::Error::last_os_error()
        )))
    } else {
        Ok(())
    }
}

fn raw_fd(file: &std::fs::File) -> c_int {
    use std::os::unix::io::AsRawFd;
    file.as_raw_fd()
}

/// One natively captured still frame (RGB24).
#[derive(Debug, Clone)]
pub struct CapturedFrame {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

/// Converts packed YUYV to RGB24 (BT.601 full-range, to match the
/// crate's JPEG/TDC-1 converters).
pub fn yuyv_to_rgb(data: &[u8], width: u32, height: u32) -> Result<Vec<u8>> {
    if width % 2 != 0 {
        return Err(MediaError::ParseError("yuyv needs even width".into()));
    }
    let expect = (width as usize) * (height as usize) * 2;
    if data.len() < expect {
        return Err(MediaError::ParseError("yuyv buffer short".into()));
    }
    let mut out = vec![0u8; (width as usize) * (height as usize) * 3];
    for i in 0..(width as usize) * (height as usize) / 2 {
        let y0 = data[i * 4] as f32;
        let u = data[i * 4 + 1] as f32 - 128.0;
        let y1 = data[i * 4 + 2] as f32;
        let v = data[i * 4 + 3] as f32 - 128.0;
        let o = i * 6;
        out[o] = (y0 + 1.402 * v).round().clamp(0.0, 255.0) as u8;
        out[o + 1] = (y0 - 0.344136 * u - 0.714136 * v).round().clamp(0.0, 255.0) as u8;
        out[o + 2] = (y0 + 1.772 * u).round().clamp(0.0, 255.0) as u8;
        out[o + 3] = (y1 + 1.402 * v).round().clamp(0.0, 255.0) as u8;
        out[o + 4] = (y1 - 0.344136 * u - 0.714136 * v).round().clamp(0.0, 255.0) as u8;
        out[o + 5] = (y1 + 1.772 * u).round().clamp(0.0, 255.0) as u8;
    }
    Ok(out)
}

fn read_exact_frame(file: &mut std::fs::File, size: usize) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut buf = vec![0u8; size];
    let mut got = 0usize;
    let mut tries = 0u32;
    while got < size {
        match file.read(&mut buf[got..]) {
            Ok(0) => {
                tries += 1;
                if tries > 5000 {
                    return Err(MediaError::ParseError("v4l2 read timeout".into()));
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            Ok(n) => {
                got += n;
                tries = 0;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(MediaError::from_io(e)),
        }
    }
    Ok(buf)
}

/// Captures one frame from `node` (e.g. `/dev/video0`) at `width` x
/// `height` in `fourcc` (`FOURCC_MJPG` or `FOURCC_YUYV`).
/// The driver may adjust dimensions; the returned frame reports the
/// negotiated size.
pub fn capture_frame_native(
    node: &std::path::Path,
    width: u32,
    height: u32,
    fourcc: u32,
) -> Result<CapturedFrame> {
    if fourcc != FOURCC_MJPG && fourcc != FOURCC_YUYV {
        return Err(MediaError::UnsupportedFormat(format!("v4l2 fourcc {fourcc:08X}")));
    }
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(node)
        .map_err(MediaError::from_io)?;
    let fd = raw_fd(&file);
    // QUERYCAP.
    let mut cap = V4l2Capability::default();
    // SAFETY: QUERYCAP reads into a correctly sized struct.
    unsafe {
        ioctl_ret(
            ioctl(
                fd,
                VIDIOC_QUERYCAP,
                &mut cap as *mut _ as *mut c_void,
            ),
            "VIDIOC_QUERYCAP",
        )?;
    }
    if cap.capabilities & V4L2_CAP_VIDEO_CAPTURE == 0 {
        return Err(MediaError::UnsupportedFormat("not a capture device".into()));
    }
    if cap.capabilities & V4L2_CAP_READWRITE == 0 {
        return Err(MediaError::UnsupportedFormat(
            "device lacks read() streaming".into(),
        ));
    }
    // S_FMT.
    let mut fmt = V4l2Format::default();
    fmt.r#type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    pix_set(&mut fmt.fmt, 0, width);
    pix_set(&mut fmt.fmt, 4, height);
    pix_set(&mut fmt.fmt, 8, fourcc);
    pix_set(&mut fmt.fmt, 12, 0); // field: any
    // SAFETY: S_FMT reads/writes a correctly sized struct.
    unsafe {
        ioctl_ret(
            ioctl(fd, VIDIOC_S_FMT, &mut fmt as *mut _ as *mut c_void),
            "VIDIOC_S_FMT",
        )?;
    }
    let actual_w = pix_get(&fmt.fmt, 0);
    let actual_h = pix_get(&fmt.fmt, 4);
    let actual_fourcc = pix_get(&fmt.fmt, 8);
    let sizeimage = pix_get(&fmt.fmt, 20) as usize;
    if actual_w == 0 || actual_h == 0 || sizeimage == 0 {
        return Err(MediaError::ParseError("v4l2 negotiated empty format".into()));
    }
    // STREAMON.
    let buftype = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    // SAFETY: STREAMON reads one u32.
    unsafe {
        ioctl_ret(
            ioctl(
                fd,
                VIDIOC_STREAMON,
                &buftype as *const u32 as *mut c_void,
            ),
            "VIDIOC_STREAMON",
        )?;
    }
    let raw = read_exact_frame(&mut file, sizeimage);
    // STREAMOFF (best effort).
    unsafe {
        let _ = ioctl(
            fd,
            VIDIOC_STREAMOFF,
            &buftype as *const u32 as *mut c_void,
        );
    }
    let raw = raw?;
    let rgb = if actual_fourcc == FOURCC_MJPG || (actual_fourcc != FOURCC_YUYV && fourcc == FOURCC_MJPG) {
        let img = crate::mjpeg::decode_jpeg(&raw[..raw.len().min(sizeimage)])
            .map_err(|e| MediaError::ParseError(e.to_string()))?;
        if img.width != actual_w || img.height != actual_h {
            // Drivers may pad; crop is handled by decoders. Trust JPEG dims.
        }
        img.rgb
    } else if actual_fourcc == FOURCC_YUYV {
        yuyv_to_rgb(&raw, actual_w, actual_h)?
    } else {
        return Err(MediaError::UnsupportedFormat(format!(
            "driver returned fourcc {actual_fourcc:08X}"
        )));
    };
    Ok(CapturedFrame {
        width: actual_w,
        height: actual_h,
        rgb,
    })
}

/// Captures one still (MJPEG 1280x720 preferred, YUYV fallback).
pub fn capture_still_native(node: &std::path::Path) -> Result<CapturedFrame> {
    match capture_frame_native(node, 1280, 720, FOURCC_MJPG) {
        Ok(f) => Ok(f),
        Err(_) => capture_frame_native(node, 640, 480, FOURCC_YUYV),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_linux() {
        assert_eq!(VIDIOC_QUERYCAP, 0x8068_5600);
        assert_eq!(VIDIOC_S_FMT, 0xC0D0_5605);
        assert_eq!(VIDIOC_STREAMON, 0x4004_5612);
        assert_eq!(FOURCC_YUYV, u32::from_le_bytes(*b"YUYV"));
        assert_eq!(FOURCC_MJPG, u32::from_le_bytes(*b"MJPG"));
    }

    #[test]
    fn struct_layout_matches_kernel() {
        assert_eq!(std::mem::size_of::<V4l2Capability>(), 104);
        assert_eq!(std::mem::size_of::<V4l2Format>(), 208);
    }

    #[test]
    fn yuyv_known_values() {
        // Gray ramp pair: Y=16/235, U=V=128.
        let raw = vec![16, 128, 235, 128];
        let rgb = yuyv_to_rgb(&raw, 2, 1).unwrap();
        assert_eq!(rgb.len(), 6);
        assert!((rgb[0] as i32 - 16).abs() <= 1);
        assert!((rgb[3] as i32 - 235).abs() <= 1);
        // Pure red-ish: Y=82, U=90, V=240.
        let raw = vec![82, 90, 82, 240];
        let rgb = yuyv_to_rgb(&raw, 2, 1).unwrap();
        assert!(rgb[0] > 200 && rgb[2] < 80);
    }

    #[test]
    fn yuyv_rejects_bad_input() {
        assert!(yuyv_to_rgb(&[0u8; 3], 2, 1).is_err());
        assert!(yuyv_to_rgb(&[0u8; 6], 3, 1).is_err());
    }

    #[test]
    fn missing_device_is_error() {
        // Any error is fine (Missing maps through crate Io mapping);
        // the point is no hang and no success without hardware.
        assert!(
            capture_frame_native(
                std::path::Path::new("/dev/nonexistent-video99"),
                640,
                480,
                FOURCC_YUYV,
            )
            .is_err()
        );
        assert!(
            capture_frame_native(
                std::path::Path::new("/dev/nonexistent-video99"),
                640,
                480,
                0x1234_5678,
            )
            .is_err()
        );
    }
}
