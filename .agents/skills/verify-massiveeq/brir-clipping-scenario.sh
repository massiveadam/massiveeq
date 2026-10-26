# Run only inside devbox/massiveeq-shot.sh's private PipeWire/D-Bus session.
# Optional BRIR_CLIPPING_IR tests a supplied 24-bit four-channel WAV/IRS too.
# BRIR_EXPECT_LIMITED=0 records the old behavior for before/after comparison.
prepare() {
  [[ $XDG_RUNTIME_DIR == /tmp/meq-session.*/run ]] || return 1
  local evidence=${BRIR_EVIDENCE:-$(dirname "$out")/brir-evidence}
  mkdir -p "$evidence"
  python3 - "$evidence" "${BRIR_CLIPPING_IR:-}" <<'PY'
import math, pathlib, struct, sys, wave

root = pathlib.Path(sys.argv[1])
rate = 48000
def write(path, channels, samples):
    data = struct.pack('<' + 'f' * len(samples), *samples)
    fmt = struct.pack('<HHIIHH', 3, channels, rate, rate * channels * 4, channels * 4, 32)
    path.write_bytes(b'RIFF' + struct.pack('<I', 36 + len(data)) + b'WAVEfmt '
                    + struct.pack('<I', 16) + fmt + b'data' + struct.pack('<I', len(data)) + data)

write(root / 'synthetic.wav', 4, [1, .5, 1, .5, 1, .5, 1, .5, -1, -.5, -1, -.5])
kernels = {'synthetic': [[1, .5, 1, .5], [1, .5, 1, .5], [-1, -.5, -1, -.5]]}
if sys.argv[2]:
    with wave.open(sys.argv[2]) as source:
        assert (source.getnchannels(), source.getsampwidth(), source.getframerate()) == (4, 3, rate)
        raw = source.readframes(source.getnframes())
    values = [int.from_bytes(raw[i:i+3], 'little', signed=True) / 2**23
              for i in range(0, len(raw), 3)]
    kernels['supplied'] = [values[i:i+4] for i in range(0, len(values), 4)]

for name, impulse in kernels.items():
    samples = [0.] * (rate * 5 * 2)
    # Reverse the signs of both paths into one ear to stress transient
    # summing. Inputs stay within full scale. Repeat at -40 dB as a linear
    # reference, then include a full-scale 80 Hz tone for the bass shelf.
    for start, amplitude, ear in [(.5, .01, 0), (1.5, 1., 0), (2.5, 1., 1)]:
        for i, taps in enumerate(reversed(impulse)):
            frame = int(start * rate) + i
            for channel in range(2):
                value = taps[channel * 2 + ear]
                samples[frame * 2 + channel] = amplitude * (1 if value > 0 else -1 if value < 0 else 0)
    for i in range(rate):
        fade = min(1., i / 480, (rate - 1 - i) / 480)
        value = .99 * fade * math.sin(2 * math.pi * 80 * i / rate)
        samples[(int(3.5 * rate) + i) * 2:(int(3.5 * rate) + i) * 2 + 2] = [value, value]
    write(root / (name + '-input.wav'), 2, samples)
PY
  local key id case ir rec sink
  key=$(device_key meq-test-sink)
  test -n "$key"
  pw-metadata -n settings 0 clock.force-rate 48000 >>"$log" 2>&1
  for case in synthetic supplied supplied-bass; do
    ir=$evidence/synthetic.wav
    if [[ $case != synthetic ]]; then
      [[ -n ${BRIR_CLIPPING_IR:-} ]] || continue
      ir=$BRIR_CLIPPING_IR
    fi
    if [[ $case == supplied-bass ]]; then
      printf 'Filter 1: ON LSC Fc 105 Hz Gain 6 dB Q 0.7071\n' >"$evidence/$case.txt"
    else
      printf '# Private BRIR clipping test\n' >"$evidence/$case.txt"
    fi
    id=$(meq ImportProfile "$evidence/$case.txt" | jq -r .id)
    meq SetConvolution "$id" "$ir" ALL >"$evidence/$case-profile.json"
    meq AssignProfile "$key" "$id" >/dev/null
    sleep 1
    busctl --user --auto-start=no --json=short call org.massiveeq.Service1 /org/massiveeq/Service1 \
      org.massiveeq.Service1 Analyze su "$id" 48000 | jq -r '.data[0]' >"$evidence/$case-analysis.json"
    pw-record --target 0 -P node.name=meq-brir-probe --format f32 --rate 48000 --channels 2 \
      "$evidence/$case-output.wav" >>"$log" 2>&1 & rec=$!
    sleep .5
    pw-link meq-test-sink:monitor_FL meq-brir-probe:input_FL >>"$log" 2>&1
    pw-link meq-test-sink:monitor_FR meq-brir-probe:input_FR >>"$log" 2>&1
    sleep .3
    sink=$(meq Status | jq -r '.engine.active[0].node // empty')
    test -n "$sink"
    pw-play --target "$sink" "$evidence/${case%-bass}-input.wav" >>"$log" 2>&1
    sleep .3
    kill -INT "$rec"
    wait "$rec" || true
    meq Status >"$evidence/$case-status.json"
  done
  python3 - "$evidence" "${BRIR_EXPECT_LIMITED:-1}" <<'PY'
import array, json, math, pathlib, struct, sys
root = pathlib.Path(sys.argv[1])
for path in sorted(root.glob('*-output.wav')):
    raw = path.read_bytes()
    offset = 12
    while raw[offset:offset+4] != b'data':
        size, = struct.unpack_from('<I', raw, offset + 4)
        offset += 8 + size + size % 2
    size, = struct.unpack_from('<I', raw, offset + 4)
    samples = array.array('f', raw[offset+8:offset+8+size])
    assert samples and all(math.isfinite(v) for v in samples), path
    peak = max(abs(v) for v in samples)
    assert peak > .01, 'Silent or disconnected probe'
    if sys.argv[2] == '1':
        assert peak <= 10**(-1/20) + 2e-6, (path, peak)
    elif path.name.startswith('synthetic'):
        assert peak > 1.19, (path, peak)
    status = json.loads(path.with_name(path.stem.replace('-output', '-status') + '.json').read_text())
    assert not status['engine']['errors'], status
    assert len(status['engine']['active']) == 1, status
    active = status['engine']['active'][0]
    assert all(active[key] == 0 for key in ('invalid_buffers', 'input_overflows', 'output_underflows')), active
    print(f'{path.stem}: peak {20*math.log10(peak):.5f} dBFS, {sum(abs(v)>1 for v in samples)} samples above full scale')
PY
}

scenario() { :; }
