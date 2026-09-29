//! Device management: capture devices + video outputs.
//!
//! Cameras delegate to `capture::list_cameras`. Outputs enumerate
//! DRM/KMS connectors on Linux (`/dev/dri/card*`, no tools);
//! headless machines return an empty list instead of an error.

use crate::capture::{list_cameras, CameraDevice};
use crate::error::Result;

#[derive(Debug, Clone)]
pub struct VideoOutput {
    pub id: String,
    pub label: String,
    pub primary: bool,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Available cameras / capture devices.
pub fn available_cameras() -> Result<Vec<CameraDevice>> {
    list_cameras()
}

/// Available video outputs (displays / external sinks).
/// Native DRM/KMS enumeration on Linux; empty elsewhere.
pub fn available_outputs() -> Vec<VideoOutput> {
    #[cfg(target_os = "linux")]
    {
        drm_outputs()
    }
    #[cfg(not(target_os = "linux"))]
    {
        Vec::new()
    }
}

/// DRM connector type id to a display name (`HDMI-A-1`, ...).
#[cfg(target_os = "linux")]
fn drm_connector_name(ctype: u32, type_id: u32) -> String {
    let type_id = type_id.max(1);
    let base = match ctype {
        1 => "VGA",
        2 => "DVI-I",
        3 => "DVI-D",
        4 => "DVI-A",
        10 => "DP",
        11 => "HDMI-A",
        12 => "HDMI-B",
        13 => "TV",
        14 => "eDP",
        15 => "VIRTUAL",
        16 => "DSI",
        17 => "DPI",
        _ => return format!("CONN{ctype}-{type_id}"),
    };
    format!("{base}-{type_id}")
}

/// Picks the preferred mode, else the largest, from packed
/// `drm_mode_modeinfo` entries (68 bytes each).
#[cfg(target_os = "linux")]
fn drm_best_mode(modes: &[u8]) -> Option<(u32, u32)> {
    const MODEINFO_SIZE: usize = 68;
    const PREFERRED: u32 = 0x8;
    let n = modes.len() / MODEINFO_SIZE;
    let mut best: Option<(u32, u32, bool, u64)> = None;
    for i in 0..n {
        let off = i * MODEINFO_SIZE;
        let h = u16::from_le_bytes([modes[off + 4], modes[off + 5]]) as u32;
        let v = u16::from_le_bytes([modes[off + 14], modes[off + 15]]) as u32;
        let mtype = u32::from_le_bytes([
            modes[off + 32],
            modes[off + 33],
            modes[off + 34],
            modes[off + 35],
        ]);
        if h == 0 || v == 0 {
            continue;
        }
        let area = h as u64 * v as u64;
        let pref = mtype & PREFERRED != 0;
        let replace = match best {
            None => true,
            Some((_, _, best_pref, best_area)) => {
                (pref && !best_pref) || (pref == best_pref && area > best_area)
            }
        };
        if replace {
            best = Some((h, v, pref, area));
        }
    }
    best.map(|(h, v, _, _)| (h, v))
}

/// Native DRM/KMS connector scan, std-only (ioctl declared locally).
/// Numbers and layouts verified against kernel `drm/drm_mode.h`.
#[cfg(target_os = "linux")]
fn drm_outputs() -> Vec<VideoOutput> {
    use std::os::raw::{c_int, c_ulong, c_void};

    extern "C" {
        fn ioctl(fd: c_int, request: c_ulong, arg: *mut c_void) -> c_int;
    }

    const DRM_IOCTL_MODE_GETRESOURCES: c_ulong = 0xC040_64A0;
    const DRM_IOCTL_MODE_GETCONNECTOR: c_ulong = 0xC050_64A7;
    const DRM_MODE_CONNECTED: u32 = 1;
    const MODEINFO_SIZE: usize = 68;

    fn ioctl_ok(fd: c_int, req: c_ulong, arg: *mut c_void) -> bool {
        // SAFETY: callers pass correctly sized structs/buffers.
        unsafe { ioctl(fd, req, arg) == 0 }
    }

    let mut out = Vec::new();
    for card in 0..32 {
        let path = format!("/dev/dri/card{card}");
        let file = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(_) => continue,
        };
        use std::os::unix::io::AsRawFd;
        let fd = file.as_raw_fd();
        // GETRESOURCES, pass 1: counts.
        let mut res = [0u8; 64];
        if !ioctl_ok(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr() as *mut c_void) {
            continue;
        }
        let n_conn = u32::from_le_bytes([res[40], res[41], res[42], res[43]]) as usize;
        if n_conn == 0 || n_conn > 64 {
            continue;
        }
        // Pass 2: connector ids.
        let mut conn_ids = vec![0u32; n_conn];
        res[16..24].copy_from_slice(&(conn_ids.as_mut_ptr() as u64).to_le_bytes());
        if !ioctl_ok(fd, DRM_IOCTL_MODE_GETRESOURCES, res.as_mut_ptr() as *mut c_void) {
            continue;
        }
        for cid in conn_ids {
            // GETCONNECTOR, pass 1: count_modes (connector_id at 48).
            let mut conn = [0u8; 80];
            conn[48..52].copy_from_slice(&cid.to_le_bytes());
            if !ioctl_ok(fd, DRM_IOCTL_MODE_GETCONNECTOR, conn.as_mut_ptr() as *mut c_void) {
                continue;
            }
            let connection = u32::from_le_bytes([conn[60], conn[61], conn[62], conn[63]]);
            if connection != DRM_MODE_CONNECTED {
                continue;
            }
            let ctype = u32::from_le_bytes([conn[52], conn[53], conn[54], conn[55]]);
            let type_id = u32::from_le_bytes([conn[56], conn[57], conn[58], conn[59]]);
            let n_modes =
                u32::from_le_bytes([conn[32], conn[33], conn[34], conn[35]]) as usize;
            let mut modes = vec![0u8; n_modes.min(256) * MODEINFO_SIZE];
            if modes.is_empty() {
                continue;
            }
            conn[8..16].copy_from_slice(&(modes.as_mut_ptr() as u64).to_le_bytes());
            if !ioctl_ok(fd, DRM_IOCTL_MODE_GETCONNECTOR, conn.as_mut_ptr() as *mut c_void) {
                continue;
            }
            if let Some((w, h)) = drm_best_mode(&modes) {
                let name = drm_connector_name(ctype, type_id);
                out.push(VideoOutput {
                    id: name.clone(),
                    label: name,
                    primary: out.is_empty(),
                    width: Some(w),
                    height: Some(h),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outputs_never_panic_headless() {
        let _ = available_outputs();
        let _ = available_cameras();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn drm_helpers_behave() {
        assert_eq!(super::drm_connector_name(11, 1), "HDMI-A-1");
        assert_eq!(super::drm_connector_name(14, 0), "eDP-1");
        assert_eq!(super::drm_connector_name(99, 2), "CONN99-2");
        // Two synthetic modes: larger non-preferred vs preferred.
        let mut modes = vec![0u8; 2 * 68];
        // Mode 0: 2560x1440, not preferred.
        modes[4..6].copy_from_slice(&2560u16.to_le_bytes());
        modes[14..16].copy_from_slice(&1440u16.to_le_bytes());
        // Mode 1: 1920x1080, preferred flag.
        modes[68 + 4..68 + 6].copy_from_slice(&1920u16.to_le_bytes());
        modes[68 + 14..68 + 16].copy_from_slice(&1080u16.to_le_bytes());
        modes[68 + 32..68 + 36].copy_from_slice(&0x8u32.to_le_bytes());
        assert_eq!(super::drm_best_mode(&modes), Some((1920, 1080)));
        assert_eq!(super::drm_best_mode(&[]), None);
    }
}
