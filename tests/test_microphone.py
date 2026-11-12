import importlib.machinery
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch, Mock

loader = importlib.machinery.SourceFileLoader("microphone", str(Path(__file__).parents[1] / "packaging/massiveeq-input"))
spec = importlib.util.spec_from_loader(loader.name, loader)
mic = importlib.util.module_from_spec(spec)
loader.exec_module(mic)


def node(id_, name, cls="Audio/Source"):
    return {"id": id_, "type": "PipeWire:Interface:Node", "info": {"props": {"node.name": name, "media.class": cls}}}


def port(id_, node_id, direction, name, channel):
    return {"id": id_, "type": "PipeWire:Interface:Port", "info": {"props": {
        "node.id": node_id, "port.direction": direction, "port.name": name, "audio.channel": channel}}}


def mono_graph():
    return [node(1, "mic"), port(2, 1, "out", "capture_MONO", "MONO"),
            node(3, mic.CLIENT, "Stream/Filter/Audio"),
            port(4, 3, "in", "audio-in1", "FL"), port(5, 3, "in", "audio-in2", "FR"),
            port(6, 3, "out", "audio-out1", "FL"), port(7, 3, "out", "audio-out2", "FR"),
            node(8, mic.CAPTURE, "Stream/Input/Audio"), port(9, 8, "in", "input_MONO", "MONO")]


class MicrophoneTests(unittest.TestCase):
    def test_mono_duplicates_input_without_summing_output(self):
        self.assertEqual(mic.desired_links(mono_graph(), {"source": "mic", "channels": ["MONO"]}),
                         [(2, 4), (2, 5), (6, 9)])

    def test_disconnected_mic_and_wrong_media_class_fail_closed(self):
        objects = mono_graph()
        with self.assertRaises(RuntimeError):
            mic.desired_links(objects[1:], {"source": "mic", "channels": ["MONO"]})
        objects[0]["info"]["props"]["media.class"] = "Audio/Sink"
        with self.assertRaises(RuntimeError):
            mic.desired_links(objects, {"source": "mic", "channels": ["MONO"]})

    def test_stereo_channels_are_not_interchanged(self):
        objects = mono_graph()
        objects[1] = port(2, 1, "out", "capture_FR", "FR")
        objects += [port(10, 1, "out", "capture_FL", "FL"), port(11, 8, "in", "input_FR", "FR")]
        objects[-3]["info"]["props"]["audio.channel"] = "FL"
        self.assertEqual(mic.desired_links(objects, {"source": "mic", "channels": ["FL", "FR"]}),
                         [(10, 4), (6, 9), (2, 5), (7, 11)])

    def test_ambiguous_or_missing_ports_fail(self):
        objects = mono_graph()
        objects.append(port(99, 1, "out", "another", "MONO"))
        with self.assertRaises(RuntimeError):
            mic.desired_links(objects, {"source": "mic", "channels": ["MONO"]})

    def test_reconcile_is_idempotent_and_repairs_a_removed_link(self):
        objects = mono_graph()
        for output, input_ in [(2, 4), (2, 5)]:
            objects.append({"type": "PipeWire:Interface:Link", "info": {
                "output-port-id": output, "input-port-id": input_}})
        with patch.object(mic.subprocess, "run") as run:
            mic.reconcile(objects, {"source": "mic", "channels": ["MONO"]})
            run.assert_called_once_with(["pw-link", "6", "9"], check=True, timeout=5)

    def test_config_validates_before_starting_audio(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / "mic.carxp"
            project.write_text('<CARLA-PROJECT><Plugin><Info><Type>VST3</Type></Info></Plugin></CARLA-PROJECT>')
            path = Path(temp) / "input.json"
            config = {"source": "mic", "project": str(project), "channels": ["MONO"]}
            path.write_text(json.dumps(config))
            self.assertEqual(mic.load_config(path), config)
            for field, value in [("source", "massiveeq-input"), ("channels", ["FR"]), ("project", "relative.carxp")]:
                path.write_text(json.dumps(config | {field: value}))
                with self.assertRaises(ValueError):
                    mic.load_config(path)
            for invalid in [None, [], 12, "input"]:
                path.write_text(json.dumps(invalid))
                with self.assertRaises(ValueError):
                    mic.load_config(path)
            path.write_text(json.dumps(config | {"project": 12}))
            with self.assertRaises(ValueError):
                mic.load_config(path)
            path.write_text(json.dumps(config))
            project.write_text('<CARLA-PROJECT><Plugin/><Patchbay/></CARLA-PROJECT>')
            with self.assertRaises(ValueError):
                mic.load_config(path)

    def test_runtime_rejects_partial_wrong_identity_and_inactive_plugins(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / "mic.carxp"
            project.write_text('<CARLA-PROJECT><Plugin><Info><Label>Expected</Label></Info></Plugin></CARLA-PROJECT>')
            host = mic.Carla.__new__(mic.Carla)
            host.handle = 1
            host.started = False
            info = mic.PluginInfo()
            info.label = b"Expected"
            host.lib = Mock()
            host.lib.carla_engine_init.return_value = True
            host.lib.carla_load_project.return_value = True
            host.lib.carla_get_current_plugin_count.return_value = 1
            host.lib.carla_get_plugin_info.return_value = mic.C.pointer(info)
            host.lib.carla_get_internal_parameter_value.return_value = 1.0
            host.start(project)
            project.write_text('<CARLA-PROJECT><Plugin><Info><Type>LV2</Type><URI>Expected</URI></Info></Plugin></CARLA-PROJECT>')
            host.start(project)
            host.lib.carla_get_current_plugin_count.return_value = 0
            with self.assertRaisesRegex(RuntimeError, "partial"):
                host.start(project)
            host.lib.carla_get_current_plugin_count.return_value = 1
            info.label = b"Wrong processor"
            with self.assertRaisesRegex(RuntimeError, "identity mismatch"):
                host.start(project)
            info.label = b"Expected"
            host.lib.carla_get_internal_parameter_value.return_value = 0.0
            with self.assertRaisesRegex(RuntimeError, "inactive"):
                host.start(project)
            host.close()
            host.lib.carla_engine_close.assert_called_once()


    def managed_profile(self):
        return {"id": "mic-profile", "name": "Microphone", "source": "mic", "channels": ["MONO"],
                "eq_enabled": True, "filters": [{"enabled": True, "kind": "Peaking", "frequency": 1000.0,
                "gain_db": -6.0, "q": 1.0, "channels": "All"}], "rnnoise": {"enabled": False},
                "gate": {"enabled": False}, "compressor": {"enabled": False}, "advanced_project": None}

    def test_managed_profiles_normalize_and_select_without_project_files(self):
        with tempfile.TemporaryDirectory() as temp:
            path = Path(temp) / "input.json"
            inactive = self.managed_profile() | {"id": "inactive", "source": ""}
            config = {"version": 1, "active_profile": "mic-profile", "profiles": [inactive, self.managed_profile()]}
            path.write_text(json.dumps(config))
            result = mic.load_config(path)
            self.assertEqual(result["profile_id"], "mic-profile")
            self.assertEqual(result["managed"]["gate"]["threshold_db"], -45.0)
            self.assertNotIn("project", result)
            config["active_profile"] = "inactive"
            path.write_text(json.dumps(config))
            with self.assertRaisesRegex(mic.NoMicrophone, "choose a microphone"):
                mic.load_config(path)
            config["active_profile"] = "missing"
            path.write_text(json.dumps(config))
            with self.assertRaisesRegex(ValueError, "active input profile"):
                mic.load_config(path)

    def test_managed_boundary_rejects_invalid_filter_and_voice_values(self):
        for field, value in [("frequency", float("nan")), ("frequency", 0), ("q", 0),
                             ("gain_db", 25), ("kind", "Unknown"), ("channels", "Left"), ("enabled", 1), ("bandwidth", 1.0)]:
            profile = self.managed_profile()
            profile["filters"][0][field] = value
            with self.assertRaises(ValueError, msg=f"{field}={value}"):
                mic.validate_managed_profile(profile)
        for field, value in [("threshold_db", float("inf")), ("threshold_db", -61), ("ratio", 0), ("attack_ms", True), ("makeup_db", -1)]:
            profile = self.managed_profile()
            profile["compressor"][field] = value
            with self.assertRaises(ValueError, msg=f"{field}={value}"):
                mic.validate_managed_profile(profile)
        for invalid in [[], None, "gate"]:
            with self.assertRaises(ValueError):
                mic.validate_managed_profile(self.managed_profile() | {"gate": invalid})
        profile = self.managed_profile()
        profile["filters"] *= 17
        with self.assertRaisesRegex(ValueError, "16"):
            mic.validate_managed_profile(profile)

    def test_managed_custom_project_replaces_builtins_and_keeps_identity(self):
        with tempfile.TemporaryDirectory() as temp:
            project = Path(temp) / "custom.carxp"
            project.write_text('<CARLA-PROJECT><Plugin><Info><Label>Effect</Label></Info></Plugin></CARLA-PROJECT>')
            path = Path(temp) / "input.json"
            profile = self.managed_profile() | {"advanced_project": str(project)}
            path.write_text(json.dumps({"version": 1, "active_profile": profile["id"], "profiles": [profile]}))
            result = mic.load_config(path)
            self.assertEqual(result["project"], str(project))
            self.assertEqual(result["profile_id"], profile["id"])
            self.assertNotIn("managed", result)

    def test_source_listing_uses_real_port_channels(self):
        objects = mono_graph()
        objects.append(node(22, "unsupported"))
        result = mic.list_sources(objects)
        self.assertEqual(result, [{"name": "mic", "description": "mic", "channels": ["MONO"]},
                                  {"name": "unsupported", "description": "unsupported", "channels": []}])
        objects[1]["info"]["props"]["audio.channel"] = "FR"
        objects.append(port(99, 1, "out", "capture_FL", "FL"))
        self.assertEqual(mic.list_sources(objects)[0]["channels"], ["FL", "FR"])

    def test_runtime_status_is_atomic_and_tracks_owned_state(self):
        with tempfile.TemporaryDirectory() as temp, patch.dict(os.environ, {"XDG_RUNTIME_DIR": temp}):
            config = {"profile_id": "one", "source": "mic", "managed": {}}
            mic.write_status("running", config)
            path = Path(temp) / "massiveeq-input-status.json"
            value = json.loads(path.read_text())
            self.assertEqual((value["state"], value["profile_id"], value["backend"]), ("running", "one", "managed"))
            self.assertEqual(value["pid"], os.getpid())
            mic.write_status("error", config, "disconnected")
            self.assertEqual(json.loads(path.read_text())["error"], "disconnected")
            self.assertEqual(list(Path(temp).glob("*.tmp")), [])

    def test_live_rack_shape_changes_only_for_topology(self):
        profile = mic.validate_managed_profile(self.managed_profile())
        config = {"managed": profile}
        shape = mic.rack_shape(config)
        profile["filters"][0]["gain_db"] = 4.0
        profile["gate"]["enabled"] = True
        self.assertEqual(mic.rack_shape(config), shape)
        profile["rnnoise"]["enabled"] = True
        self.assertNotEqual(mic.rack_shape(config), shape)

    def test_managed_parameter_mapping_uses_linear_gain_and_explicit_eq_mode(self):
        profile = mic.validate_managed_profile(self.managed_profile())
        host = mic.Carla.__new__(mic.Carla)
        host.handle = 1
        host.lib = Mock()
        host.effects = {"gate": (0, {}), "compressor": (1, {}), "eq": (2, {})}
        host.set_controls = Mock()
        host.apply_managed(profile)
        calls = {call.args[0]: call.args[1] for call in host.set_controls.call_args_list}
        self.assertAlmostEqual(calls["gate"]["gt"], 10 ** (-45 / 20))
        self.assertEqual(calls["compressor"]["cr"], 3)
        self.assertAlmostEqual(calls["compressor"]["al"], 10 ** (-18 / 20))
        self.assertEqual(calls["eq"]["ft_0"], 1)
        self.assertEqual(calls["eq"]["fm_0"], 6)
        self.assertAlmostEqual(calls["eq"]["g_0"], 10 ** (-6 / 20))
        self.assertEqual(calls["eq"]["ft_15"], 0)
        profile["eq_enabled"] = False
        host.apply_managed(profile)
        self.assertEqual(host.set_controls.call_args_list[-1].args[1]["ft_0"], 0)


if __name__ == "__main__":
    unittest.main()
