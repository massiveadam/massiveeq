# MassiveEQ

MassiveEQ is a system-wide playback equalizer for Arch Linux audio sessions.
It combines a GTK4/libadwaita profile editor with a user-level service that
places PipeWire smart filters in front of assigned output devices.

> [!WARNING]
> **Beta 2 software:** MassiveEQ is ready for testing, but its audio routing and
> profile format may still change. Keep an easy way to select the original
> hardware output while testing it.

## Tested on

The current beta has been validated on:

- Arch Linux x86_64 with the niri Wayland compositor
- PipeWire 1.6.8, WirePlumber 0.5.15, GTK 4.22.4, and libadwaita 1.9.2
- Apple AirPods over Bluetooth A2DP and a built-in analog stereo output
- Noctalia/Quickshell StatusNotifier tray registration and live menu state
- PipeWire hotplug/reconnect, simultaneous outputs, service restart/fail-open,
  live filter edits, bypass, and device assignment restoration
- Automated DSP reference tests at 44.1, 48, 96, and 192 kHz

It has **not yet been manually validated** on GNOME, KDE Plasma, surround
outputs, professional multichannel interfaces, or ARM systems. Version
0.3.0-beta.2 should therefore be treated as a public beta, not a
production-stable release. Fully updated Arch Linux x86_64 is the supported
packaging target; partial upgrades and Arch Linux ARM are outside this beta's
support contract.

## Current features

- Squiglink, AutoEQ, and common Equalizer APO text profile import
- Parametric EQ, per-ear filters, GraphicEQ, preamp, includes, and convolution
- Automatic profile assignment using stable Bluetooth and ALSA identifiers
- Unlimited saved profiles with per-output assignment or an explicit unassigned state
- Per-output comparison banks for 2–9 profiles plus level-matched dry playback
- Multiple simultaneous output devices, bypass, headroom, and level analysis
- Native 32-bit floating-point DSP with allocation-free live processing
- Eight-millisecond click-safe chain changes without removing the audio endpoint
- Rate-aware compilation, high-quality IR resampling, and reported latency
- Wayland-native GTK4 interface and systemd user service
- Synchronized visual and Equalizer APO text editors with live validation
- Direct graph editing with click-to-add, dragging, and precise keyboard controls
- Responsive per-output and per-band filter switches for fast A/B listening
- Optional StatusNotifier tray controls for Noctalia, Waybar, KDE Plasma, and other compatible bars
- Optional native Noctalia 4 and 5 quick-controls panels anchored to the bar

## Build

```sh
cargo build --workspace
cargo test --workspace
```

Run `massiveeqd` first, then `massiveeq`. For a persistent session install the
files under `packaging/` or build the included `PKGBUILD`.

The rolling `massiveeq-git` AUR package follows new upstream commits whenever
it is rebuilt. Maintainer automation for safely synchronizing packaging changes
is documented in [`packaging/aur/README.md`](packaging/aur/README.md).

The full editor checks once shortly after launch for a newer upstream revision.
When one is available, a compact **Update** button appears in the header and
offers to copy `yay -S massiveeq-git` or open the AUR package page. The app
never runs an AUR helper or requests administrator privileges by itself.

After a package install, open MassiveEQ once or enable the service directly:

```sh
systemctl --user enable --now massiveeq.service massiveeq-tray.service
```

The tray companion shows the active profile for each output, switches or
unassigns profiles, controls the per-output Filters switch and master engine,
and opens the full editor. It is a separate process, so stopping it never
changes audio routing.

## Noctalia quick controls

Noctalia 4.7.x and Noctalia 5 can use an optional native MassiveEQ widget
instead of the generic tray icon. It opens a themed, bar-anchored panel with the
master Engine switch, the currently routed output, profile assignment,
per-output Filters, active comparison candidates, and compact frequency, gain,
and Q controls for the active parametric profile. Advanced profile editing
remains in the full application.

The two Noctalia generations have different plugin systems. After installing
the Arch package, use the matching adapter.

For Noctalia 5:

```sh
mkdir -p ~/.local/share/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v5/massiveeq/. ~/.local/share/noctalia/plugins/massiveeq/
noctalia msg plugins enable massiveeq/massiveeq
```

Add **MassiveEQ** to the bar in Noctalia settings. The v5 panel uses native
host placement, scaling, click-away dismissal, Escape handling, and monitor/bar
orientation behavior. It displays one connected playback output at a time; if
several are connected, the selector defaults to WirePlumber's current playback
sink and keeps the others available without rendering duplicate cards.

For Noctalia 4.7.x:

```sh
mkdir -p ~/.config/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v4/massiveeq/. ~/.config/noctalia/plugins/massiveeq/
```

Restart Noctalia, enable **MassiveEQ** under **Settings → Plugins**, and add its
widget to the bar. Once it is working, avoid duplicate icons by disabling only
the generic tray companion:

```sh
systemctl --user disable --now massiveeq-tray.service
```

The audio engine remains in `massiveeq.service` and is not stopped by this
command. Source-tree installation and restoration instructions are in the
[`Noctalia 5 adapter guide`](packaging/noctalia-v5/massiveeq/README.md) and
[`Noctalia 4 adapter guide`](packaging/noctalia-v4/massiveeq/README.md).

The desktop-neutral `massiveeqctl` helper used by the widget is also available
for scripts and other bars. Run `massiveeqctl status` for a versioned JSON
snapshot or `massiveeqctl status --watch` for line-delimited live updates.

Profiles live in `~/.local/share/massiveeq/profiles/`; assignments and manual
trims live in `~/.config/massiveeq/state.json`. Unassigned and unsupported
multichannel devices are never intercepted.

## Comparing profiles

Open **Compare** in the Signal Route strip, choose between two and nine
candidates, and select **Start Comparison**. The panel then becomes a set of
listening buttons; selecting one immediately switches to that candidate. `Off`
can be included as a dry candidate. Banks are remembered independently for
each output and do not replace its normal assigned profile.

Level matching predicts the expected [ITU-R BS.1770-5](https://www.itu.int/rec/R-REC-BS.1770-5-202311-I/en)
K-weighted mean-square energy of stationary pink noise after the complete
left/right transfer response. Every candidate is then attenuation-only matched
to the quietest clipping-safe result in the bank. This gives a deterministic,
content-independent comparison without gain pumping; the manual trim remains
available for listener-specific correction. Exact perceived equality cannot be
guaranteed without the listener's playback SPL and headphone transfer, which
more detailed stationary-loudness models such as
[ISO 532-2](https://www.iso.org/standard/63078.html) require.

While the app is focused, `Alt+1` through `Alt+9` select candidates in bank
order. `Ctrl+B` toggles the selected output's **Filters** switch. Turning
Filters off removes the filters while retaining the active profile's
clipping-safe perceived level and existing PipeWire endpoint. The header **Engine**
switch is deliberately different: Engine Off is true 0 dB dry audio with no
EQ, perceptual correction, safety attenuation, or user trim. Comparison
candidates switch through the live 8 ms crossfade without replacing the
PipeWire endpoint. Processed profiles with incompatible convolution latency
cannot be placed in the same bank.

The Parametric page can switch between **Visual** filter cards and the canonical
Equalizer APO-style **Text** profile. Both edit the same draft and autosave
after 300 ms. Invalid text remains visible for correction while the last valid
audio chain continues playing. `Ctrl+E` toggles the two editor views.

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
