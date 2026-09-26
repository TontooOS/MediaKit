# Controls

Player control descriptors: SF Symbols via CoreIcon, SF Pro text via
CoreText and the TontooOS theme colors. Core-only data so every TontooUI
app renders identical transport bars.

## Icons (CoreIcon)

```rust
let path = mediakit::ControlIcon::Play.icon_path();
```

- `ControlIcon::sf_symbol` returns the SF Symbol name (`play.fill`,
  `pause.fill`, `stop.fill`, `speaker.wave.3.fill`, ...).
- `icon_path` resolves the staged PNG via
  `coreicon::resolve_icon_path`.

## Text (CoreText)

```rust
assert_eq!(mediakit::controls::sf_pro_family(), "SF Pro");
```

- Labels use `coretext::SF_PRO_FAMILY` (`SF Pro`).
- System paths are `/usr/share/fonts/OTF/SF-Pro-Display-Regular.otf`
  etc. (shipped from `BaseOS/fonts/SF-Pro/`).

## Theme colors

| Mode | Background | Text |
|---|---|---|
| Dark | `#1b2022` (27, 32, 34) | `#d8d9d9` (216, 217, 217) |
| Light | `#ffffff` (255, 255, 255) | `#272727` (39, 39, 39) |

Only background + text are themed (no secondary colors).

## `transport_bar`

```rust
for c in mediakit::transport_bar() {
    println!("{} {}", c.icon.sf_symbol(), c.label_key);
}
```

Returns play/pause/stop/seek/volume/captions/chapters/fullscreen
descriptors with SF Pro sizes.

## Cross References

- [Playback.md](Playback.md) – state behind the buttons
- [Localization.md](Localization.md) – `label_key` lookup
