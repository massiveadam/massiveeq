# MassiveEQ for Noctalia 5

This is the Noctalia 5 adapter for MassiveEQ. It uses Noctalia's native Luau
plugin API and the desktop-neutral `massiveeqctl` helper. The existing
Noctalia 4/QML adapter is separate and remains supported.

The adapter uses `wpctl` (provided by WirePlumber) to identify the currently
routed playback sink. Other detected sound devices remain available in the
output selector instead of becoming duplicate cards.

Noctalia 5 is currently beta software, so its plugin API can still change.
Current hosts accept plugin APIs 3–4. This adapter deliberately declares API 3
so it works across early and current v5 builds without using API 4-only
features.

## Install from the Arch package

```sh
mkdir -p ~/.local/share/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v5/massiveeq/. ~/.local/share/noctalia/plugins/massiveeq/
noctalia msg plugins enable massiveeq/massiveeq
```

Then add **MassiveEQ** to a bar in Noctalia settings. The widget entry id is
`massiveeq/massiveeq:quick-controls` if you edit the configuration manually.

For a source checkout, copy `packaging/noctalia-v5/massiveeq` to the same
per-user data directory. Restart or reload Noctalia after replacing the files.

Once the native widget is working, the generic StatusNotifier icon can be
disabled to avoid duplicates:

```sh
systemctl --user disable --now massiveeq-tray.service
```

This only disables the tray companion. It does not stop `massiveeq.service`,
change the engine switch, or alter active audio routing.

## Remove

Remove the widget from the bar, disable the plugin in Noctalia settings, then:

```sh
rm -rf ~/.local/share/noctalia/plugins/massiveeq
```

The full editor, daemon, profiles, audio routing, and generic tray companion
are unaffected.

The complete feature, compatibility, update, and troubleshooting reference is
in [`docs/noctalia-widget.md`](../../../docs/noctalia-widget.md).
