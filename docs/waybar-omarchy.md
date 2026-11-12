# Use MassiveEQ from Waybar and Omarchy

MassiveEQ uses Waybar's standard StatusNotifier tray. The package includes the
tray companion, icon, and user service. Enable the companion once:

```sh
systemctl --user enable --now massiveeq.service massiveeq-tray.service
```

If your bar already has a tray, the MassiveEQ icon appears there. The editor
enables the audio service on first launch; the tray companion is a separate
opt-in so a native Noctalia widget can be used without a duplicate icon.

## Add a tray to Waybar

If your bar has no tray, add `"tray"` to an existing module list in
`~/.config/waybar/config.jsonc` (or `config`). Keep the other modules:

```jsonc
"modules-right": ["pulseaudio", "tray", "clock"],
"tray": {
  "icon-size": 18,
  "spacing": 8,
  "tooltip": true
}
```

The package also installs this tray block at
`/usr/share/massiveeq/waybar/tray.jsonc`. You can copy the block into your
configuration or add that path to your existing Waybar `include` list.
An include configures the module; you still need `"tray"` in a module list.
Restart Waybar through your desktop's usual bar controls after editing.

MassiveEQ does not modify your bar configuration or require a custom script,
font icon, or additional menu launcher.

## Reveal the icon on Omarchy

On Omarchy setups using Waybar, the default tray is inside a drawer. Expand
the tray with the arrow in the bar to reach MassiveEQ. To keep all tray icons
visible, replace `"group/tray-expander"` with `"tray"` in the `modules-right`
list in `~/.config/waybar/config.jsonc`. Do not add a second tray if the drawer
already contains one. See the [Omarchy manual's tray instructions](https://learn.omacom.io/2/the-omarchy-manual/102/common-tweaks).

Omarchy configurations using a different bar can use the same companion if
that bar supports StatusNotifier items. This guide's Waybar configuration
applies only to Waybar.

## Change profiles and settings

- **Click or right-click** the icon to open quick controls. The system output
  appears first, followed by other connected outputs and saved disconnected
  devices.
- Open an output's submenu to assign a saved profile, leave it unassigned,
  toggle **Filters**, or switch candidates in an active comparison bank.
- **Engine enabled** controls processing for every output. **Filters** keeps
  the selected output's level matching while bypassing its filters.
- **Open MassiveEQ** opens the editor for frequency, gain, Q, profile editing,
  and other settings.
- **Middle-click** toggles the engine. The tooltip names the output and its
  profile. Menu state refreshes after changes and at least every three seconds.
- **Quit tray icon** closes only the companion. Audio continues processing.

## Restore a missing icon

Check that the service is running:

```sh
systemctl --user status massiveeq-tray.service
journalctl --user -u massiveeq-tray.service -b
```

If you selected **Quit tray icon**, start it again with
`systemctl --user start massiveeq-tray.service`.

If the service is running, check the tray drawer and confirm that Waybar's
`tray` module is enabled. Remove any tray `ignore-list` or `icons` rule that
hides `massiveeq-tray`. The active icon uses the installed
`org.massiveeq.MassiveEQ` icon; source builds need the icon installed in a GTK
icon theme. An error icon indicates that the audio service is unavailable:
open the menu for its error and **Retry connection**.

To use a native Noctalia widget instead, disable the generic companion:

```sh
systemctl --user disable --now massiveeq-tray.service
```

See [Waybar's tray reference](https://github.com/Alexays/Waybar/wiki/Module:-Tray)
for icon size, spacing, and visibility options.
