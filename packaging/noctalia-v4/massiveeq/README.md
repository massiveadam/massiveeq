# MassiveEQ for Noctalia 4

This optional plugin adds a native MassiveEQ bar widget and attached quick-controls panel. It
requires MassiveEQ, `massiveeqctl`, and Noctalia 4.7.x. It does not replace or modify the
MassiveEQ audio service.

## Install from an Arch package

```sh
mkdir -p ~/.config/noctalia/plugins/massiveeq
cp -a /usr/share/massiveeq/noctalia-v4/massiveeq/. ~/.config/noctalia/plugins/massiveeq/
```

For a source checkout, copy the contents of `packaging/noctalia-v4/massiveeq` to the same
destination instead.
Restart Noctalia, enable **MassiveEQ** under **Settings → Plugins**, then add the MassiveEQ widget
to the desired bar section.

Once the widget is working, disable the generic tray companion to avoid showing two icons:

```sh
systemctl --user disable --now massiveeq-tray.service
```

This only removes the generic tray icon. `massiveeq.service` remains active and audio routing is
unchanged. To return to the generic StatusNotifier tray, remove the widget and run:

```sh
systemctl --user enable --now massiveeq-tray.service
```

Left-click the widget to open quick controls for the currently routed output. The active
parametric profile includes compact frequency, gain, and Q fields; committing a value updates
the live profile. Right-click opens the full MassiveEQ editor.
