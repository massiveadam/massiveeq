#!/usr/bin/env bash
# Source through devbox/massiveeq-shot.sh; private synthetic graph and XDG dirs.
prepare() {
  pw-loopback --channels 1 --channel-map '[ MONO ]' \
    --capture-props '{ node.name = input-fixture-feed node.autoconnect = false }' \
    --playback-props '{ node.name = input-fixture-mic node.description = "Test Microphone" media.class = Audio/Source node.autoconnect = false }' >>"$log" 2>&1 &
  sleep 0.5
}
scenario() {
  python3 "$repo/.agents/skills/verify-massiveeq/input-workspace-ui.py" "$out"
  if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then return 1; fi
}
