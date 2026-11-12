#!/usr/bin/env python3
"""Opt-in native VST3 signal test; uses a private PipeWire session, no hardware.

Requires Arch carla, pipewire, pipewire-jack, wireplumber, lsp-plugins-vst3,
python and an X display (Xvfb works). Run: python tests/native_microphone.py.
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
sys.path.insert(0, "/usr/share/carla")
from carla_backend import (CarlaHostDLL, BINARY_NATIVE, PLUGIN_VST3,
                          ENGINE_OPTION_PROCESS_MODE, ENGINE_PROCESS_MODE_CONTINUOUS_RACK)


def wait_for(predicate, seconds=15):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (RuntimeError, subprocess.SubprocessError):
            pass
        time.sleep(0.1)
    raise AssertionError("timed out waiting for the private audio graph")


def main():
    with tempfile.TemporaryDirectory(prefix="massiveeq-vst3-test-") as temp:
        root = Path(temp)
        runtime = root / "runtime"
        runtime.mkdir(mode=0o700)
        os.environ.update(XDG_RUNTIME_DIR=str(runtime), PIPEWIRE_RUNTIME_DIR=str(runtime),
                          XDG_STATE_HOME=str(root / "state"), XDG_CONFIG_HOME=str(root / "config"),
                          CARLA_DONT_MANAGE_CONNECTIONS="1",
                          PIPEWIRE_PROPS='{ "node.autoconnect": false, "node.always-process": true }')
        # Ensure the host uses PipeWire's JACK library even outside pw-jack.
        # The test itself is re-executed through pw-jack below.
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
        try:
            start(["pipewire"])
            wait_for(lambda: (runtime / "pipewire-0").exists())
            start(["dbus-run-session", "--", "wireplumber"])
            source = start(["pw-loopback", "--channels", "1", "--channel-map", "[ MONO ]",
                           "--capture-props", '{ node.name = test-feed node.autoconnect = false }',
                           "--playback-props", '{ node.name = test-mic media.class = Audio/Source node.autoconnect = false }'])
            wait_for(lambda: mic.node_ports(mic.graph(), "test-mic", "out"))
            samples = array.array("f", (0.1 * math.sin(2 * math.pi * 1000 * i / 48000) for i in range(48000 * 6)))
            tone = root / "tone.raw"
            tone.write_bytes(samples.tobytes())
            results = []
            for label, gain in [("unity", None), ("attenuated", 0.5)]:
                host = CarlaHostDLL("/usr/lib/carla/libcarla_standalone2.so", True)
                host.set_engine_option(ENGINE_OPTION_PROCESS_MODE, ENGINE_PROCESS_MODE_CONTINUOUS_RACK, "")
                assert host.engine_init("JACK", "fixture-builder"), host.get_last_error()
                try:
                    # Carla 2.5 loads the first class in a multi-class bundle.
                    # This is the first class in LSP's bundle, with a gain control.
                    assert host.add_plugin(BINARY_NATIVE, PLUGIN_VST3,
                        "/usr/lib/vst3/lsp-plugins.vst3", "A/B Tester x2 Mono",
                        "A/B Tester x2 Mono", -1066246689, None, 0), host.get_last_error()
                    assert host.get_plugin_info(0)["label"] == "A/B Tester x2 Mono"
                    host.set_active(0, True)
                    for index in range(host.get_parameter_count(0)):
                        host.set_parameter_value(0, index, host.get_parameter_ranges(0, index)["def"])
                    host.set_parameter_value(0, 3, 0.5)  # Select input 1; default 0 mutes all inputs.
                    if gain is not None:
                        host.set_parameter_value(0, 4, gain)
                    for _ in range(30):
                        host.engine_idle()
                        time.sleep(0.02)
                    project = root / f"{label}.carxp"
                    assert host.save_project(str(project)), host.get_last_error()
                finally:
                    host.engine_close()
                config = root / "input.json"
                config.write_text(json.dumps({"source": "test-mic", "project": str(project), "channels": ["MONO"]}))
                helper = start(["pw-jack", sys.executable, str(ROOT / "packaging/massiveeq-input"),
                                "run", "--config", str(config)])
                wait_for(lambda: mic.node_ports(mic.graph(), mic.SOURCE, "out"))
                wait_for(lambda: len([o for o in mic.graph() if o.get("type") == "PipeWire:Interface:Link"]) >= 3)
                recorded = root / f"{label}.raw"
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
                assert helper.poll() is None, "microphone helper failed during processing"
                data = array.array("f")
                data.frombytes(recorded.read_bytes())
                middle = data[48000:120000]
                rms = math.sqrt(sum(value * value for value in middle) / len(middle))
                assert math.isfinite(rms)
                results.append(rms)
                stop(helper)
                wait_for(lambda: not mic.node_ports(mic.graph(), mic.SOURCE, "out"))
                assert not mic.node_ports(mic.graph(), mic.CLIENT, "out"), "rack survived teardown"
            assert 0.06 < results[0] < 0.08, results
            assert 0.001 < results[1] < results[0] * 0.5, results
            # A removed input must remove the processed source; never leave a
            # dry fallback recording endpoint behind while supervision retries.
            helper = start(["pw-jack", sys.executable, str(ROOT / "packaging/massiveeq-input"),
                            "run", "--config", str(config)])
            wait_for(lambda: mic.node_ports(mic.graph(), mic.SOURCE, "out"))
            wait_for(lambda: len([o for o in mic.graph() if o.get("type") == "PipeWire:Interface:Link"]) >= 3)
            stop(source)
            assert helper.wait(timeout=15) != 0, "unplug did not fail closed"
            wait_for(lambda: not mic.node_ports(mic.graph(), mic.SOURCE, "out"))
            assert not mic.node_ports(mic.graph(), mic.CLIENT, "out"), "rack survived unplug"
            print(f"Native Linux VST3: unity RMS={results[0]:.7f}, changed gain RMS={results[1]:.7f}, "
                  f"attenuation={20 * math.log10(results[1] / results[0]):.3f} dB")
            print("PASS: source -> VST3 -> virtual microphone; independent source/rack removed on stop and unplug")
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
        env = os.environ | {"MASSIVEEQ_TEST_PW_JACK": "1"}
        os.execvpe("pw-jack", ["pw-jack", sys.executable, __file__], env)
    main()
