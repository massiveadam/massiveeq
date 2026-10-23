#!/usr/bin/env bash
# Run only in a private D-Bus session, against an otherwise unused ALSA output.
set -euo pipefail
repo=$(realpath "$1")
helpers=$(dirname "$(realpath "$0")")
test "${MEQ_TESTFLIGHT_PRIVATE_BUS:-}" = 1
test -n "${MEQ_TESTFLIGHT_ROOT:-}"
test -n "${WAYLAND_DISPLAY:-}"
if busctl --user --acquired --no-pager --no-legend list | grep '^org.massiveeq.Service1 ' >/dev/null; then
  echo 'This bus already has a MassiveEQ service; use a new private D-Bus session.' >&2
  exit 1
fi
if pgrep -x massiveeq >/dev/null; then
  echo 'An editor is already running; do not drive an unrelated instance.' >&2
  exit 1
fi
if pw-dump | jq -e 'any(.[]; (.info.props."node.name"? // "") | startswith("massiveeq."))' >/dev/null; then
  echo 'A MassiveEQ audio node is already running; select an unused output for this test.' >&2
  exit 1
fi
export XDG_CONFIG_HOME="$MEQ_TESTFLIGHT_ROOT/config" XDG_DATA_HOME="$MEQ_TESTFLIGHT_ROOT/data"
export XDG_STATE_HOME="$MEQ_TESTFLIGHT_ROOT/state" XDG_CACHE_HOME="$MEQ_TESTFLIGHT_ROOT/cache"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME" "$MEQ_TESTFLIGHT_ROOT/evidence"
log="$MEQ_TESTFLIGHT_ROOT/evidence/session.log"
bin="$repo/target/release"
service_pid= editor_pid=
cleanup() {
  local result=$?
  if ((result)); then
    ui dump >"$MEQ_TESTFLIGHT_ROOT/evidence/failure-ui.txt" 2>/dev/null || true
    shot failure.png 2>/dev/null || true
  fi
  [[ -z $editor_pid ]] || kill "$editor_pid" 2>/dev/null || true
  [[ -z $service_pid ]] || kill "$service_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  return "$result"
}
trap cleanup EXIT
meq() {
  local method=$1 signature=""; shift
  (($# == 0)) || signature=$(printf 's%.0s' "$@")
  busctl --user --auto-start=no --json=short call org.massiveeq.Service1 /org/massiveeq/Service1 \
    org.massiveeq.Service1 "$method" ${signature:+"$signature"} "$@" | jq -r '.data[0] // empty'
}
ui() { python3 "$helpers/ui.py" "$@"; }
device_key() { printf '%s\n' "$hardware_key"; }
shot() {
  local window_id
  window_id=$(niri msg --json windows | jq -r '.[] | select(.app_id == "org.massiveeq.MassiveEQ") | .id' | head -1)
  test -n "$window_id"
  niri msg action screenshot-window --id "$window_id" --path "$MEQ_TESTFLIGHT_ROOT/evidence/$1"
}
"$bin/massiveeqd" >>"$log" 2>&1 & service_pid=$!
for _ in $(seq 1 50); do meq Ping >/dev/null 2>&1 && break; sleep 0.2; done
meq Ping
hardware_key=$(meq ListDevices | jq -r '[.[] | select(.connected and (.node_name | startswith("alsa_output.")))][0].key | "\(.backend)|\(.stable_id)|\(.route)"')
test "$hardware_key" != 'null|null|null'
source "$helpers/listening-scenario.sh"
prepare
"$bin/massiveeq" >>"$log" 2>&1 & editor_pid=$!
sleep 3
# The existing listening scenario resolves its helper directory from $0.
scenario
ui click "Sweep and signal" "toggle button"
sleep 0.5
ui setnamedvalue "Graph tone frequency" 0.725363752
ui click "Listen" "toggle button"
sleep 1
before=$(python3 "$helpers/record-tone.py" "$MEQ_TESTFLIGHT_ROOT/evidence/before.wav")
# Bands are edited in the panel beside the selected band's node.
ui click "Select band 2" "toggle button"
sleep 0.4
ui setnamedvalue "Selected band Gain" -9
sleep 1
after=$(python3 "$helpers/record-tone.py" "$MEQ_TESTFLIGHT_ROOT/evidence/after.wav")
python3 - "$before" "$after" <<'PY'
import sys
before,after=map(float,sys.argv[1:])
assert after < before-3, (before,after)
print(f'PASS: recorded live ALSA path changed {after-before:+.1f} dB at 3 kHz')
PY
ui click "Listen" "toggle button"
sleep 0.5
meq Status >"$MEQ_TESTFLIGHT_ROOT/evidence/status.json"
jq -e '.engine.errors == {} and (.engine.active | length) == 1 and (.engine.active[0] | .input_overflows == 0 and .output_underflows == 0 and .invalid_buffers == 0 and .cpu_warning == false)' "$MEQ_TESTFLIGHT_ROOT/evidence/status.json"
shot benji-editor.png
ui setnamedvalue "Selected band Gain" -4.25
ui click "Close" button
sleep 0.8
meq ListProfiles | jq -e '.[] | select(.name == "HD 650") | .text | contains("Gain -4.25 dB")'
kill -0 "$editor_pid" 2>/dev/null && { echo 'Editor did not close'; exit 1; }
if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then exit 1; fi
echo 'PASS: benji listening workflow, real-device DSP recording, and close-time save'
