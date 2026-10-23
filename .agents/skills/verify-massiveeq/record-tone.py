"""Record the candidate DSP output before the hardware volume/mute control."""
import array
import json
import math
import subprocess
import sys
import time
import wave

nodes = json.loads(subprocess.check_output(["pw-dump"], text=True))
outputs = [n["info"]["props"]["node.name"] for n in nodes
           if n.get("info", {}).get("props", {}).get("node.name", "").startswith("massiveeq.output.")]
assert len(outputs) == 1, outputs
process = subprocess.Popen([
    "pw-record", "--target", "0", "-P", "node.name=massiveeq.testflight.probe",
    "--format", "s16", "--channels", "2", sys.argv[1],
], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
try:
    time.sleep(0.5)
    ports = subprocess.check_output(["pw-link", "-o"], text=True).splitlines()
    for channel in ("FL", "FR"):
        source = next(p.strip() for p in ports
                      if p.strip().startswith(outputs[0] + ":") and p.strip().endswith("_" + channel))
        subprocess.run(["pw-link", source, "massiveeq.testflight.probe:input_" + channel], check=True)
    time.sleep(1.5)
finally:
    process.send_signal(2)
    process.wait(timeout=5)
with wave.open(sys.argv[1]) as recording:
    values = array.array("h", recording.readframes(recording.getnframes()))
samples = values[len(values) // 3::2]
assert samples, "No recorded samples"
rms = math.sqrt(sum((v / 32768) ** 2 for v in samples) / len(samples))
assert rms > 1e-7, "The DSP output is silent"
print(20 * math.log10(rms))
