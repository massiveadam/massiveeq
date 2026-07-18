# Noctalia quick-controls widget

MassiveEQ ships two native Noctalia adapters. They present the same controls,
but each uses the plugin system native to its Noctalia generation. The widget
is optional: the audio engine, GTK editor, profiles, and generic
StatusNotifier tray continue to work without it.

## Supported versions

| Noctalia generation | Tested/targeted host | Adapter | Plugin API |
|---|---|---|---|
| Noctalia 4 | 4.7.0–4.7.7 | QML | Noctalia 4 manifest API |
| Noctalia 5 | Early and current v5 builds | Luau | Declares API 3; current v5 accepts API 3–4 |

Noctalia 4.7.7 is the final v4 release. The v4 adapter remains supported but
is kept separate from v5 so installing it cannot alter v5 configuration or
theme state. Noctalia 5's plugin system is still beta; the adapter deliberately
uses the oldest currently supported API because it does not need API 4-only
features.

Upstream references:

- [Noctalia 4 plugin overview](https://docs.noctalia.dev/v4/development/plugins/overview/)
- [Noctalia 4 bar widgets](https://docs.noctalia.dev/v4/development/plugins/bar-widget/)
- [Noctalia 4 panels](https://docs.noctalia.dev/v4/development/plugins/panel/)
- [Noctalia 5 plugins](https://docs.noctalia.dev/v5/plugins/)
- [Noctalia 5 plugin development](https://docs.noctalia.dev/v5/plugins/development/)

## Requirements

- A running `massiveeq.service`
- `massiveeqctl` in `PATH`
- PipeWire and WirePlumber
- `wpctl` for the v5 default-output lookup
- Noctalia running on a supported Wayland compositor

Check the two MassiveEQ requirements with:

```sh
systemctl --user is-active massiveeq.service
massiveeqctl status
```

## Install Noctalia 5

Use this adapter when the running shell is the native Noctalia 5 process.

```sh
mkdir -p ~/.local/share/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v5/massiveeq/. ~/.local/share/noctalia/plugins/massiveeq/
noctalia msg plugins enable massiveeq/massiveeq
```

Add **MassiveEQ** to the bar in Noctalia settings. Its manual widget id is
`massiveeq/massiveeq:quick-controls`.

For a source checkout, replace `/usr/share/massiveeq/noctalia-v5/massiveeq/`
with `packaging/noctalia-v5/massiveeq/`.

## Install Noctalia 4

Use this adapter when the shell is running as `qs -c noctalia-shell`.

```sh
mkdir -p ~/.config/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v4/massiveeq/. ~/.config/noctalia/plugins/massiveeq/
```

Restart or reload Noctalia, enable **MassiveEQ** under
**Settings → Plugins**, and add the widget to the desired bar section.

For a source checkout, copy `packaging/noctalia-v4/massiveeq/` instead.

## Choosing the adapter

The two adapters may be installed at the same time because they use different
directories. Only enable the adapter belonging to the shell you are currently
running.

```sh
pgrep -a -f 'qs.*-c noctalia-shell|(^|/)noctalia($| )'
```

- A `qs -c noctalia-shell` process means Noctalia 4.
- A native `noctalia` process means Noctalia 5.
- `noctalia --version` alone is not sufficient when both generations are
  installed, because it reports the CLI found first in `PATH`, not necessarily
  the running shell.

## Widget behavior

The bar uses one waveform glyph. The v4 adapter follows Noctalia's documented
full-height click-area and themed-capsule pattern; the v5 host owns the widget
capsule and pointer behavior. Both approaches keep scaling and hover feedback
correct on horizontal and vertical bars.

| Glyph state | Meaning |
|---|---|
| Primary color | Engine on and an output is actively filtered |
| Normal foreground | Engine on but no output is currently being processed |
| Muted foreground | Engine off |
| Error color | MassiveEQ is unavailable |

- **Left-click:** open or close the attached quick-controls panel.
- **Right-click:** open the complete GTK editor.
- **Escape or click away:** close the panel using Noctalia's panel behavior.

## Panel controls

The panel intentionally contains fast listening controls, not the complete
profile editor.

### Engine

The header switch controls the master engine. Engine Off is true dry audio at
0 dB: it bypasses EQ, level matching, safety attenuation, and manual trim.

### Output

Only one connected playback output is rendered at a time. The panel first
selects WirePlumber's current default sink, then an actively filtered assigned
output, another assigned output, and finally the first connected output. When
several outputs are connected, the output selector keeps the others available
without creating duplicate cards. A manual choice is remembered while the
panel remains loaded and falls back automatically if that device disconnects.

Disconnected devices remain visible only in the complete editor.

### Profile

The profile selector assigns a saved profile or explicitly unassigns the
output. Invalid profiles remain visible with an unavailable/invalid label but
cannot be activated. Fix them in the complete editor.

### Comparison bank

When the selected output has an active comparison bank, the panel displays up
to nine level-matched candidates. Selecting a candidate updates the running
audio chain without closing the panel. `Off · level matched` is the comparison
bank's dry candidate; it is different from the master Engine Off state.

### Filters

The Filters switch bypasses EQ for only the selected output while retaining
its level-matched playback level and PipeWire endpoint. Parametric profiles
show up to ten compact filter strips:

- filter type icon and abbreviation
- frequency in Hz
- gain in dB
- Q

Frequency is limited to 20–20,000 Hz, gain to −60–60 dB, and Q to
0.01–1,000. Press Enter to commit a value. Noctalia 4 also commits when the
field loses focus. Changes are written through `massiveeqctl set-filter`, so
the daemon, full editor, tray, and both Noctalia adapters receive the same
refreshed state.

GraphicEQ and convolution profiles remain assignable, but their specialized
editing stays in the full application.

### Status and errors

The panel has explicit loading, offline, no-output, and action-error states.
**Retry** refreshes the desktop-neutral status helper. **Open full editor** is
always available at the bottom of the panel.

## Multiple monitors and bar positions

Both adapters use Noctalia's host-owned panel placement. The v4 bar widget
reads per-screen capsule height, font size, and bar position. The v5 adapter
uses the host widget capsule and attached panel APIs. Supported layouts are:

- top and bottom bars
- left and right bars
- different scaling per monitor
- repeated widget instances on multiple monitors

The audio state is shared, but each widget instance opens on its own screen.

## How the integration works

The adapters are presentation-only clients. They do not load audio libraries,
open PipeWire nodes, read profile files directly, or run with elevated
privileges.

1. `massiveeqctl status` normalizes the existing user-session D-Bus service
   into versioned JSON.
2. The v4 adapter keeps a watched helper process and refreshes on the daemon's
   `Changed` signal. The v5 adapter polls the same snapshot and survives a
   stopped or replaced daemon.
3. Every control calls a narrowly scoped `massiveeqctl` mutation command.
4. The daemon validates and applies the request, then publishes refreshed
   state to the editor, tray, and widget.

Device keys and profile ids are passed as arguments; they are not interpreted
as plugin code. The v5 adapter quotes each argument before invoking the host's
asynchronous command runner. Neither adapter invokes `sudo`, a package manager,
or the system service manager.

## Generic tray icon

Once the native widget works, disable the generic tray companion to prevent a
duplicate icon:

```sh
systemctl --user disable --now massiveeq-tray.service
```

This command does not stop `massiveeq.service`, change the engine state, or
alter audio routing. Restore the generic tray with:

```sh
systemctl --user enable --now massiveeq-tray.service
```

## Updating the adapter

Package upgrades refresh the adapters under `/usr/share/massiveeq/`. Copy the
matching directory to the per-user plugin directory again, then reload
Noctalia. Existing MassiveEQ profiles and assignments are not stored in the
plugin directory and are unaffected.

## Troubleshooting

### Widget says unavailable

```sh
systemctl --user status massiveeq.service
massiveeqctl status
journalctl --user -u massiveeq.service -n 100 --no-pager
```

If `massiveeqctl status` works but the widget does not, confirm the helper is
in the environment inherited by Noctalia and reload the plugin.

### Wrong or duplicate output

Confirm WirePlumber's current sink:

```sh
wpctl inspect @DEFAULT_AUDIO_SINK@
massiveeqctl status
```

Use the output selector when more than one connected output is present. The
panel never renders disconnected devices.

### Duplicate bar icons

Disable `massiveeq-tray.service`; do not disable `massiveeq.service`.

### v5 plugin is incompatible

Current v5 accepts plugin APIs 3–4. If a later beta changes that range, compare
the installed Noctalia documentation with `plugin_api = 3` in
`plugin.toml`, then use the MassiveEQ adapter release matching that host.

### Full editor opens but the panel does not update

Run `massiveeqctl status --watch`. A snapshot should appear immediately and
again after every change. Restart the Noctalia plugin if the helper stream or
v5 polling service was replaced during an upgrade.

## Removal

Remove the widget from the bar and disable the plugin in Noctalia settings.
Then remove only the matching adapter directory:

```sh
rm -rf ~/.config/noctalia/plugins/massiveeq
rm -rf ~/.local/share/noctalia/plugins/massiveeq
```

Removing a widget adapter never deletes profiles, stops the audio daemon, or
changes active routing.

## Screenshots

See the [MassiveEQ screenshot gallery](screenshots.md) for the widget panel,
comparison controls, full editor, visual, text, and convolution editors,
comparison mode, and tray. The live Noctalia captures were taken on 4.7.7;
the v5 adapter exposes the same control set through v5's native Luau widgets.
