#!/usr/bin/env python3
"""Check both workspace tabs at laptop widths in the private Sway harness."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

sys.path.insert(0, '/home/adam/Playground/devbox')
import ui
from gi.repository import Atspi

os.environ['SWAYSOCK'] = str(next(Path(os.environ['XDG_RUNTIME_DIR']).glob('sway-ipc.*.sock')))


def sway(*args):
    return json.loads(subprocess.check_output(['swaymsg', *args]))


def windows(node):
    if node.get('app_id') == 'org.massiveeq.MassiveEQ':
        yield node
    for child in node.get('nodes', []) + node.get('floating_nodes', []):
        yield from windows(child)


def rect(node):
    return node.get_component_iface().get_extents(Atspi.CoordType.WINDOW)


def overlap(a, b):
    return min(a.x+a.width, b.x+b.width) > max(a.x, b.x) and min(a.y+a.height, b.y+b.height) > max(a.y, b.y)


sway('[app_id="org.massiveeq.MassiveEQ"]', 'floating', 'enable')
for width in [1280, 1100, 1024, 900, 760, 1024, 1280]:
    sway('[app_id="org.massiveeq.MassiveEQ"]', 'resize', 'set', str(width), '768')
    time.sleep(.5)
    frame = next(windows(sway('-t', 'get_tree')))['rect']
    assert frame['width'] <= width, ('window cannot fit laptop width', width, frame)
    for workspace in ['Output', 'Input']:
        ui.main(['click', workspace, 'page tab'])
        time.sleep(.3)
        tabs = [n for n, _ in ui.walk(ui.app()) if n.get_role_name() == 'page tab']
        assert len(tabs) == 2, (width, workspace, [ui.label(n) for n in tabs])
        for tab in tabs:
            r = rect(tab)
            assert r.width >= 45 and r.height >= 20, (width, ui.label(tab), r.width, r.height)
            assert r.x >= 0 and r.x+r.width <= frame['width'], (width, ui.label(tab), r.x, r.width)
            for other, _ in ui.walk(ui.app()):
                if other.get_role_name() == 'combo box':
                    assert not overlap(r, rect(other)), (width, workspace, 'picker overlaps tab')
        state = ui.find(workspace, 'page tab').get_state_set()
        assert state.contains(Atspi.StateType.CHECKED) or state.contains(Atspi.StateType.PRESSED), (width, workspace)
        if workspace == 'Output':
            picker = next(n for n, _ in ui.walk(ui.app()) if n.get_role_name() == 'combo box' and 'MOONDROP' in ui.label(n))
            assert (rect(picker).y > 50) == (width <= 1180), (width, rect(picker).y)
        subprocess.run(['grim', str(Path(sys.argv[1]).with_name(f'navigation-{workspace.lower()}-{width}.png'))], check=True)
    print(f'PASS {width}px: both tabs usable, picker separated, resize transition clean', flush=True)
print('PASS responsive workspace navigation with long profile/device names', flush=True)
