#!/usr/bin/env python3
"""Real GTK input workspace persistence and playback isolation checks.

Run inside input-workspace-scenario.sh, never against normal profile storage.
Needs the existing devbox UI helper and wtype. Does not start the real input service.
"""
import json
import math
import os
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, '/home/adam/Playground/devbox')
import ui
import pointer
from gi.repository import Atspi


def click(name, role=None):
    if role == 'check box':
        node = ui.find(name, role)
        label_node = next(child for child, _ in ui.walk(node) if child.get_role_name() == 'label')
        rect = label_node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
        pointer.main([f'click {rect.x+20} {rect.y+rect.height//2+16}'])
    else:
        ui.main(['click', name] + ([role] if role else []))
    time.sleep(.3)


def saved():
    time.sleep(.5)
    library = json.loads((Path(os.environ['XDG_CONFIG_HOME']) / 'massiveeq/input.json').read_text())
    return library, next(p for p in library['profiles'] if p['id'] == library['active_profile'])


def choose_microphone():
    click('Choose a microphone', 'toggle button')
    subprocess.run(['wtype', '-s', '600', '-k', 'Down', '-s', '100', '-k', 'Return'], check=True)
    time.sleep(.5)


def playback():
    output = subprocess.check_output(['busctl', '--user', '--json=short', 'call',
        'org.massiveeq.Service1', '/org/massiveeq/Service1', 'org.massiveeq.Service1', 'ListProfiles'])
    return json.loads(json.loads(output)['data'][0])


before = playback()
click('Input', 'page tab')
choose_microphone()
_, profile = saved()
assert profile['source'] == 'input-fixture-mic', profile
assert profile['channels'] == ['MONO'], profile
click('Add band', 'button')
# Locate the graph's leaf panel by its geometry, then use a real pointer drag.
panels = []
for node, _ in ui.walk(ui.app()):
    if node.get_role_name() == 'panel' and node.get_child_count() == 0:
        try:
            rect = node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)
            if rect.width > 600 and rect.height > 200:
                panels.append(rect)
        except Exception:
            pass
assert panels, 'No visible microphone graph'
rect = min(panels, key=lambda r: r.width*r.height)
x = rect.x + 58 + math.log(1000/20)/math.log(1000)*(rect.width-82)
y = rect.y + 34 + (rect.height-76)/2
pointer.main([f'drag {x} {y} {x+50} {y-25}'])
_, dragged = saved()
assert dragged['filters'][0]['frequency'] > 1100, dragged
assert dragged['filters'][0]['gain_db'] > .5, dragged
click('Undo input edit', 'button')
assert saved()[1]['filters'][0]['frequency'] == 1000
assert saved()[1]['filters'][0]['gain_db'] == 0
click('1000 Hz', 'toggle button')
ui.main(['setnamedvalue', 'Input band Frequency', '623.123456'])
ui.main(['setnamedvalue', 'Input band Gain', '-7.25'])
ui.main(['setnamedvalue', 'Input band Q', '.72913'])
_, profile = saved()
assert profile['filters'][0]['frequency'] == 623.123456, profile
assert profile['filters'][0]['gain_db'] == -7.25, profile
assert profile['filters'][0]['q'] == .72913, profile
click('Undo input edit', 'button')
assert saved()[1]['filters'][0]['q'] == 1.0
click('Redo input edit', 'button')
assert saved()[1]['filters'][0]['q'] == .72913
click('Voice processing', 'button')
click('RNNoise noise reduction', 'check box')
click('Noise gate', 'check box')
ui.main(['setnamedvalue', 'Gate threshold dB', '-38.5'])
click('Compressor', 'check box')
ui.main(['setnamedvalue', 'Compressor ratio', '4'])
_, profile = saved()
assert profile['rnnoise']['enabled'] and profile['gate']['enabled'] and profile['compressor']['enabled']
assert profile['gate']['threshold_db'] == -38.5 and profile['compressor']['ratio'] == 4
old_id = profile['id']
click('Duplicate input profile', 'button')
library, profile = saved()
assert len(library['profiles']) == 2 and profile['id'] != old_id and profile['filters'][0]['gain_db'] == -7.25
click('New input profile', 'button')
library, profile = saved()
assert len(library['profiles']) == 3 and not profile['filters'] and profile['source'] == 'input-fixture-mic'
click('Delete input profile', 'button')
assert len(saved()[0]['profiles']) == 2
click('Output', 'page tab')
assert playback() == before, 'microphone edits changed playback profiles'
click('Input', 'page tab')
assert saved()[1]['filters'][0]['gain_db'] == -7.25
ui.main(['settext', 'Input profile name', 'My voice', 'text'])
time.sleep(.5)
assert saved()[1]['name'] == 'My voice'
subprocess.run(['grim', sys.argv[1]], check=True)
# Close immediately after an edit: the app must flush rather than discard it.
ui.main(['setnamedvalue', 'Compressor ratio', '5'])
click('Close', 'button')
assert saved()[1]['compressor']['ratio'] == 5
print('PASS Input GTK: source, EQ, voice controls, undo/redo, profile lifecycle, view isolation, close-time save')
