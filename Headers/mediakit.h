#ifndef TONTOO_MEDIAKIT_H
#define TONTOO_MEDIAKIT_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

const char *tontoo_mediakit_version(void);
void tontoo_mediakit_string_free(char *ptr);

uint64_t tontoo_mediakit_open(const char *path, char **error_out);
int32_t tontoo_mediakit_play(uint64_t id, char **error_out);
int32_t tontoo_mediakit_pause(uint64_t id, char **error_out);
int32_t tontoo_mediakit_stop(uint64_t id, char **error_out);
int32_t tontoo_mediakit_seek(uint64_t id, double position_secs, char **error_out);
int32_t tontoo_mediakit_set_speed(uint64_t id, float speed, char **error_out);
int32_t tontoo_mediakit_close(uint64_t id);
int32_t tontoo_mediakit_state(uint64_t id);

char *tontoo_mediakit_metadata(const char *path, char **error_out);
char *tontoo_mediakit_mov_info(const char *path, char **error_out);
int32_t tontoo_mediakit_prores_profile(const char *fourcc);
char *tontoo_mediakit_list_cameras(char **error_out);
int32_t tontoo_mediakit_load_subtitles(uint64_t id, const char *subtitle_path, char **error_out);
char *tontoo_mediakit_chapters(const char *path, char **error_out);
int32_t tontoo_mediakit_system_volume_get(void);

#ifdef __cplusplus
}
#endif

#endif
