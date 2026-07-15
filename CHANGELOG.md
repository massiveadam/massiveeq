# Changelog

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
