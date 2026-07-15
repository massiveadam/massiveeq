# MassiveEQ

MassiveEQ is a system-wide playback equalizer for Arch Linux audio sessions.
It combines a GTK4/libadwaita profile editor with a user-level service that
places PipeWire smart filters in front of assigned output devices.

## Current features

- Squiglink, AutoEQ, and common Equalizer APO text profile import
- Parametric EQ, per-ear filters, GraphicEQ, preamp, includes, and convolution
- Automatic profile assignment using stable Bluetooth and ALSA identifiers
- Multiple simultaneous output devices, bypass, headroom, and level analysis
- Wayland-native GTK4 interface and systemd user service

## Build

```sh
cargo build --workspace
cargo test --workspace
```

Run `massiveeqd` first, then `massiveeq`. For a persistent session install the
files under `packaging/` or build the included `PKGBUILD`.

After a package install, open MassiveEQ once or enable the service directly:

```sh
systemctl --user enable --now massiveeq.service
```

Profiles live in `~/.local/share/massiveeq/profiles/`; assignments and manual
trims live in `~/.config/massiveeq/state.json`. Unassigned and unsupported
multichannel devices are never intercepted.

## Text compatibility

MassiveEQ accepts `Preamp`, the common `Filter` variants (`PK`, `LS`, `HS`,
`LP`, `HP`, `BP`, `NO`, and `AP`), `GraphicEQ`, `Channel`, `Include`, and
`Convolution`. Unsupported commands are reported with their source line and
will not be activated silently. Included files are flattened on import and IR
assets are copied into the profile library.

The audio host uses PipeWire's built-in biquads and partitioned convolver.
GraphicEQ curves are compiled to a linear-phase impulse response, and
libsndfile is used to preserve mono/stereo IR channel mapping across WAV,
FLAC, AIFF, and OGG assets.
