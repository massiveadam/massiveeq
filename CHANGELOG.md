# Changelog

## Unreleased

### Changed

- Rename LISTEN to TONE GEN. While the tone plays, the output level holds
  where it was when the tone started, for the edit and the A/B starting
  sound alike, so a band change sounds like exactly that change instead of
  also re-matching the whole output's loudness. Clipping protection still
  applies to the tone, and matching resumes when it stops.
- Delete a band by double-clicking its node, right-clicking its node or
  chip, or pressing Delete or Backspace; Ctrl+Z restores it.
- Band chips wrap onto a second row, then scroll, instead of running off the
  footer; ＋ follows the last chip.
- Band numbers on the graph and chips take their ear's colour.
- Arrow keys on the selected band: Up/Down change gain by 0.1 dB, Shift+Up/Down
  by 1 dB, Left/Right move frequency, and Shift+Left/Right change Q. Held keys
  speed up. Clicking a band chip or a panel knob keeps these keys on the band.
- Double-click empty graph space to add a band; a single click there now
  closes the band panel instead of adding a stray band.
- Dim the graph while an output's Filters or the engine is off.
- Draw impulse responses on the graph smoothed to a sixth of an octave, so a
  room or binaural IR no longer buries the EQ curve. Headroom and level
  matching still use the exact response.
- Open the preset menu on the profile list rather than the selected name,
  so a stray keypress cannot rename the profile.
- Show the curve-context note only when measurement and target contexts
  conflict.

### Added

- Add `massiveeqctl` for versioned JSON status, live D-Bus change streaming,
  and fast engine, profile, comparison, and per-output Filters controls.
- Add an optional native Noctalia 4 bar widget with an anchored quick-controls
  panel, without changing the generic StatusNotifier tray behavior.
- Show the currently routed output in the Noctalia panel and provide compact,
  live frequency, gain, and Q controls for its active parametric profile.
- Add matching Noctalia 4 and 5 widget states, single-output selection with a
  connected-output picker, complete installation and troubleshooting
  documentation, and a feature screenshot gallery.
- Make the `yay -S massiveeq-git` AUR path prominent and document an equivalent
  no-helper `makepkg -si` installation and update workflow.
- Open the editor on the system's current output with its playing profile in
  focus, and follow newly connected outputs (or a new system default) to
  their assigned profile.
- Mark what is actually playing: a LIVE badge beside the profile name and on
  profile bank rows in use, a SYSTEM OUTPUT badge for the default output, and
  "Assigned · … (not connected)" for profiles waiting on an absent output.
  Outputs with filters off, the engine off, or a comparison on its bypass
  slot show as PAUSED instead of LIVE.
- Wait to follow a newly connected output until the editor has been idle for
  a few seconds and no menu is open; a Bluetooth headset switching between
  call and music routes no longer counts as a new output.
- Sort the profile bank by name everywhere, and keep it sorted (and titled)
  when the open profile is renamed.
- Name the profile playing on the system output in the tray title, list the
  system output first, and mark each output's playing profile in the menu.
- Optionally support true-stereo (4-channel) and 7.1 virtual-surround
  (14-channel HeSuVi) impulse responses. Stereo profiles are unchanged. A 7.1
  profile exposes a selectable "MassiveEQ 7.1" sink
  that takes over as the system output for its device; Filters off keeps the
  sink and plays a standard downmix. The convolution page shows the detected
  layout.
- Apply parametric filters and GraphicEQ on top of an impulse response in the
  same profile, per ear.
- Add a tone finder: a sine or band of pink noise through the current EQ,
  with one-click cut and boost at the tone's frequency.
- Show a headphone measurement and a target or target range behind the EQ,
  with the measurement as corrected by the current filters.
- Show whether playing audio is resampled, with the reason and the PipeWire
  setting that avoids it.

- Edit bands on the graph, as in plugin EQs: the selected band opens a
  floating panel beside its node with low shelf, peak, and high shelf (as on
  Squiglink) drawn as symbols, ears, and frequency, gain, and Q knobs; scroll over a node for Q, Escape to close.
  Band chips in the footer replace the fader bank. The header becomes a
  preset bar (profile bank, undo/redo, A/B, output) and the footer holds
  Filters, Listen, the bands, Bass/Tilt/Output knobs, and Text, IR, Curves.

### Fixed

- Dragging the Bass, Tilt, or Output knob no longer moves the window: the
  footer is a window handle, and knob drags now claim the pointer. Knob value
  labels keep a fixed width, so the footer does not shift as values change.
- Dragging a band knob quickly no longer glitches: the panel moved with its
  node under the pointer and fed that movement back into the knob. The panel
  now holds still while the pointer is on it or a knob is dragged, and knob
  travel is measured in window coordinates.
- Keep band numbers stable when a band is set to one ear: profile text now
  keeps bands in order and switches `Channel:` as needed, so the graph, band
  chips, and text no longer disagree and the band panel opens on the right
  node.
- Name the filter-type and ear buttons for screen readers.
- Apply the narrow (under 720 px) layout. It never took effect because the
  wider route breakpoint, added later, always won; narrow windows were
  clipped on the right instead of reflowing.
- Mark the Convolution tab, not Parametric, when the editor opens on a
  convolution profile.
- Play 4-channel impulse responses as true stereo. They previously played only
  their L→L and L→R channels, as the left and right ears.

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
