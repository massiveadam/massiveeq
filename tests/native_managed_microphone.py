#!/usr/bin/env python3
"""Opt-in managed microphone proof in private PipeWire; uses synthetic audio only.

Requires carla, pipewire-jack, wireplumber, lsp-plugins-lv2,
noise-suppression-for-voice and an X display (Xvfb works).
Run: python tests/native_managed_microphone.py.
"""
import array
import importlib.machinery
import importlib.util
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
loader = importlib.machinery.SourceFileLoader("microphone", str(ROOT / "packaging/massiveeq-input"))
spec = importlib.util.spec_from_loader(loader.name, loader)
mic = importlib.util.module_from_spec(spec)
loader.exec_module(mic)


def wait_for(predicate, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (RuntimeError, OSError, subprocess.SubprocessError):
            pass
        time.sleep(0.1)
    raise AssertionError("timed out waiting for the private audio graph")


def main():
    with tempfile.TemporaryDirectory(prefix="massiveeq-managed-test-") as temp:
        root = Path(temp)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        os.environ.update(XDG_RUNTIME_DIR=str(runtime), PIPEWIRE_RUNTIME_DIR=str(runtime),
                          XDG_STATE_HOME=str(root / "state"), XDG_CONFIG_HOME=str(root / "config"),
                          CARLA_DONT_MANAGE_CONNECTIONS="1",
                          PIPEWIRE_PROPS='{ "node.autoconnect": false, "node.always-process": true }')
        children = []
        log = open(root / "session.log", "w+")
        def start(args):
            process = subprocess.Popen(args, stdout=log, stderr=log)
            children.append(process)
            return process
        def stop(process):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        config = root / "input.json"
        profile = {"id": "managed-test", "name": "Managed microphone", "source": "test-mic", "channels": ["MONO"],
                   "eq_enabled": True, "filters": [{"enabled": True, "kind": "Peaking", "frequency": 1000.0,
                   "gain_db": 0.0, "q": 1.0, "channels": "All"}], "rnnoise": {"enabled": False},
                   "gate": {"enabled": False}, "compressor": {"enabled": False}, "advanced_project": None}
        def save():
            tmp = config.with_suffix(".tmp")
            tmp.write_text(json.dumps({"version": 1, "active_profile": profile["id"], "profiles": [profile]}))
            tmp.replace(config)
        def status():
            return json.loads((runtime / "massiveeq-input-status.json").read_text())
        def source_id():
            return next((obj["id"] for obj in mic.graph() if obj.get("type") == "PipeWire:Interface:Node"
                         and obj.get("info", {}).get("props", {}).get("node.name") == mic.SOURCE), None)
        counter = 0
        def record(voiced=False):
            nonlocal counter
            counter += 1
            if voiced:
                values = ((0.14 * math.sin(2 * math.pi * 180 * i / 48000) + 0.07 * math.sin(2 * math.pi * 360 * i / 48000)
                           + 0.035 * math.sin(2 * math.pi * 540 * i / 48000)) * (0.7 + 0.3 * math.sin(2 * math.pi * 3 * i / 48000))
                          for i in range(48000 * 6))
            else:
                values = (0.1 * math.sin(2 * math.pi * 1000 * i / 48000) for i in range(48000 * 6))
            tone = root / f"tone{counter}.raw"
            tone.write_bytes(array.array("f", values).tobytes())
            recorded = root / f"recorded{counter}.raw"
            recorder = start(["pw-cat", "--record", "--target", "0", "--rate", "48000", "--channels", "1",
                              "--channel-map", "MONO", "--format", "f32", "--raw", "--sample-count", "144000",
                              "--properties", '{ node.name = test-recorder node.autoconnect = false }', str(recorded)])
            player = start(["pw-cat", "--playback", "--target", "0", "--rate", "48000", "--channels", "1",
                            "--channel-map", "MONO", "--format", "f32", "--raw",
                            "--properties", '{ node.name = test-player node.autoconnect = false }', str(tone)])
            wait_for(lambda: mic.node_ports(mic.graph(), "test-player", "out") and mic.node_ports(mic.graph(), "test-recorder", "in"))
            for left, right in [("test-player:output_MONO", "test-feed:input_MONO"),
                                (f"{mic.SOURCE}:capture_MONO", "test-recorder:input_MONO")]:
                subprocess.run(["pw-link", left, right], check=True)
            recorder.wait(timeout=15)
            stop(player)
            data = array.array("f")
            data.frombytes(recorded.read_bytes())
            middle = data[48000:120000]
            assert middle, "recording is empty"
            rms = math.sqrt(sum(value * value for value in middle) / len(middle))
            assert math.isfinite(rms)
            return rms
        try:
            start(["pipewire"])
            wait_for(lambda: (runtime / "pipewire-0").exists())
            start(["dbus-run-session", "--", "wireplumber"])
            source = start(["pw-loopback", "--channels", "1", "--channel-map", "[ MONO ]",
                            "--capture-props", '{ node.name = test-feed node.autoconnect = false }',
                            "--playback-props", '{ node.name = test-mic media.class = Audio/Source node.autoconnect = false }'])
            wait_for(lambda: mic.node_ports(mic.graph(), "test-mic", "out"))
            save()
            helper = start(["pw-jack", sys.executable, str(ROOT / "packaging/massiveeq-input"), "run", "--config", str(config)])
            wait_for(lambda: status()["state"] == "running")
            published = source_id()
            assert published
            unity = record()
            assert 0.06 < unity < 0.08, unity
            profile["filters"][0]["gain_db"] = -12.0
            save()
            time.sleep(0.8)
            assert source_id() == published, "EQ edit replaced the virtual microphone"
            cut = record()
            assert 0.24 < cut / unity < 0.26, (unity, cut)
            profile["id"] = "second-profile"
            profile["filters"][0]["gain_db"] = -6.0
            save()
            wait_for(lambda: status()["state"] == "running" and status()["profile_id"] == profile["id"])
            assert source_id() == published, "profile switch replaced the virtual microphone"
            half = record()
            assert 0.49 < half / unity < 0.52, (unity, half)
            profile["gate"] = {"enabled": True, "threshold_db": 0.0, "attack_ms": 0.1, "release_ms": 10.0}
            save()
            time.sleep(0.8)
            gated = record()
            assert gated < half * 0.01, (half, gated)
            profile["gate"]["enabled"] = False
            profile["compressor"] = {"enabled": True, "threshold_db": 0.0, "ratio": 1.0,
                                     "attack_ms": 10.0, "release_ms": 120.0, "makeup_db": 6.0}
            save()
            time.sleep(0.8)
            makeup = record()
            assert 0.98 < makeup / unity < 1.02, (unity, makeup)
            profile["rnnoise"]["enabled"] = True
            profile["gate"] = {"enabled": True, "threshold_db": -60.0, "attack_ms": 5.0, "release_ms": 100.0}
            profile["compressor"] = {"enabled": True, "threshold_db": -30.0, "ratio": 3.0,
                                     "attack_ms": 10.0, "release_ms": 120.0, "makeup_db": 0.0}
            save()
            time.sleep(2)
            wait_for(lambda: status()["state"] == "running")
            assert source_id() == published, "voice toggle replaced the virtual microphone"
            speech = record(voiced=True)
            assert 1e-8 < speech < 0.5, speech
            assert helper.poll() is None
            stop(helper)
            assert status()["state"] == "stopped"
            wait_for(lambda: source_id() is None)
            assert not mic.node_ports(mic.graph(), mic.CLIENT, "out")
            helper = start(["pw-jack", sys.executable, str(ROOT / "packaging/massiveeq-input"), "run", "--config", str(config)])
            wait_for(lambda: status()["state"] == "running")
            stop(source)
            assert helper.wait(timeout=15) != 0
            wait_for(lambda: source_id() is None)
            assert status()["state"] == "error"
            assert not mic.node_ports(mic.graph(), mic.CLIENT, "out")
            print(f"Managed microphone: unity={unity:.7f}, -12dB={cut:.7f}, -6dB={half:.7f}, gate={gated:.9f}, makeup={makeup:.7f}, voice chain={speech:.7f}")
            print("PASS managed creation, live EQ/profile/voice edits with stable source, status, stop and unplug")
        except BaseException:
            log.flush()
            print((root / "session.log").read_text(), file=sys.stderr)
            raise
        finally:
            for process in reversed(children):
                stop(process)
            log.close()


if __name__ == "__main__":
    if not os.environ.get("MASSIVEEQ_TEST_PW_JACK"):
        os.execvpe("pw-jack", ["pw-jack", sys.executable, __file__], os.environ | {"MASSIVEEQ_TEST_PW_JACK": "1"})
    main()
