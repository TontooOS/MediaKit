# Ffi

C ABI (`tontoo_mediakit_*`) and JSON shapes for the SDK bridge.

## Transport

| Function | Return | Meaning |
|---|---|---|
| `tontoo_mediakit_open` | `uint64_t` | Player id, 0 on error (`error_out` set) |
| `tontoo_mediakit_play` | `0` / `1` | 0 on success, 1 on error |
| `tontoo_mediakit_pause` | `0` / `1` | 0 on success, 1 on error |
| `tontoo_mediakit_stop` | `0` / `1` | 0 on success, 1 on error |
| `tontoo_mediakit_seek` | `0` / `1` | 0 on success, 1 on error |
| `tontoo_mediakit_set_speed` | `0` / `1` | 0 on success (0.5-2.0), 1 on error |
| `tontoo_mediakit_close` | `0` / `1` | 0 when the id existed |
| `tontoo_mediakit_state` | `int` | 0 stopped, 1 playing, 2 paused, 3 finished, -1 unknown |

```c
char *err = NULL;
uint64_t id = tontoo_mediakit_open("clip.mp4", &err);
tontoo_mediakit_play(id, &err);
tontoo_mediakit_seek(id, 10.0, &err);
tontoo_mediakit_close(id);
```

## Metadata and devices

| Function | Return | Meaning |
|---|---|---|
| `tontoo_mediakit_metadata` | `char *` / `NULL` | JSON metadata, NULL on error |
| `tontoo_mediakit_mov_info` | `char *` / `NULL` | Native MOV JSON, NULL on error |
| `tontoo_mediakit_mkv_info` | `char *` / `NULL` | Native MKV JSON, NULL on error |
| `tontoo_mediakit_avi_info` | `char *` / `NULL` | Native AVI JSON, NULL on error |
| `tontoo_mediakit_prores_profile` | `int` | 1 proxy, 2 LT, 3 standard, 4 HQ, 5 4444, 6 4444XQ, 0 unknown |
| `tontoo_mediakit_list_cameras` | `char *` / `NULL` | JSON camera array, NULL on error |
| `tontoo_mediakit_load_subtitles` | `0` / `1` | 0 when the SRT/VTT loaded |
| `tontoo_mediakit_chapters` | `char *` | JSON chapter array (empty when none) |
| `tontoo_mediakit_system_volume_get` | `int` | Percent, -1 when unavailable |

Metadata JSON:

```json
{ "duration_secs": 12.5, "width": 1920, "height": 1080, "video_codec": "h264", "audio_codec": "aac", "framerate": 30.0, "container": "mov,mp4" }
```

## Memory Rules

| Rule | Detail |
|---|---|
| Strings | Free every returned `char *` with `tontoo_mediakit_string_free` |
| Version | `tontoo_mediakit_version` is static, do not free |
| Errors | `error_out` strings also use `tontoo_mediakit_string_free` |

## Cross References

- [Playback.md](Playback.md) – Rust transport API
- [Metadata.md](Metadata.md) – metadata field meanings
- [ProRes.md](ProRes.md) – MOV and ProRes native API
