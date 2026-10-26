#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} != --session ]]; then
  repo=$(realpath "$1")
  evidence=$(realpath -m "$2")
  ir=$(realpath "$3")
  mkdir -p "$evidence"
  session=$(mktemp -d /tmp/meq-session.XXXXXX)
  export XDG_RUNTIME_DIR=$session/run XDG_CONFIG_HOME=$session/config \
    XDG_DATA_HOME=$session/data XDG_STATE_HOME=$session/state XDG_CACHE_HOME=$session/cache
  mkdir -p "$XDG_RUNTIME_DIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_CACHE_HOME"
  chmod 700 "$XDG_RUNTIME_DIR"
  unset DBUS_SESSION_BUS_ADDRESS PIPEWIRE_REMOTE PULSE_SERVER
  exec dbus-run-session -- bash "$0" --session "$repo" "$evidence" "$ir"
fi
repo=$2
export BRIR_EVIDENCE=$3 BRIR_CLIPPING_IR=$4
log=$BRIR_EVIDENCE/session.log
out=$BRIR_EVIDENCE/unused.png
mkdir -p "$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d"
cat >"$XDG_CONFIG_HOME/wireplumber/wireplumber.conf.d/10-private.conf" <<'CONF'
wireplumber.profiles = {
  main = {
    monitor.alsa = disabled
    monitor.alsa-midi = disabled
    monitor.bluez = disabled
    monitor.bluez-midi = disabled
    monitor.v4l2 = disabled
    monitor.libcamera = disabled
  }
}
CONF
children=()
cleanup() { kill "${children[@]}" 2>/dev/null || true; wait 2>/dev/null || true; }
trap cleanup EXIT
pipewire >>"$log" 2>&1 & children+=("$!")
for _ in $(seq 1 50); do [[ -S $XDG_RUNTIME_DIR/pipewire-0 ]] && break; sleep .1; done
test -S "$XDG_RUNTIME_DIR/pipewire-0"
wireplumber >>"$log" 2>&1 & children+=("$!")
sleep 1
pw-cli create-node adapter '{ factory.name=support.null-audio-sink node.name=meq-test-sink node.description="Private Test Output" media.class=Audio/Sink audio.position=[FL FR] object.linger=true }' >>"$log" 2>&1
sleep .5
wpctl set-default "$(pw-dump | jq -r '.[] | select(.info.props."node.name" == "meq-test-sink") | .id')"
"$repo/target/release/massiveeqd" >>"$log" 2>&1 & children+=("$!")
meq() {
  local method=$1 signature=''; shift
  (($#)) && signature=$(printf 's%.0s' "$@")
  busctl --user --auto-start=no --json=short call org.massiveeq.Service1 /org/massiveeq/Service1 \
    org.massiveeq.Service1 "$method" ${signature:+"$signature"} "$@" | jq -r '.data[0] // empty'
}
device_key() { meq ListDevices | jq -r --arg n "$1" '.[] | select(.node_name == $n) | .key | "\(.backend)|\(.stable_id)|\(.route)"'; }
for _ in $(seq 1 50); do meq Ping >/dev/null 2>&1 && break; sleep .1; done
meq Ping >/dev/null
read -r _ owner < <(busctl --user call org.freedesktop.DBus /org/freedesktop/DBus \
  org.freedesktop.DBus GetConnectionUnixProcessID s org.massiveeq.Service1)
test "$owner" = "${children[2]}"
test "$(meq ListDevices | jq length)" = 1
source "$repo/.agents/skills/verify-massiveeq/brir-clipping-scenario.sh"
prepare
