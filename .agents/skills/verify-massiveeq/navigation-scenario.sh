#!/usr/bin/env bash
# Private devbox session; stress navigation with long profile and device names.
prepare() {
  local id key
  add_sink meq-navigation-output 'MOONDROP Dawn Pro Analog Stereo USB Audio Headphones'
  set_default meq-navigation-output
  id=$(meq ListProfiles | jq -r '.[0].id')
  busctl --user call org.massiveeq.Service1 /org/massiveeq/Service1 org.massiveeq.Service1 \
    PutProfile sssd "$id" 'Sennheiser HD 600 2.1 Stereo BRIR - very long listening profile' \
    $'Filter 1: ON PK Fc 3000 Hz Gain -3 dB Q 1\n' 0 >/dev/null
  key=$(device_key meq-navigation-output)
  meq AssignProfile "$key" "$id" >/dev/null
}
scenario() {
  python3 "$repo/.agents/skills/verify-massiveeq/navigation-ui.py" "$out"
  if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then return 1; fi
}
