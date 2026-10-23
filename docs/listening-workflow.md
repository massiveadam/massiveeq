# Listening and live editing

MassiveEQ keeps the graph, audio audition, and saved profile separate. Dragging an EQ point or moving a fader previews its current value while the gesture is still in progress. A quiet period of 300 ms saves a completed edit; releasing a graph point starts that save timer. Live requests are coalesced at 35 ms and sent on a background worker, so compiling a profile or writing it does not block GTK input.

## Controls

The response panel holds Listen, Sweep, the full-range sweep duration, a tone-cursor mode, frequency, ear selection, and small corrections at the held frequency. Sweep moves logarithmically upward and stops at 20 kHz. Turning Sweep off holds the frequency while Listen continues. Turning Listen off stops playback. Tone cursor makes graph drags move the test frequency; turning it off returns the graph to normal EQ editing. Moving to Text or IR stops playback. The full Tone page remains available for pink noise and its band controls.

Bass is a broad shelf at 105 Hz. Positive Tilt is brighter and negative Tilt is warmer, using opposing shelves around 1 kHz. These are ordinary parametric filters, visible in the band bank and portable in exported profiles. They preserve IRs and other filters. Repeated changes reuse the same matching shelves rather than adding a new filter on every step.

Undo and Redo restore complete EQ text and manual trim. A drag or a continuous burst of edits is one step. Ctrl+Z and Ctrl+Shift+Z work outside editable text fields, where native text undo takes precedence. History is bounded to 100 steps and resets when opening another profile or loading an IR.

Hear starting sound auditions the snapshot captured when a profile opens. Capture start replaces that snapshot with the current EQ. The reference and edited profile use a common clipping-safe estimated perceived level. This uses the same matching basis as the existing comparison bank. It does not measure SPL at the listener's ears. Changing the audition does not overwrite the edited profile.

## Curve context

The Curves menu has separate measurement and target details. Each stores source, fixture, compensation, named compensation baseline, and headphone type alongside the curve. Old curves remain usable with unknown context. Replacing a curve clears the previous provenance. Unknown fields never imply a match. Declared mismatches are shown, and user-declared matches are not verification of the underlying measurement. The graph calls the corrected overlay Predicted because it is the imported measurement plus the calculated EQ response, not a fresh acoustic measurement.

Target ranges remain optional references. No GraphWorks database, preference-bound data, or smoothing algorithm is bundled. The research link in curve details credits [Griffin Silver's Headphones.com write-up](https://headphones.com/blogs/features/what-headphone-sound-profiles-do-people-like). Its discussion of listener variation and fixture-specific compensation informed this treatment of imported data.

## Ownership and recovery

`editing.rs` owns deterministic history and portable tone-shaping helpers. `live_client.rs` owns the coalescing worker queue, background status polling, tone retunes, completion delivery, and close-time save barrier. GTK owns the current document, graph preview, and controls.

The service's `PreviewProfile(id, request_json)` validates a draft and holds it in memory for five seconds after its last renewal. `EndPreview(id)` restores saved playback immediately. Normal `PutProfile` remains the persistence boundary. Requests cannot introduce convolution files through the preview API. The compiler still validates all asset paths and keeps clipping protection. Reference compilations are cached per rate, quantum capacity, and channel count. The cache is replaced when the reference changes or a new IR loads. Existing comparison banks retain their own candidate selection and common level while incorporating the edited candidate.

The editor renews live edits and tones every two seconds. A vanished editor leaves the saved profile intact; the service expires its live draft and independently stops an unrenewed tone. Closing the editor flushes the latest save before ending the preview. A service timeout or failed save leaves the window open with the error and the unsaved draft.

`latest_processor.rs` transfers ownership of the newest prepared processor through one atomic slot. Publishing replaces any unconsumed processor and destroys it on the control thread. Resuming idle audio takes the newest edit instead of exhausting a queue or replaying old edits. The audio callback crossfades over 8 ms and returns retired processors to the control thread. Allocation checks cover processing, crossfades, surround downmix, and resuming after 32 idle updates.

## Design choice

The alternative was to shorten the existing save delay and call PutProfile during every movement. That would repeatedly write profiles, run compilation on GTK's input thread, and still tie listening latency to persistence. The chosen transient-preview protocol gives the worker only the newest draft while retaining queued saves when profiles change. All processors are built on the control thread, and the existing audio engine crossfades them without allocating on its real-time thread.

The background service and editor must both be updated for the preview API. An older service can still save profiles, but cannot provide the new live-preview behavior.
