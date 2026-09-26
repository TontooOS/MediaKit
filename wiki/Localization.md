# Localization

Error message localization via `lang/en_us.json` and `lang/de_de.json`.
Locale is detected from `LC_ALL` / `LC_MESSAGES` / `LANG` (German prefix
selects `de_de`, everything else falls back to `en_us`).

## Keys

| Key | en_us | de_de |
|---|---|---|
| `not_available` | Video hardware or tool not available | Videogerät oder Werkzeug nicht verfügbar |
| `unsupported_format` | Unsupported video format: {} | Nicht unterstütztes Videoformat: {} |
| `ffmpeg_missing` | ffmpeg/ffprobe not found on PATH | ffmpeg/ffprobe nicht im PATH gefunden |

Control labels (`controls.play`, `controls.pause`, ...) live in app
`lang/` folders per the TontooOS contract (`lang/en_us.json` and
`lang/de_de.json`); MediaKit ships `label_key` descriptors only. Text
renders in SF Pro via CoreText.

## Usage / Example

```rust
let msg = mediakit::lang::t("ffmpeg_missing");
```

## Cross References

- [Controls.md](Controls.md) – label keys and SF Pro usage
