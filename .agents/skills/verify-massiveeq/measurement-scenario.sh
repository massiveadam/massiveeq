# Native GTK measurement imports in an isolated massiveeq-shot session.
export PYTHONDONTWRITEBYTECODE=1
merge_helpers=$(dirname "$(realpath "${BASH_SOURCE[0]}")")
prepare() {
 mkdir -p "$XDG_RUNTIME_DIR/measurement-qa"
 mkdir -p "$XDG_RUNTIME_DIR/imports" "$XDG_RUNTIME_DIR/invalid"
 printf "not a measurement\n" >"$XDG_RUNTIME_DIR/invalid/broken.csv"
 python3 - "$XDG_RUNTIME_DIR/imports" <<'PY'
import math,sys,pathlib
root=pathlib.Path(sys.argv[1])
for name,base,shift in [('take_L_1.csv',80,4),('take_L_2.csv',84,4),('take_R_1.csv',84,-2),('placement2.csv',88,-2)]:
 with (root/name).open('w') as file:
  file.write('frequency,raw\n')
  for i in range(200):
   f=20*1000**(i/199)
   y=base+shift*math.exp(-(math.log2(f/2800)/.7)**2)-3/(1+(f/90)**2)
   file.write(f'{f:.6f},{y:.6f}\n')
PY
}
scenario() {
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 ui click "Add measurement files…" button; sleep 0.5
 ui select "Open your personal folder"
 sleep 0.4
 wtype -s 1000 -M ctrl -k l -m ctrl -s 400 -- "$XDG_RUNTIME_DIR/imports/"
 wtype -s 800 -k Return
 sleep 0.6
 python3 "$merge_helpers/measurement-ui.py" files
 ui click "Add measurements" button
 sleep 1
 ui dump >"$XDG_RUNTIME_DIR/measurement-qa/imported-tree.txt"
 shot merge-imported.png
 python3 - "$merge_helpers" <<'CHECK'
import sys
sys.path.insert(0,sys.argv[1]);import ui
assert not ui.find('Use measurements','button').get_state_set().contains(ui.Atspi.StateType.SENSITIVE)
print('PASS: ambiguous filename needs an ear before Apply')
CHECK
 python3 "$merge_helpers/measurement-ui.py" ear placement2.csv 2
 shot merge-separate-preview.png
 ui click "Use measurements" button
 sleep 1
 local id
 id=$(meq ListProfiles | jq -r '.[0].id')
 meq GetCurves "$id" > "$XDG_RUNTIME_DIR/measurement-qa/saved-separate.json"
 shot merge-separate-graph.png
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 python3 "$merge_helpers/measurement-ui.py" mode merged
 shot merge-merged-preview.png
 ui click "Use measurements" button
 sleep 1
 meq GetCurves "$id" > "$XDG_RUNTIME_DIR/measurement-qa/saved-merged.json"
 shot merge-merged-graph.png
 jq -e '.measurement_set.output == "merged" and (.measurement_set.takes | length) == 4 and .measurement_channels == null' "$XDG_RUNTIME_DIR/measurement-qa/saved-merged.json"
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 ui click "Remove take_L_1.csv" button; sleep 0.2
 shot merge-remove-draft.png
 ui click "Cancel" button; sleep 0.2
 meq GetCurves "$id" > "$XDG_RUNTIME_DIR/measurement-qa/after-cancel.json"
 cmp "$XDG_RUNTIME_DIR/measurement-qa/saved-merged.json" "$XDG_RUNTIME_DIR/measurement-qa/after-cancel.json"
 echo 'PASS: cancelling a removed take preserves all stored sources'
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 ui click "Add measurement files…" button; sleep 0.5
 wtype -s 800 -M ctrl -k l -m ctrl -s 300 -- "$XDG_RUNTIME_DIR/invalid/broken.csv"
 wtype -s 600 -k Return
 sleep 0.8
 ui dump | grep 'Expected at least 10'
 shot merge-invalid-file.png
 ui click "Cancel" button; sleep 0.2
 meq GetCurves "$id" > "$XDG_RUNTIME_DIR/measurement-qa/after-invalid.json"
 cmp "$XDG_RUNTIME_DIR/measurement-qa/saved-merged.json" "$XDG_RUNTIME_DIR/measurement-qa/after-invalid.json"
 echo 'PASS: invalid file reports its error and retains stored measurements'
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 python3 "$merge_helpers/measurement-ui.py" mode separate
 python3 "$merge_helpers/measurement-ui.py" match
 ui click "Use measurements" button; sleep 1
 meq GetCurves "$id" > "$XDG_RUNTIME_DIR/measurement-qa/saved-matched.json"
 jq -e '.measurement_set.align_takes and .measurement_set.output == "separate" and (.measurement_set.takes | length) == 4 and .measurement_channels != null' "$XDG_RUNTIME_DIR/measurement-qa/saved-matched.json"
 echo 'PASS: reopening restores takes and can switch output or match levels'
 ui click "CURVES" "toggle button"; sleep 0.3
 ui click "Measurements…" button; sleep 0.3
 python3 "$merge_helpers/measurement-ui.py" narrow "$(dirname "$out")/merge-narrow.png"
 ui click "Cancel" button
 if grep -E 'Gtk-CRITICAL|Adwaita-WARNING|panicked' "$log"; then return 1; fi
 echo 'PASS: four-file picker import, L/R assignment, separate/merged previews, save, cancel and narrow layout'
}
