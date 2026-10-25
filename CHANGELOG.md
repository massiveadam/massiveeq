# Changelog

## Unreleased

### Added

- Volume in MassiveEQ (footer → VOLUME, per output): the volume keys set
  MassiveEQ's volume on an even decibel scale, 0.5 dB per 1%, while the
  device keeps its own volume as the ceiling. The level survives restarts, and
  the device is turned down in its place whenever MassiveEQ's sink goes away,
  so stopping, upgrading or turning the option off is never louder.
- Block volume jumps (same menu): undo any raise of more than 10% at once,
  such as an app or Bluetooth headphones pushing the volume to 100%. Volume
  keys still work, and the menu shows the last jump it blocked.
- Import several left/right measurement files in Curves → Measurements,
  average takes per ear, or merge both ears with equal weight. Preview and
  revise ear assignments, remove takes, and optionally match levels before
  applying. Original takes remain stored for later changes.
- Show where the corrected measurement leaves a target range (preference
  bounds): each stretch outside is shaded and listed with how far out it goes.
- Add placements: more measurements of the same headphone after putting it on
  again, drawn as a median with 50% and 90% bands; the median is corrected.
- Choose the measurement fixture from a list (B&K 5128, GRAS KB50xx, B&K 4128,
  IEC 711) and recognise fixtures however they were typed; a measurement
  from another fixture than its target or bounds is named in the note.
- Drag the line above the footer to make it taller and see more rows of
  band chips; drag it back down to return the room to the graph.

### Changed

- Filters read the way Equalizer APO reads them, so a preset sounds the same.
  `BW Oct` is a digital bandwidth, designed at each sample rate (it was turned
  into one Q, which made bands above about 5 kHz up to 1.6 dB narrower).
  `LS`/`HS` with a Q or a slope (`LS 6dB`, `HS 12dB`) take Fc as the corner
  rather than the centre, a shelf with neither has APO's slope of 0.9, a
  slope after the type (`LSC 12dB`) sets it, and shelves ignore `BW`. A notch
  without a Q has APO's Q of 30 (it was 0.71, a 32 dB difference at Fc).
  Room EQ Wizard's `Modal` filters load as peaking filters, and `Fc 1.500 Hz`
  is 1500 Hz, as REW writes it. Measured through the service, a nine-band
  preset now matches APO within 0.0002 dB at 44.1, 48 and 96 kHz.
- The graph, headroom and level match hold a GraphicEQ's end gains outside
  its points, as the audio and Equalizer APO do; they used 0 dB there.
- Profile text keeps every value to six decimals instead of rounding gains
  and frequencies to 0.01 and Q to 0.0001, so saving never changes the sound.
- Tone generator: pink noise no longer clips. It used to hit the −12 dBFS
  limit on about one sample in six at full level; noise now stops at
  −22 dBFS, where its peaks reach the same limit cleanly, and the level
  slider follows when Pink noise is picked.
- Tone generator: the Octave and ⅓ octave noise bands are now that wide.
  They were about 0.65 and 0.21 octaves.
- Tone generator: the service runs the sweep itself, so it glides at an even
  rate instead of stepping every 25 ms and stalling while the editor is
  busy. Moving the frequency by hand also glides instead of jumping.
- Tone generator: while the level is held for the tone, a −1 dBFS safety
  limiter guards the output, so music or a notification playing alongside
  cannot clip. The tone itself always stays below it and is never touched.
- Tone generator: the tone has its own connection in the editor and its own
  lock in the service, so moving it never waits behind a profile preview
  and starting it never holds up other requests.
- The selected band's panel moves to whichever side of its node hides the
  least of the curves near that band, using the empty top or bottom of the
  graph when the space beside the node is busy, so it no longer covers the
  region being tuned. It holds its place while the node is dragged.
- Tilt is now a real tilt: a constant slope in dB per octave (±1.5) that
  pivots at 1 kHz and turns the whole 20 Hz–20 kHz response, instead of a
  step between two shelves at 1 kHz. It is one setting on the profile, not
  extra bands, and saves as five ordinary shelf filters so exported profiles
  play the same in Equalizer APO. Profiles with the old 1 kHz shelf pair keep
  it as two normal bands.
- Align curves at 630 Hz ± one octave, as GraphWorks does, instead of across
  300 Hz–3 kHz.

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
- Outputs whose channels are listed as `FL,FR`, such as the Easy Effects
  Sink, are read as stereo. MassiveEQ counted one channel and put a mono
  filter in front of them, so a profile could not sit before EasyEffects.
- `Alt+1`…`Alt+9` pick comparison candidates on any keyboard layout,
  including AZERTY, whose digit row types symbols, and while a slider or
  field has focus.
- Music plays at its own sample rate again when PipeWire is allowed to
  switch (`default.clock.allowed-rates`). MassiveEQ pinned the output to the
  default rate, so 44.1 kHz music was resampled to 48 kHz, and 96 kHz music,
  which PipeWire did switch to, came out silent. MassiveEQ now follows the
  output's rate and rebuilds the chain for it. Parametric EQ switches with
  no audible gap; with an impulse response, about the first 100 ms of the
  track stays silent while it is resampled. The header and status show the
  rate the output really runs at, and the test tone joins at that rate.

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

- Closed editors stop their preview timers and workers. Reopening the app
  reuses its window, and competing editor connections cannot overwrite an
  active preview, save over its profile, or end its session.
- Offer MassiveEQ updates in cachy-update, arch-update, and AUR helpers
  without devel checks: the AUR package now carries each new upstream
  version. It had been published from a shallow clone as `r1`, below every
  installed build, so no update tool ever saw one.
- After an upgrade, the audio service and tray restart on the new version by
  themselves instead of running the old binaries until the next login, and
  the editor's header button becomes RESTART.
- The UPDATE button's **Update now** runs the installed AUR helper in a
  terminal; the editor also re-checks for updates every six hours.
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
