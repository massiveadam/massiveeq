# MassiveEQ

MassiveEQ is a system-wide playback equalizer for Arch Linux audio sessions.
It combines a GTK4/libadwaita profile editor with a user-level service that
places PipeWire smart filters in front of assigned output devices.

## Current features

- Squiglink, AutoEQ, and common Equalizer APO text profile import
- Parametric EQ, per-ear filters, GraphicEQ, preamp, includes, and convolution
- Automatic profile assignment using stable Bluetooth and ALSA identifiers
- Multiple simultaneous output devices, bypass, headroom, and level analysis
- Native 32-bit floating-point DSP with allocation-free live processing
- Eight-millisecond click-safe chain changes without removing the audio endpoint
- Rate-aware compilation, high-quality IR resampling, and reported latency
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

The audio host maintains a persistent native PipeWire smart-filter pair for
each assigned output. MassiveEQ's Rust DSP engine uses stable biquad cascades,
minimum-phase GraphicEQ FIR design, and a two-stage partitioned convolver.
Convolution assets are decoded with libsndfile and resampled offline with
libsamplerate's highest-quality sinc converter. The real-time callback performs
no allocation, locking, file access, logging, process control, or D-Bus work.

Profiles use either parametric/GraphicEQ processing or convolution. A profile
that mixes those modes, references a missing IR, exceeds the device Nyquist
rate, or fails headroom analysis remains an editable draft while the last valid
audio chain continues playing.

## Diagnostics

`massiveeqd --self-test-node NODE_NAME` creates a temporary bypass filter for a
specific PipeWire sink and reports buffer and processing statistics. The D-Bus
`Status` method reports active rates, latency, CPU deadline use, buffer health,
and any candidate activation error.
