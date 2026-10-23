# Measurement and target-range overlays behind a correcting EQ.
prepare() {
  local work=${MEQ_TESTFLIGHT_ROOT:-$XDG_RUNTIME_DIR}/curves id text
  mkdir -p "$work"
  python3 - "$work" <<'PY'
import math, sys
work = sys.argv[1]
freqs = [20 * 1000 ** (i / 199) for i in range(200)]
def bell(f, fc, gain, width):
    return gain * math.exp(-(math.log2(f / fc) / width) ** 2)
with open(f"{work}/hd.csv", "w") as out:
    out.write("frequency,raw\n")
    for f in freqs:
        db = 80 - 6 / (1 + (f / 60) ** 2) + bell(f, 3000, 5, 0.4) + bell(f, 9000, -4, 0.3) - 6 / (1 + (12000 / f) ** 4)
        out.write(f"{f:.2f},{db:.2f}\n")
with open(f"{work}/target.txt", "w") as out:
    out.write("* frequency low high\n")
    for f in freqs:
        center = 4 / (1 + (f / 105) ** 2) + bell(f, 2800, 8, 0.9) - 3 / (1 + (8000 / f) ** 2)
        out.write(f"{f:.2f} {center - 2:.2f} {center + 2:.2f}\n")
PY
  id=$(meq CreateProfile "HD 650" | jq -r .id)
  text='Preamp: -3 dB
Filter 1: ON LS Fc 105 Hz Gain 4 dB Q 0.7
Filter 2: ON PK Fc 3000 Hz Gain 3 dB Q 1.4
Filter 3: ON PK Fc 9000 Hz Gain 3 dB Q 3'
  busctl --user call org.massiveeq.Service1 /org/massiveeq/Service1 org.massiveeq.Service1 \
    PutProfile sssd "$id" "HD 650" "$text" 0 >/dev/null
  busctl --user call org.massiveeq.Service1 /org/massiveeq/Service1 org.massiveeq.Service1 \
    SetCurve sss "$id" measurement "$work/hd.csv" >/dev/null
  echo "TARGET $(busctl --user --json=short call org.massiveeq.Service1 /org/massiveeq/Service1 org.massiveeq.Service1 \
    SetCurve sss "$id" target "$work/target.txt" | jq -r '.data[0]' | jq -c '{measurement: (.measurement | length), target: (.target | length), first: .target[0]}')"
  meq AssignProfile "$(device_key meq-test-sink)" "$id"
  sleep 1
}
scenario() {
  shot c1-curves.png
  ui click "CURVES" "toggle button"
  sleep 0.7
  shot c1b-menu.png
  ui click "CLEAR TARGET" button
  sleep 1
  shot c2-no-target.png
}
