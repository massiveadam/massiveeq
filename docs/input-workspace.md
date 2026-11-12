# Input workspace architecture

The GTK app has separate Input and Output pages. Playback retains its existing
model, daemon, storage, and live-edit protocol. Input uses an independent
profile library and a separate Carla/PipeWire service.

## Designs considered

A direction-aware playback daemon could host input and output profiles through
the existing API. The app would select a direction and reuse the entire editor.
That would require changing playback-oriented discovery, profile compilation,
assignment, previews, and real-time ownership. Plugin failures would also share
the playback daemon's process unless a new sandbox were added.

The implemented design keeps those boundaries. The Input page reuses the
response graph, knobs, and undo history utility, but owns its widget state and
profile selection. `massiveeq-core::input` defines its persistent model and
validates saves. The microphone companion validates the same external format,
creates effects through Carla's native API, and owns input routing. This keeps
plugin failures outside playback and avoids migrating the playback library.

The separate page requires some editor wiring of its own. It also uses a
file-based control boundary rather than the playback D-Bus API. Those are the
chosen tradeoffs for keeping the existing playback path intact.

## State and control flow

`input.json` version 1 contains an active profile ID and input profiles. Each
profile owns an exact physical source name, mono or stereo channel map, EQ
bands, voice controls, and optional custom rack. A custom rack is exclusive
with built-in processing. The previous unversioned custom-rack format remains
readable and is migrated only on an explicit edit.

GTK debounces input edits for 300 ms, validates the complete library, writes a
temporary file beside the target, and renames it atomically. Closing flushes
pending edits and refuses to discard a failed save. Input timers are released
when the window is destroyed. Service operations and source/status queries run
outside the GTK thread.

The independently enabled `massiveeq-input.service` reads the selected profile
and checks for changes every 250 ms. Normal EQ and voice parameter edits update
the existing native effects. RNNoise topology changes rebuild the rack while
keeping the published source. A channel-map change rebuilds the virtual source.
The managed order is RNNoise, gate, compressor, EQ. Input gain is not adjusted
by playback's perceptual matching or safety-gain analysis.

The helper reports state, error, profile, source, backend mode, PID, and update
time in an atomic runtime status file. The app reports Input on only when that
status belongs to the systemd service's current PID and says the rack is ready.
A missing microphone, plugin failure, or incompatible sample rate produces an
explicit error and cleanup. Systemd owns retries and child-process cleanup.
The helper does not select a system default or create speaker connections.

## Verification boundaries

Core tests check profile storage, validation, migration, and failed-save
preservation. Python tests check external schema validation, effect control
mapping, routing, source discovery, and runtime status. Native tests measure
actual Carla processing and exercise live updates and teardown on synthetic
sources. The GTK scenario checks source selection, exact saved EQ values,
voice controls, profile lifecycle, undo/redo, close-time save, and playback
profile isolation. Device-specific microphone tuning remains a manual check.
