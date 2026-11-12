import importlib.machinery
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

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
            path.write_text(json.dumps(config))
            project.write_text('<CARLA-PROJECT><Plugin/><Patchbay/></CARLA-PROJECT>')
            with self.assertRaises(ValueError):
                mic.load_config(path)


if __name__ == "__main__":
    unittest.main()
