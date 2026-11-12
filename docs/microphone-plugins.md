# Set up microphone processing

MassiveEQ has separate **Output** and **Input** views. The Output view keeps
your playback profiles and controls. The Input view processes a microphone and
publishes **MassiveEQ Microphone** for calling and recording applications.
Each view has its own profiles and power switch.

## Set up an input profile

1. Open MassiveEQ and select **Input**.
2. If the page shows missing support, select **Install support…**. Open the
   command in a terminal, or copy it to your terminal:

   ```sh
   sudo pacman -S --needed carla pipewire-jack python lsp-plugins-lv2 noise-suppression-for-voice
   ```

   Authentication happens in your terminal. Return to MassiveEQ and select the
   refresh button beside the microphone picker after installation.
3. Choose your physical microphone. Mono and stereo sources are supported.
   A disconnected source stays visible in its saved profile.
4. Select **Add band**, or double-click the graph, to add EQ. Drag a band to
   change frequency and gain, scroll to change Q, or use its knobs. Double-click
   a numeric readout to type a value. Peak, low shelf, high shelf, high pass,
   and low pass filters are available, up to 16 bands per profile.
5. Expand **Voice processing** to enable RNNoise, the noise gate, or compressor.
   Set gate threshold above room noise and below speech. Adjust compressor
   threshold, ratio, attack, release, and makeup gain while checking your
   calling application's input meter.
6. Turn on the Input power switch. Wait for **Input on**, then select
   **MassiveEQ Microphone** in your calling or recording app.

Edits save automatically and apply to a running input chain. The graph shows
EQ only. Voice processing follows this order: RNNoise, gate, compressor, EQ.
Microphone EQ does not apply playback loudness matching or automatic gain
correction. Allow headroom when boosting EQ or compressor makeup gain.

RNNoise is optional. EQ, gate, and compressor need `lsp-plugins-lv2` and the
Carla host, but can run without `noise-suppression-for-voice` installed.
The application configures these effects directly. Normal setup does not
require the Carla window or a hand-edited configuration file.

## Manage input profiles

Use the Input profile picker to switch profiles. The name field renames the
current profile. The adjacent buttons create, duplicate, or delete a profile.
Each profile keeps its microphone, EQ, and voice-processing settings.
Undo and redo operate on input edits while the Input view is visible.
Playback profiles and their undo history remain separate.

The Input power switch controls `massiveeq-input.service`. Turning it on also
enables it at login. Turning it off stops the service and disables login
startup. Closing the editor keeps a running input chain active.
The Output power switch has no effect on microphone processing.

Normal parameter edits, profile changes with the same channel map, and
RNNoise toggles retain the published virtual microphone. Switching between
mono and stereo recreates that endpoint. Applications may need to select it
again after a channel-map change.

## Use external plugins

Expand **Advanced plugins** and select **Use custom Carla rack…** to load an
existing `.carxp` file. A custom rack replaces that input profile's built-in
EQ and voice chain. The disabled controls and rack label show which mode is
active. Select **Use built-in processing** to return to the app's controls.

Create a custom rack with `pw-jack carla-rack`. Add native Linux VST3 or LV2
effects, enable every effect, and save a **Continuous Rack** project. Close
Carla before starting the managed service. Patchbay projects are rejected
because their connections do not describe a serial rack. Saved external
hardware connections are ignored.

Some Carla versions cannot select arbitrary classes in a multi-plugin VST3
bundle. MassiveEQ checks the loaded plugin identity and reports an error if
the host selects a different effect. Use a compatible single-class VST3 build
or an LV2 alternative. Windows DLLs and Wine bridges are not supported by this
integration. Plugins own their latency and real-time behavior.

## Diagnose an input error

The Input page reports missing dependencies, disconnected microphones,
unsupported settings, and plugin load errors. Select the microphone refresh
button after reconnecting a device or installing support. For detailed logs:

```sh
systemctl --user status massiveeq-input.service
journalctl --user -u massiveeq-input.service -b
massiveeq-input doctor
massiveeq-input sources
massiveeq-input check
```

The helper removes its virtual source when stopped or when its microphone is
removed. Your desktop or recording application may then choose another source.
The helper never changes the system's default source or connects a microphone
to speakers. Systemd retries failed input chains independently of playback.

Profiles are stored in `~/.config/massiveeq/input.json`. The earlier
`source`/`project`/`channels` format remains supported. Opening an older config
shows its rack in Advanced mode and preserves the original file until you
edit it. Invalid configs are reported and are not overwritten.

## Verify the processing path

Run configuration, routing, and parameter mapping checks:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s tests -p test_microphone.py -v
```

With the optional host and effects installed, an X display available, and
PipeWire/JACK tools in PATH, run the synthetic signal tests:

```sh
python3 tests/native_managed_microphone.py
python3 tests/native_microphone.py
```

The tests use a private PipeWire session without audio hardware. They measure
EQ gain, gate attenuation, compressor makeup gain, saved custom-plugin state,
live updates, virtual-source identity, and stop/unplug cleanup. RNNoise testing
checks processing and finite audio, not voice quality on a real microphone.
See [the input architecture decision](input-workspace.md) for ownership and
compatibility details.
