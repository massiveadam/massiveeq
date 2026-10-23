# Idle CPU regression

This fix was originally maintained locally on benji. It is now included in the listening-workflow branch so updating the editor and service preserves it. The measurements below describe the original fix's verification; the current workspace checks are recorded in [design-qa.md](../design-qa.md).

## Cause and fix

Device discovery runs short-lived `pw-dump` and `pw-metadata` clients. The registry monitor used to treat every global-object connection and removal as a device change. Its own discovery clients therefore scheduled more discovery. On benji, the daemon created 38 observed `pw-dump` children within five idle seconds and used about 28% of one CPU core.

The monitor now requests discovery only for Device, Node, and Metadata globals. It tracks those IDs so unrelated Client and Port removals do not request discovery. Node-info and metadata-property listeners remain in place. The 30-second filter-health check remains unchanged.

Four regression tests in `crates/massiveeqd/src/device_monitor.rs` cover observer clients, ports, meaningful globals, and unknown or already removed IDs. Three tests failed against the original event policy and passed after the fix.

## Verification on benji

- All 69 workspace tests passed with `cargo test --workspace --locked`.
- An optimized daemon built with `cargo build --release -p massiveeqd --locked`.
- A 10-second idle measurement after installation showed 0.30% of one CPU core and no discovery child processes during that window.
- A five-second silent PCM playback through the active EQ endpoint reached the physical output. Both stereo links were active.
- During that playback check, the daemon used 3.87% of one CPU core. Native DSP reported no input overflows, output underflows, invalid buffers, or filter errors.
- All six profiles and the connected output's profile assignment were preserved. The daemon and tray services were active.

The playback test verifies processing and routing, not subjective sound quality. Physical hotplug, Bluetooth reconnect, and listening tests were not performed.

## Installation and rollback

The patched release daemon is installed at `~/.local/bin/massiveeqd`, which the existing user service already uses. The custom GUI, tray, control tool, presets, and package-owned `/usr/bin` copies were not replaced.

The original daemon and full settings backup are retained under:

`~/.local/state/system-audits/20261009-updates/backups/`

Raw verification evidence is in the parent audit directory.
