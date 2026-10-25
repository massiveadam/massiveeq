---
name: verify-massiveeq
description: Verify MassiveEQ's GTK listening workflow and record its DSP output on an unused real ALSA output in a private D-Bus session.
---

# MassiveEQ verification

## Launch

Use a staged checkout and its matching release binaries. The editor and daemon must come from the same build. On this development machine, build and run the unit checks in the Arch distrobox:

```bash
distrobox enter arch -- bash -lc 'cd ~/Playground/massiveeq && cargo clippy --workspace --all-targets --locked -- -D warnings && cargo test --workspace --release --locked && cargo build --release --workspace --locked'
```

For a real desktop test, copy `Cargo.toml`, `Cargo.lock`, `crates`, `packaging`, `.agents/skills/verify-massiveeq`, and the release binaries to a new staging directory on the chosen host. Compare SHA-256 hashes of the binaries before launching. Do not replace installed binaries or real profile directories for this test.

## Doctor

Read the infrastructure skill before connecting to another host. Verify `hostname`, user, and home. Inspect `wpctl status`, the existing MassiveEQ service's `Status`, and running editors. This runner requires an unused ALSA output and no existing MassiveEQ filter on that output. If an unrelated editor is open, do not drive it. Preserve the output's volume and mute state.

Check `ldd` for missing libraries. Required tools are `dbus-run-session`, `busctl`, `jq`, `pw-dump`, `pw-record`, `pw-link`, `niri`, and Python with PyGObject/AT-SPI. Get `WAYLAND_DISPLAY` and `NIRI_SOCKET` from `systemctl --user show-environment`. Record hashes of installed binaries and `~/.config/massiveeq/state.json`, the original service PID, and original PipeWire output state.

## Drive

Create a fresh test root outside normal profile storage. Keep the real `XDG_RUNTIME_DIR` so the candidate reaches the actual PipeWire and Wayland sockets. Export the verified desktop environment and run:

```bash
MEQ_TESTFLIGHT_ROOT=/absolute/new/test-root \
MEQ_TESTFLIGHT_PRIVATE_BUS=1 \
dbus-run-session -- bash /absolute/staged/repo/.agents/skills/verify-massiveeq/testflight.sh /absolute/staged/repo
```

The runner owns a separate D-Bus service and XDG configuration/data directories. It checks acquired D-Bus names, since an activatable service is not a running instance. It creates a synthetic HD 650 fixture, assigns it to the real ALSA output, opens the actual GTK editor, and drives named controls through AT-SPI. Do not describe the fixture as a real headphone measurement.

## Main user-facing features

- Editor lifecycle and ownership have an isolated GTK scenario: run `SHOT_SCENARIO="$PWD/.agents/skills/verify-massiveeq/editor-lifetime-scenario.sh" /home/adam/Playground/devbox/massiveeq-shot.sh /home/adam/Playground/devbox/out/editor-after.png 3`. It rejects competing D-Bus previews/saves/cleanup, tests explicit release and crash expiry, checks three launches retain one window, edits a real band, verifies stable processing across renewals, saves on immediate close, edits again after reopening, stops a playing sweep on close, and preserves the editor across a failed save until retry succeeds.
- Measurement merging has a dedicated native GTK scenario. From the development checkout run `SHOT_SIZE=1280x1100 SHOT_SCENARIO="$PWD/.agents/skills/verify-massiveeq/measurement-scenario.sh" /home/adam/Playground/devbox/massiveeq-shot.sh /home/adam/Playground/devbox/out/merge-complete.png 3`. It needs the existing devbox harness, its virtual-pointer helper, and `wtype` in the Arch container. It uses the real multi-file chooser, assigns an ambiguous ear, saves separate and merged views, reopens retained takes, tests removal/cancel and invalid-file handling, enables level matching by pointer, and resizes only its own dialog to 380 px. Fixtures and profile state are isolated by the harness.
- Bass and tilt persist as portable shelves. Repeated edits retain readouts and do not duplicate shelves after serialization.
- Undo/redo restore grouped edits. Hear starting sound auditions the captured reference.
- Graph listening creates a tone stream. Sweep advances logarithmically, then holds its frequency when paused. Corrections apply at the held frequency. Stop removes the stream.
- Curve details persist source and fixture metadata through the dialog.
- A fader edit changes recorded 3 kHz DSP output by more than 3 dB. Recording taps the candidate's playback ports before the hardware's volume and mute control.
- Closing immediately after a fader edit saves the newest value and exits.

The development harness at `/home/adam/Playground/devbox/massiveeq-shot.sh` additionally covers held graph drags before saving, lease expiry, failed-save recovery, IR preservation, and narrow layouts. Commands and evidence are in `design-qa.md`.

## Evidence

Keep the test root's `evidence/` directory. It contains app-only screenshots, before/after WAV recordings, a service status snapshot, and the session log. Inspect the screenshot, status counters, and log. A passing process exit alone does not prove audio or layout behavior. Check for GTK criticals, Adwaita layout warnings, panics, engine errors, invalid buffers, and input overflows.

## Cleanup

The runner kills and waits for only its own editor and daemon. After it exits, verify no test tone, candidate DSP nodes, or test recording streams remain. Verify the installed daemon PID, installed binary hashes, real profile state hash, and physical output volume/mute match the doctor snapshot. Preserve evidence and staged sources; remove temporary runtime processes. Report `clean`, `changed`, or `blocked` with coverage and limitations.
