# MassiveEQ visual refresh and listening QA

Updated 2026-10-22 for the approved btop, Winamp, and watch-tile redesign on `feature/visual-refresh`.

The listening workflow adds live audition during graph drags, a graph-integrated sine sweep, portable bass and tilt shelves, undo/redo, and level-matched comparison with a captured starting sound. Imported measurement and target curves can store their source, fixture, compensation, baseline, and headphone type. The graph labels the calculated correction Predicted. Unknown or conflicting metadata remains visible. See [the workflow and protocol](docs/listening-workflow.md).

## Design reference and implementation

The approved concept is `/tmp/meq/mock/mock.html`. The app now uses its brighter orange `#ff7a1a`, light playing-profile tile, circular power and filter controls, profile/tool rail, framed response panel, and vertical fader bank. A shared editor changes the selected band's type, ears, frequency, gain, Q, enabled state, and order.

At widths below 880 px the live profile/tool rail moves into a scrollable header popover. Below 720 px the response readouts and IR controls stack or wrap. Both editor stacks measure the visible page's height, so hidden text and tone pages do not leave empty space under the bands. Long IR names ellipsize instead of forcing the window wider.

Faders refresh in place during autosave, graph edits, and keyboard nudges. Their common range starts at ±12 dB and expands for larger gains without shrinking during a drag. Loading another profile resets that range. Adding EQ bands preserves an existing impulse response.

## Verification

- Release workspace build passed.
- Workspace Clippy passed with `-D warnings`.
- Workspace release tests passed: 134 passed, one ignored DSP load benchmark. This includes the four preserved idle-discovery regression tests from benji.
- `git diff --check` passed.
- Drove the GTK controls through AT-SPI in isolated headless Sway sessions with private D-Bus, PipeWire, and configuration directories.
- Verified fader gain plus shared frequency/Q edits in the saved profile text, reorder, enable/disable, delete, add, gains above 12 dB, and deletion of the final band.
- Verified measurement/target controls, text and IR pages, and tone stopping when moving to Text or IR.
- Measured a held graph drag through the real GTK pointer path and recorded the PipeWire null sink. The audio engine accepted the edit in 85.5 ms and the 3 kHz output fell 10.7 dB before pointer release, while saved text remained unchanged. Release saved the final three bands; one undo restored the whole gesture. This is a headless-session observation, not a hardware latency guarantee.
- Verified bass and tilt save as standard shelf filters, grouped undo/redo, starting-sound audition, logarithmic sweep movement, pause holding frequency, correction at the held frequency, and stopping the tone stream.
- Repeated broad tone changes retain their controls' displayed values and reuse shelves after profile serialization rounds Q. A regression test reproduces the earlier readout reset and duplicate-shelf failure.
- Edited source and fixture through the curve-details dialog and checked persisted metadata. Core tests cover old files without sidecars, invalid metadata, cleared provenance on replacement, and unknown or conflicting context.
- Verified an abandoned live draft expires to unchanged saved playback, and immediate close saves the last edit.
- Removed profile-directory write access to force a real save failure. Close kept the editor open with its error; restoring access and retrying close saved the draft.
- Verified 32 edits while audio is idle resume with the newest processor, without allocation or deallocation in the audio callback.
- Verified adding and editing filters while an IR is loaded preserves the IR and leaves the engine error map empty.
- Captured 1280 px desktop, 1024 px laptop, and 387 px narrow layouts. Resized a running window through 760, 387, and 1280 px while switching profiles and opening tone from the rail. No GTK criticals, application panics, or Adwaita layout warnings in the final resize run.

## Evidence

All captures are under `/home/adam/Playground/devbox/out/`:

- `listening-wide.png`, `listening-shaped.png`: response controls and broad tone shaping.
- `listening-live-drag.png`, `listening-undo.png`, `listening-start.png`: live editing and comparison.
- `listening-sweep.png`, `listening-metadata.png`: graph sweep and curve context.
- `listening-narrow.png`, `listening-narrow-details.png`: 387 px editing and curve-details dialog.

- `refresh-wide.png`: desktop response, profile rail, faders, and selected-band editor.
- `refresh-edited.png`: saved gain/frequency/Q changes.
- `refresh-expanded-range.png`: an 18 dB band.
- `refresh-empty.png`: final band removed.
- `refresh-tone.png`, `refresh-text.png`, `refresh-loaded-ir.png`: other editor pages.
- `refresh-387x1200.png`, `refresh-tone-387x1200.png`: narrow bands and tone.
- `refresh-rail-387x1200.png`, `refresh-curves-387x1200.png`: narrow profile and curves popovers.
- `refresh-resize-760.png`, `refresh-resize-387.png`, `refresh-resize-1280.png`: breakpoint transitions.

## Repeat the checks on this dev machine

From `/home/adam/Playground/massiveeq`:

```bash
distrobox enter arch -- bash -lc 'cd ~/Playground/massiveeq && cargo build --release --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --release --locked'
```

From `/home/adam/Playground/devbox`:

```bash
SHOT_SIZE=1280x1000 SHOT_SCENARIO=scenarios/visual-refresh.sh ./massiveeq-shot.sh out/refresh.png 3
SHOT_SIZE=1280x1100 SHOT_SCENARIO=scenarios/refresh-resize.sh ./massiveeq-shot.sh out/refresh-resize.png 3
SHOT_SIZE=1280x1000 SHOT_SCENARIO=scenarios/refresh-ir.sh ./massiveeq-shot.sh out/refresh-ir-test.png 3
SHOT_SIZE=1280x1200 SHOT_SCENARIO=scenarios/listening-workflow.sh ./massiveeq-shot.sh out/listening.png 3
SHOT_SIZE=1280x1200 SHOT_SCENARIO=scenarios/live-response.sh ./massiveeq-shot.sh out/listening-live.png 3
SHOT_SIZE=1280x1200 SHOT_SCENARIO=scenarios/live-expiry.sh ./massiveeq-shot.sh out/listening-expiry.png 3
SHOT_SIZE=1280x1200 SHOT_SCENARIO=scenarios/listening-close.sh ./massiveeq-shot.sh out/listening-close.png 3
SHOT_SIZE=1280x1200 SHOT_SCENARIO=scenarios/listening-save-failure.sh ./massiveeq-shot.sh out/listening-save-failure.png 3
```

The headless sessions use null audio outputs. Bluetooth reconnection, an audible headphone listening check, and file-picker interaction remain outside this pass.

## Benji test flight

The matching editor and daemon release binaries passed a test on benji's actual niri desktop and PipeWire/ALSA analog output. The candidate ran on a private D-Bus session with separate configuration and profile directories. SHA-256 hashes matched the locally verified build. Installed binaries and the existing service were left in place.

- Exercised bass/tilt, repeated shelf edits, undo/redo, starting-sound comparison, graph listening, sweep/pause, correction at the held frequency, metadata editing, and immediate-close saving through the real GTK controls.
- Recorded the candidate's playback ports before hardware volume/mute. Changing the 3 kHz fader changed the recorded signal by -12.0 dB. The physical output remained muted throughout.
- The final snapshot had no engine errors, input overflows, output underflows, or invalid buffers. Average DSP work used 0.29% of the audio deadline at 48 kHz and a 2048-frame quantum.
- Inspected the app-only screenshots at benji's fractional display scale. GTK's Vulkan backend logged suboptimal-swapchain and presentation-timing warnings; the UI remained usable, and no GTK critical, Adwaita layout warning, or panic occurred.
- Cleanup verified the original service PID, installed binary hashes, saved-state hash, and physical output properties were unchanged. No candidate audio or tone nodes remained.

The first automation attempts needed waits for asynchronous AT-SPI actions and the compact profile/tools menu. The successful run is `~/.local/state/massiveeq-testflight/20261022-listening/run4/evidence/` on benji. Its status, log, screenshots, and before/after WAV recordings are retained there. The repeatable project workflow is [.agents/skills/verify-massiveeq/SKILL.md](.agents/skills/verify-massiveeq/SKILL.md).
