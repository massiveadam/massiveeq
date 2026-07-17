# Changelog

## Unreleased

### Added

- Add `massiveeqctl` for versioned JSON status, live D-Bus change streaming,
  and fast engine, profile, comparison, and per-output Filters controls.
- Add an optional native Noctalia 4 bar widget with an anchored quick-controls
  panel, without changing the generic StatusNotifier tray behavior.
- Show the currently routed output in the Noctalia panel and provide compact,
  live frequency, gain, and Q controls for its active parametric profile.

## 0.3.0-beta.2 — 2026-07-15

Second public beta release focused on precise, responsive filter editing.

### Highlights

- Click empty graph space to create a parametric band at that frequency and gain.
- Select and adjust graph bands with fine and coarse keyboard controls for
  frequency, gain, and Q.
- Keep output and individual-band switches visually responsive while audio
  changes are committed safely in the background.
- Focus the graph from filter cards and expose complete editing instructions to
  assistive technology.
- Protect convolution profiles and per-channel filter limits during direct graph
  editing.

### Validated environment

- Arch Linux x86_64 and niri Wayland.
- PipeWire 1.6.8 and WirePlumber 0.5.15.
- Bluetooth A2DP and built-in analog stereo playback.

### Beta limitations

- Playback only; capture and surround outputs are bypassed.
- GNOME, KDE Plasma, professional multichannel interfaces, and ARM systems have
  not yet received manual hardware validation.
- Profile storage and the D-Bus interface may change before the stable release.

## 0.3.0-beta.1 — 2026-07-15

First public beta release.

### Highlights

- Native allocation-free PipeWire DSP for mono and stereo playback outputs.
- Parametric, per-ear, GraphicEQ, Equalizer APO text, and convolution profiles.
- Stable Bluetooth and ALSA device assignments with reconnect restoration.
- Two-to-nine-way profile comparisons using BS.1770 K-weighted level matching.
- Persistent audio endpoints with click-suppressed live filter changes.
- Synchronized visual and text editors with invalid-draft protection.
- Optional StatusNotifier controls for compatible desktop panels.

### Validated environment

- Arch Linux x86_64 and niri Wayland.
- PipeWire 1.6.8 and WirePlumber 0.5.15.
- Bluetooth A2DP and built-in analog stereo playback.

### Beta limitations

- Playback only; capture and surround outputs are bypassed.
- GNOME, KDE Plasma, professional multichannel interfaces, and ARM systems have
  not yet received manual hardware validation.
- Profile storage and the D-Bus interface may change before the stable release.
