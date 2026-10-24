# Combining left and right measurements

Open **Curves → Measurements…**. Add several TXT or CSV frequency-response exports in one selection. Filename tokens such as `left`, `right`, `_L`, or `_R` suggest an ear; check each row before applying. Ambiguous names require an explicit choice. Choose **Both / mono** for a single shared measurement.

The preview shows the average for each ear. **Separate left / right** keeps those averages distinct in the main graph, where each ear's EQ is applied to its own measurement. **Merge both ears** uses their shared average instead. Original takes remain stored in either mode. Reopen Measurements to change ear assignments, remove takes, or choose another output mode. Cancel discards the dialog's edits. **Use measurements** saves the set for the current profile; it does not change EQ bands or the target.

The average is arithmetic in dB. Each take has equal weight within its ear; merging gives the two resulting ear averages equal weight even when their take counts differ. Curves are interpolated in log frequency over their common overlap, which must cover at least a decade. The averaging operation never fabricates points beyond the overlap.

**Match take levels at 630 Hz** is off by default, preserving measured level differences. When enabled, each ear's takes align to that ear's first take, using the mean level over 315–1260 Hz. Every take must cover that band. The graph uses a shared display alignment for both ear averages so a real level difference between ears remains visible. This combines frequency-response magnitudes, not audio recordings or impulse-response phase.

Use takes from the same headphone, fixture, and compensation context. Existing shared measurements and placements can be opened in the manager as Both / mono takes. The previous placement spread stays unchanged until the new average is explicitly applied. Changing the source takes clears measurement provenance that belonged to the old data. Changing output mode alone preserves it. Target data and target provenance remain independent.

## Storage and ownership

`measurement.rs` in the core crate owns validation and averaging. The editor's `measurements.rs` owns the unsaved import workspace, background file reading, preview, and apply operation. The service validates `SetMeasurements(profile_id, set_json)` before persisting it. `curves/measurements.json` holds original named points, ear assignments, output options, and measurement metadata in one atomic document. Derived L/R and ear-balanced averages are returned through GetCurves.

Older `measurement.txt`, placement files, targets, and metadata remain readable. A saved set takes precedence over legacy measurement files. The existing clear/replace measurement operations remove that set too. Per-ear sets use the manager for additional takes instead of the legacy Add placement action. The engine never compiles these display measurements into DSP filters.

An alternative design would save only the merged text curve. It was rejected because it loses ear ownership and prevents changing the merge without importing every original again. The retained-set design keeps cancellation and validation outside persistence and commits one complete document. Imports are bounded to 32 takes, 100,000 points in total, and 4 MB per selected file.
