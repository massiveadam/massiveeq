source "$(dirname "$(realpath "${BASH_SOURCE[0]}")")/curves-fixture.sh"
scenario() {
  sleep 1
  shot listening-wide.png
  ui setnamedvalue "Bass" 4
  ui setnamedvalue "Tilt" -3
  ui setnamedvalue "Bass" 3.5
  ui setnamedvalue "Bass" 4
  sleep 1.5
  shot listening-shaped.png
  local text
  text=$(meq ListProfiles | jq -r '.[] | select(.name == "HD 650") | .text')
  python3 - "$text" <<'PY'
import sys
s=sys.argv[1]
assert 'Fc 105.00 Hz Gain 4.00' in s and 'Fc 1000.00 Hz Gain -1.50' in s and 'Fc 1000.00 Hz Gain 1.50' in s, s
assert s.count('Filter ') == 6, s
print('PASS: bass/tilt saved as standard shelf filters')
PY
  python3 - "$(dirname "$(realpath "$0")")" <<'PY'
import sys
sys.path.insert(0,sys.argv[1]);import ui
for name,value in [('Bass',4),('Tilt',-3)]:
    actual=ui.find(name,'slider').get_value_iface().get_current_value()
    assert actual==value,(name,actual,value)
print('PASS: repeated bass/tilt edits retain their readouts and reuse the same shelves')
PY
  ui click "Undo" button
  sleep 1
  shot listening-undo.png
  ui click "Redo" button
  # Saving is debounced; wait for the service's copy rather than a fixed delay.
  for _ in $(seq 1 25); do
    meq ListProfiles | jq -e '.[] | select(.name == "HD 650") | .text | contains("Fc 1000.00 Hz Gain -1.50")' >/dev/null && break
    sleep 0.2
  done
  meq ListProfiles | jq -e '.[] | select(.name == "HD 650") | .text | contains("Fc 1000.00 Hz Gain -1.50")' >/dev/null
  echo 'PASS: undo and redo return the complete grouped edit'
  # Starting sound lives in the A/B menu; tone controls in the listen menu.
  ui click "A/B" "toggle button"
  sleep 0.5
  ui click "Hear starting sound" "toggle button"
  sleep 0.7
  shot listening-start.png
  ui click "Hear starting sound" "toggle button"
  # Close A/B first; one menu opens at a time.
  ui click "A/B" "toggle button"
  sleep 0.3
  ui click "Sweep and signal" "toggle button"
  sleep 0.5
  ui setnamedvalue "Graph tone frequency" 0.4
  ui click "Listen" "toggle button"
  sleep 0.7
  test "$(pw-dump | jq '[.[] | select(.info.props."node.name"? == "massiveeq.tone")] | length')" = 1
  ui click "Sweep" "toggle button"
  sleep 0.7
  ui click "Sweep" "toggle button"
  sleep 0.2
  python3 - "$(dirname "$(realpath "$0")")" <<'PY'
import sys,time
sys.path.insert(0,sys.argv[1]);import ui
node=ui.find('Graph tone frequency','slider');a=node.get_value_iface().get_current_value();time.sleep(.4);b=node.get_value_iface().get_current_value()
assert a>.41 and abs(a-b)<.001, (a,b)
print('PASS: sweep advances logarithmically, then holds its frequency when paused',a,b)
PY
  shot listening-sweep.png
  ui click "−1 dB" button
  sleep 1
  ui click "Listen" "toggle button"
  # A stop can wait behind a save already in flight after the correction.
  for _ in $(seq 1 15); do
    test "$(pw-dump | jq '[.[] | select(.info.props."node.name"? == "massiveeq.tone")] | length')" = 0 && break
    sleep 0.2
  done
  test "$(pw-dump | jq '[.[] | select(.info.props."node.name"? == "massiveeq.tone")] | length')" = 0
  echo 'PASS: graph listening, correction at held frequency, and stop'
  # Close the sweep menu first; one menu opens at a time.
  ui click "Sweep and signal" "toggle button"
  sleep 0.3
  ui click "CURVES" "toggle button"
  sleep 0.4
  ui click "Measurement details" button
  sleep 0.5
  shot listening-metadata.png
  ui settext "Source" "My measurement export" "text"
  ui settext "Measurement fixture" "B&K 5128" "text"
  ui click "Save curve details" button
  sleep 0.7
  local id
  id=$(meq ListProfiles | jq -r '.[] | select(.name == "HD 650") | .id')
  meq GetCurves "$id" | jq -e '.measurement_metadata.source == "My measurement export" and .measurement_metadata.fixture == "B&K 5128"' >/dev/null
  echo 'PASS: curve provenance edited through the app and saved with the profile'
  if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then return 1; fi
}
