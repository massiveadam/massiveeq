import sys,time,subprocess
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parent))
import ui
if sys.argv[1]=='files':
 rows=[n for n,_ in ui.walk(ui.app()) if n.get_role_name()=='table row' and '.csv' in ui.label(n)]
 assert len(rows)==4,len(rows)
 parent=rows[0].get_parent()
 try: selection=parent.get_selection_iface()
 except Exception: selection=None
 if selection:
  assert selection.select_all()
 else:
  assert rows[0].get_component_iface().grab_focus()
  subprocess.run(['wtype','-s','600','-M','ctrl','-k','a','-m','ctrl'],check=True)
 time.sleep(.2)
 print('Selected',sum(n.get_state_set().contains(ui.Atspi.StateType.SELECTED) for n in rows),'files',flush=True)
elif sys.argv[1]=='ear':
 label=ui.find(sys.argv[2],'label');row=label.get_parent()
 combo=next(n for n,_ in ui.walk(row) if n.get_role_name()=='combo box')
 button=next(n for n,_ in ui.walk(combo) if n.get_role_name()=='toggle button')
 assert button.get_action_iface().do_action(0)
 time.sleep(.2)
 subprocess.run(['wtype','-s','600','-k','Home']+['-k','Down']*int(sys.argv[3])+['-k','Return'],check=True)
 time.sleep(.3)
 print('Ear chosen',combo.get_name(),flush=True)
elif sys.argv[1]=='mode':
 frame=ui.find('Measurements','frame')
 combo=next(n for n,_ in ui.walk(frame) if n.get_role_name()=='combo box')
 button=next(n for n,_ in ui.walk(combo) if n.get_role_name()=='toggle button')
 assert button.get_action_iface().do_action(0)
 time.sleep(.2)
 subprocess.run(['wtype','-s','600','-k','End' if sys.argv[2]=='merged' else 'Home','-k','Return'],check=True)
 time.sleep(.3)
 print('Output chosen',combo.get_name(),flush=True)
elif sys.argv[1]=='match':
 import json,os
 check=ui.find('Match take levels at 630 Hz','check box')
 bounds=check.get_component_iface().get_extents(ui.Atspi.CoordType.WINDOW)
 socket=next(Path(os.environ['XDG_RUNTIME_DIR']).glob('sway-ipc.*.sock'))
 tree=json.loads(subprocess.check_output(['swaymsg','-s',str(socket),'-t','get_tree','-r'],text=True))
 def nodes(node):
  yield node
  for child in node.get('nodes',[])+node.get('floating_nodes',[]):
   yield from nodes(child)
 window=next(node for node in nodes(tree) if node.get('name')=='Measurements')
 rect=window['rect']
 x=rect['x']+bounds.x+60
 y=rect['y']+bounds.y+bounds.height/2
 subprocess.run([sys.executable,str(Path.home()/'Playground/devbox/pointer.py'),f'click {x} {y}'],check=True)
 time.sleep(.3)
 assert check.get_state_set().contains(ui.Atspi.StateType.CHECKED)
 print('Level matching enabled by pointer',flush=True)
elif sys.argv[1]=='narrow':
 import json,os
 socket=next(Path(os.environ['XDG_RUNTIME_DIR']).glob('sway-ipc.*.sock'))
 subprocess.run(['swaymsg','-s',str(socket),'[title="^Measurements$"] resize set width 380 px height 900 px'],check=True,stdout=subprocess.DEVNULL)
 time.sleep(.4)
 tree=json.loads(subprocess.check_output(['swaymsg','-s',str(socket),'-t','get_tree','-r'],text=True))
 def nodes(node):
  yield node
  for child in node.get('nodes',[])+node.get('floating_nodes',[]):
   yield from nodes(child)
 rect=next(node for node in nodes(tree) if node.get('name')=='Measurements')['rect']
 print('Narrow measurement window',rect,flush=True)
 assert rect['width']<=400,rect
 geometry=f"{rect['x']},{rect['y']} {rect['width']}x{rect['height']}"
 subprocess.run(['grim','-g',geometry,sys.argv[2]],check=True)
