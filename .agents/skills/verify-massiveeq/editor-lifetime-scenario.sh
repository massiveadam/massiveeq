# Isolated native GTK regression for repeated launches and preview ownership.
editor_helpers=$(dirname "$(realpath "${BASH_SOURCE[0]}")")
prepare() {
 local id key
 id=$(meq ListProfiles | jq -r '.[0].id')
 busctl --user call org.massiveeq.Service1 /org/massiveeq/Service1 org.massiveeq.Service1 PutProfile sssd "$id" "Editor regression" $'Filter 1: ON PK Fc 3000 Hz Gain -3 dB Q 1\n' 0 >/dev/null
 key=$(device_key meq-test-sink)
 meq AssignProfile "$key" "$id" >/dev/null
}
scenario() {
 python3 "$editor_helpers/editor-ownership.py"
 "$bin/massiveeq" >>"$log" 2>&1 &
 local activation=$!
 wait "$activation"
 "$bin/massiveeq" >>"$log" 2>&1 &
 activation=$!
 wait "$activation"
 sleep 1
 local count
 count=$(swaymsg -s "$(printf '%s' "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)" -t get_tree | jq '[..|objects|select(.app_id? == "org.massiveeq.MassiveEQ")]|length')
 test "$count" = 1 || { echo "FAIL: repeated launches created $count editors"; return 1; }
 echo 'PASS: repeated activation keeps one editor window'
 local id saved revision
 id=$(meq ListProfiles | jq -r '.[0].id')
 ui click "Select band 1" "toggle button"
 sleep 0.5
 ui setnamedvalue "Selected band Gain" -7.25
 sleep 1
 saved=$(meq GetProfile "$id" | jq -r '.text')
 [[ $saved == *'Gain -7.25 dB'* ]]
 revision=$(meq Status | jq -r '.engine.active[0].revision')
 sleep 6
 test "$(meq Status | jq -r '.engine.active[0].revision')" = "$revision"
 echo 'PASS: live edit saves and holds one processing revision across renewal ticks'
 shot editor-lifetime.png
 ui click "Tone generator" "toggle button"
 sleep 0.5
 ui click "Sweep and signal" "toggle button"
 sleep 0.3
 ui click "Sweep" "toggle button"
 sleep 0.5
 test "$(pw-dump | jq '[.[]|select(.info.props."node.name"? == "massiveeq.tone")]|length')" = 1
 # Close immediately after changing a band, before its debounced save fires.
 ui setnamedvalue "Selected band Gain" -4.25
 ui click "Close" button
 sleep 1
 count=$(swaymsg -s "$(printf '%s' "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)" -t get_tree | jq '[..|objects|select(.app_id? == "org.massiveeq.MassiveEQ")]|length')
 test "$count" = 0
 test "$(pw-dump | jq '[.[]|select(.info.props."node.name"? == "massiveeq.tone")]|length')" = 0
 meq GetProfile "$id" | jq -e '.text | contains("Gain -4.25 dB")'
 test "$(busctl --user --acquired --no-pager --no-legend list | awk '$1 == "org.massiveeq.MassiveEQ" {print $1}')" = ""
 echo 'PASS: immediate close saves the newest edit, stops the playing sweep and exits the editor'
 "$bin/massiveeq" >>"$log" 2>&1 &
 sleep 3
 ui click "Select band 1" "toggle button"
 sleep 0.5
 ui setnamedvalue "Selected band Gain" -1.75
 sleep 1
 revision=$(meq Status | jq -r '.engine.active[0].revision')
 sleep 6
 test "$(meq Status | jq -r '.engine.active[0].revision')" = "$revision"
 meq GetProfile "$id" | jq -e '.text | contains("Gain -1.75 dB")'
 echo 'PASS: reopened editor edits normally without a stale session changing the result'
 local directory="$XDG_DATA_HOME/massiveeq/profiles/$id"
 chmod u-w "$directory"
 ui setnamedvalue "Selected band Gain" -4.75
 ui click "Close" button
 sleep 0.5
 ui dump >"$(dirname "$out")/editor-failed-save-ui.txt"
 rg -q 'could not be saved|Permission denied' "$(dirname "$out")/editor-failed-save-ui.txt"
 meq GetProfile "$id" | jq -e '.text | contains("Gain -1.75 dB")'
 echo 'PASS: a failed save keeps the editor open and preserves the saved profile'
 chmod u+w "$directory"
 ui click "Close" button
 sleep 1
 meq GetProfile "$id" | jq -e '.text | contains("Gain -4.75 dB")'
 echo 'PASS: retrying close saves successfully after write access is restored'
 if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then return 1; fi
}
