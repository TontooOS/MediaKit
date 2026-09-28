//! C FFI exports for MediaKit.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use crate::{ChapterList, SubtitleTrack, VideoPlayer};

static PLAYERS: OnceLock<Mutex<std::collections::HashMap<u64, VideoPlayer>>> =
    OnceLock::new();
static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn players_lock() -> &'static Mutex<std::collections::HashMap<u64, VideoPlayer>> {
    PLAYERS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn set_error(error_out: *mut *mut c_char, message: &str) {
    if error_out.is_null() {
        return;
    }
    if let Ok(c) = CString::new(message.to_owned()) {
        unsafe { *error_out = c.into_raw() };
    }
}

fn json_ptr(value: &Value) -> *mut c_char {
    CString::new(value.to_string())
        .unwrap_or_default()
        .into_raw()
}

fn c_str(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(ptr).to_str().ok().map(|s| s.to_owned()) }
}

/// Library version (static string, do not free).
#[no_mangle]
pub extern "C" fn tontoo_mediakit_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

/// Frees a string returned by MediaKit.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_string_free(ptr: *mut c_char) {
    if !ptr.is_null() {
        unsafe { let _ = CString::from_raw(ptr); }
    }
}

/// Opens a video file and returns a player id (0 on error).
#[no_mangle]
pub extern "C" fn tontoo_mediakit_open(
    path: *const c_char,
    error_out: *mut *mut c_char,
) -> u64 {
    let Some(path) = c_str(path) else {
        set_error(error_out, "invalid path");
        return 0;
    };
    let mut player = VideoPlayer::new();
    match player.open(Path::new(&path)) {
        Ok(()) => {
            let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            players_lock().lock().ok().map(|mut m| m.insert(id, player));
            id
        }
        Err(e) => {
            set_error(error_out, &e.to_string());
            0
        }
    }
}

fn with_player<T>(id: u64, error_out: *mut *mut c_char, f: impl FnOnce(&mut VideoPlayer) -> T) -> Option<T> {
    let mut lock = players_lock().lock().ok()?;
    let player = lock.get_mut(&id)?;
    if player as *mut VideoPlayer as usize == 0 {
        set_error(error_out, "stream not found");
        return None;
    }
    Some(f(player))
}

fn call_player(id: u64, error_out: *mut *mut c_char, op: &str) -> i32 {
    let result = players_lock()
        .lock()
        .ok()
        .and_then(|mut m| {
            let player = m.get_mut(&id)?;
            let r = match op {
                "play" => player.play(),
                "pause" => player.pause(),
                "stop" => player.stop(),
                _ => return None,
            };
            Some(r)
        });
    match result {
        Some(Ok(())) => 0,
        Some(Err(e)) => {
            set_error(error_out, &e.to_string());
            1
        }
        None => {
            set_error(error_out, "stream not found");
            1
        }
    }
}

/// Starts playback. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_play(id: u64, error_out: *mut *mut c_char) -> i32 {
    call_player(id, error_out, "play")
}

/// Pauses playback. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_pause(id: u64, error_out: *mut *mut c_char) -> i32 {
    call_player(id, error_out, "pause")
}

/// Stops playback and resets position. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_stop(id: u64, error_out: *mut *mut c_char) -> i32 {
    call_player(id, error_out, "stop")
}

/// Seeks to `position_secs`. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_seek(
    id: u64,
    position_secs: f64,
    error_out: *mut *mut c_char,
) -> i32 {
    let result = players_lock().lock().ok().and_then(|mut m| {
        let player = m.get_mut(&id)?;
        Some(player.seek(position_secs))
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(e)) => {
            set_error(error_out, &e.to_string());
            1
        }
        None => {
            set_error(error_out, "stream not found");
            1
        }
    }
}

/// Sets playback speed (0.5-2.0). Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_set_speed(
    id: u64,
    speed: f32,
    error_out: *mut *mut c_char,
) -> i32 {
    let result = players_lock().lock().ok().and_then(|mut m| {
        let player = m.get_mut(&id)?;
        Some(player.set_speed(speed))
    });
    match result {
        Some(Ok(())) => 0,
        Some(Err(e)) => {
            set_error(error_out, &e.to_string());
            1
        }
        None => {
            set_error(error_out, "stream not found");
            1
        }
    }
}

/// Removes a player. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_close(id: u64) -> i32 {
    match players_lock().lock().ok().and_then(|mut m| m.remove(&id)) {
        Some(_) => 0,
        None => 1,
    }
}

/// Player state: 0 stopped, 1 playing, 2 paused, 3 finished, -1 unknown.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_state(id: u64) -> i32 {
    let lock = players_lock().lock().ok();
    let player = lock.as_ref().and_then(|m| m.get(&id));
    match player.map(|p| p.state()) {
        Some(crate::PlaybackState::Stopped) => 0,
        Some(crate::PlaybackState::Playing) => 1,
        Some(crate::PlaybackState::Paused) => 2,
        Some(crate::PlaybackState::Finished) => 3,
        None => -1,
    }
}

/// Video metadata as JSON, or NULL with `error_out` set.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_metadata(
    path: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(path) = c_str(path) else {
        set_error(error_out, "invalid path");
        return std::ptr::null_mut();
    };
    match crate::read_metadata(Path::new(&path)) {
        Ok(meta) => json_ptr(&json!({
            "duration_secs": meta.duration_secs,
            "width": meta.width,
            "height": meta.height,
            "video_codec": meta.video_codec,
            "audio_codec": meta.audio_codec,
            "framerate": meta.framerate,
            "container": meta.container,
        })),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Camera list as JSON array, or NULL with `error_out` set.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_list_cameras(error_out: *mut *mut c_char) -> *mut c_char {
    match crate::list_cameras() {
        Ok(cameras) => {
            let arr: Vec<Value> = cameras
                .iter()
                .map(|c| {
                    json!({
                        "id": c.id,
                        "label": c.label,
                        "node": c.node.to_string_lossy(),
                    })
                })
                .collect();
            json_ptr(&Value::Array(arr))
        }
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Loads subtitles into a player from `subtitle_path`. Returns 0 on success.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_load_subtitles(
    id: u64,
    subtitle_path: *const c_char,
    error_out: *mut *mut c_char,
) -> i32 {
    let Some(sub) = c_str(subtitle_path) else {
        set_error(error_out, "invalid subtitle path");
        return 1;
    };
    let track = match SubtitleTrack::from_file(Path::new(&sub)) {
        Ok(t) => t,
        Err(e) => {
            set_error(error_out, &e.to_string());
            return 1;
        }
    };
    match with_player(id, error_out, |p| p.set_subtitles(Some(track))) {
        Some(()) => 0,
        None => {
            set_error(error_out, "stream not found");
            1
        }
    }
}

/// Chapter list of `path` as JSON array, or NULL with `error_out` set.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_chapters(
    path: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(path) = c_str(path) else {
        set_error(error_out, "invalid path");
        return std::ptr::null_mut();
    };
    let list: ChapterList =
        crate::read_chapters(Path::new(&path)).unwrap_or_default();
    let arr: Vec<Value> = list
        .chapters
        .iter()
        .map(|c| {
            json!({
                "index": c.index,
                "start_secs": c.start_secs,
                "end_secs": c.end_secs,
                "title": c.title,
            })
        })
        .collect();
    json_ptr(&Value::Array(arr))
}

/// System volume in percent, or -1 when unavailable.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_system_volume_get() -> i32 {
    crate::system_volume()
        .unwrap_or(None)
        .map(|v| v as i32)
        .unwrap_or(-1)
}

/// Native `.mov` info as JSON (no ffprobe required), or NULL.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_mov_info(
    path: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(path) = c_str(path) else {
        set_error(error_out, "invalid path");
        return std::ptr::null_mut();
    };
    match crate::read_mov_info(Path::new(&path)) {
        Ok(info) => json_ptr(&json!({
            "major_brand": info.major_brand,
            "duration_secs": info.duration_secs,
            "width": info.width,
            "height": info.height,
            "video_fourcc": info.video_fourcc,
            "audio_fourcc": info.audio_fourcc,
            "framerate": info.framerate,
            "container": info.container_name(),
        })),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Native `.mkv` info as JSON (no ffprobe required), or NULL.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_mkv_info(
    path: *const c_char,
    error_out: *mut *mut c_char,
) -> *mut c_char {
    let Some(path) = c_str(path) else {
        set_error(error_out, "invalid path");
        return std::ptr::null_mut();
    };
    match crate::read_mkv_info(Path::new(&path)) {
        Ok(info) => json_ptr(&json!({
            "doctype": info.doctype,
            "duration_secs": info.duration_secs,
            "width": info.width,
            "height": info.height,
            "video_codec": info.video_codec,
            "audio_codec": info.audio_codec,
            "framerate": info.framerate,
            "container": info.container_name(),
        })),
        Err(e) => {
            set_error(error_out, &e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// ProRes profile for a fourcc string: 1 proxy, 2 LT, 3 standard,
/// 4 HQ, 5 4444, 6 4444XQ, 0 unknown.
#[no_mangle]
pub extern "C" fn tontoo_mediakit_prores_profile(fourcc: *const c_char) -> i32 {
    let Some(fourcc) = c_str(fourcc) else {
        return 0;
    };
    match crate::profile_from_fourcc(&fourcc) {
        crate::ProResProfile::Proxy => 1,
        crate::ProResProfile::Lt => 2,
        crate::ProResProfile::Standard => 3,
        crate::ProResProfile::Hq => 4,
        crate::ProResProfile::FourFourFourFour => 5,
        crate::ProResProfile::FourFourFourFourXq => 6,
        crate::ProResProfile::Unknown => 0,
    }
}
