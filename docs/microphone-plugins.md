# Microphone plugins

MassiveEQ can run an independent microphone chain through Carla's native Linux
VST3 and LV2 host. The chain publishes **MassiveEQ Microphone** as a PipeWire
recording source. Select that source in your calling or recording application.
Playback profiles, the playback Engine switch, and microphone processing have
separate controls and service lifetimes.

The first version uses Carla to edit the plugin rack and save plugin settings.
It does not embed plugin editors into MassiveEQ's GTK window. Native Linux
plugins are supported; Windows VST DLLs, Wine bridges, and standalone RNNoise
executables are outside this integration.

## Set up a rack

Install the optional backend and the plugins you want to use:

```sh
sudo pacman -S carla pipewire-jack python
```

Open `pw-jack carla-rack`. Add native Linux VST3 or LV2 effects in processing
order. For speech, a useful starting order is an RNNoise plugin, noise gate,
then compressor. Install those plugins separately: MassiveEQ does not bundle
third-party plugin binaries or RNNoise models. RNNoise can be an LV2 plugin in
the same rack as your VST3 compressor or gate. Use the plugin browser's format
and architecture indicators to choose installed native Linux effects.

Set the gate threshold above your room noise and below speech; adjust compressor
threshold, ratio, and makeup gain while monitoring the plugin's meters. Save
an active **Continuous Rack** project as
`~/.config/massiveeq/microphone.carxp`. All rack plugins must be active; a
missing, inactive, or mismatched plugin causes startup to fail instead of publishing a
partially processed chain. Do not save a Patchbay project: its connections do
not describe the serial rack this helper runs. Close Carla before starting the
managed chain. Saved external hardware connections are ignored by the helper.

List microphone node names:

```sh
massiveeq-input sources
```

Copy `/usr/share/massiveeq/input/input.json` to
`~/.config/massiveeq/input.json`, then replace `source` with the exact physical
microphone's `node.name` and `project` with the absolute path to your project.
For example:

```json
{
  "source": "alsa_input.usb-My_Microphone-00.mono-fallback",
  "project": "/home/alex/.config/massiveeq/microphone.carxp",
  "channels": ["MONO"]
}
```

Use `["FL", "FR"]` for a stereo microphone. `pw-dump` shows each source port's
`audio.channel`; the helper requires exact channel matches. A mono source feeds
both sides of Carla's stereo rack; its first output becomes the mono recording
source, without summing the two sides. Stereo sources retain left/right order.
This deliberately supports mono and stereo capture, not arbitrary channel maps.

Validate the configuration and start the separate service:

```sh
massiveeq-input check
systemctl --user enable --now massiveeq-input.service
journalctl --user -u massiveeq-input.service -f
```

Wait for **MassiveEQ Microphone ready** in the journal, then select **MassiveEQ
Microphone** in your application. The helper never calls `set-default` or
changes your playback routing. Desktop policy still determines which source a
new application chooses; explicitly choose the processed source in each app.

For a foreground check instead of the service:

```sh
pw-jack massiveeq-input run
```

## Control and troubleshoot

```sh
systemctl --user status massiveeq-input.service
systemctl --user stop massiveeq-input.service
systemctl --user restart massiveeq-input.service
```

Stopping removes the virtual microphone. The helper does not select a replacement;
your desktop or recording application may fall back to another source. A microphone unplug, stopped
plugin engine, missing required port, or virtual-source process failure stops
the chain and reports an error. Systemd retries every three seconds, so a
reconnected microphone can return without changing playback. A manual
foreground run exits on those failures; run it again after reconnecting.

To change settings, stop the microphone service, edit and save the rack in
Carla, close Carla, then start the service. There is no live crossfade or
hot-swap of microphone projects in this version. Use your calling application's
input test to check levels after changing a rack. Plugins own their processing
latency and real-time behavior; this host cannot enforce third-party RT safety.

Some Carla versions cannot select arbitrary classes in multi-plugin VST3
bundles. The helper compares loaded plugin labels with the saved project and
refuses a different class instead of silently running the wrong effect. Use a
Carla-compatible single-class VST3 build or LV2 alternative if this occurs.

If startup fails, check the journal for the exact missing source/port or Carla
error. Check `massiveeq-input sources`, absolute paths, plugin installation,
and the `channels` setting. The source is published at low session priority;
the helper uses no default-source mutation and no microphone-to-speaker link.

## Architecture decision

Two shapes were considered:

1. Add a VST3 COM host, plugin state/parameter model, scanner, editor integration,
   and input direction to the playback daemon. The GTK editor would own the
   chain UI; a direction-tagged engine would own capture nodes and state. This
   permits a unified editor but expands the real-time callback and daemon crash
   surface, and requires a separate plugin sandbox and migration of the
   playback-centric device and profile model. Verification would require
   capture routing plus plugin ABI, state, GUI, and crash tests.
2. Keep the current playback daemon and profile format intact. A separate
   `massiveeq-input` process owns a Carla serial rack and a PipeWire virtual
   source. Carla projects own plugin state; `input.json` owns physical source,
   project path, and channel map. Systemd owns retries and process-group
   cleanup. This gives an explicit independent lifetime and isolates plugin
   failure from playback, with no library migration. The caller configures a
   rack in Carla and starts/stops one independent service.

The second shape is implemented. The helper uses Carla's standalone C API
(`carla_standalone_host_init`, engine options, project loading, engine idle and
close); Python runs only control work. Audio processing runs inside Carla's
native JACK callback through PipeWire's JACK library. `CARLA_DONT_MANAGE_CONNECTIONS`
suppresses restoration of the project's external connections; the helper alone
links the configured source to the rack, then the rack to the virtual source.
Nodes have automatic connections disabled. Required port resolution is exact
and checked periodically. Missing devices fail closed, and each restart
re-resolves IDs instead of persisting transient PipeWire IDs. A runtime lock
prevents two microphone helpers from competing for the same node names.

The helper validates configuration before starting audio, refuses empty,
partial, or inactive racks, and removes the published source before closing
Carla during normal teardown. Systemd's `KillMode=control-group` also removes
its child process after a native plugin crash. Prefer the service for normal
use; a native crash in a foreground run cannot execute Python cleanup.

Upstream contracts: [Carla standalone bindings](https://github.com/falkTX/Carla/blob/main/source/frontend/carla_backend.py),
[Carla project connection restoration](https://github.com/falkTX/Carla/blob/main/source/backend/engine/CarlaEngine.cpp),
and [PipeWire loopback virtual sources](https://docs.pipewire.org/page_module_loopback.html).

Pure configuration/routing checks run with:

```sh
python3 -m unittest discover -s tests -p test_microphone.py -v
```

An opt-in signal test runs a real installed Linux VST3 plugin through a private
PipeWire session, measures its saved gain change, and verifies teardown:

```sh
python3 tests/native_microphone.py
```

It requires Carla, PipeWire/JACK, WirePlumber, `lsp-plugins-vst3`, and an X display
(Xvfb works). It creates only synthetic audio nodes and uses no hardware or
existing audio session. The fixture uses the first class in LSP's multi-class
bundle, an A/B tester with input gain, to accommodate Carla 2.5's class-selection
limit. It verifies processing and state restoration; microphone hardware,
compressor/gate tuning, and RNNoise quality still need device testing.
