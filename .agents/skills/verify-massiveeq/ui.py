#!/usr/bin/env python3
"""Drive the MassiveEQ editor through AT-SPI inside a massiveeq-shot session.

    ui.py dump                      print the accessible tree (role, name)
    ui.py click <name> [role]       activate the first widget whose name matches
    ui.py select <name>             select the first list row/item whose name matches
    ui.py settext <name> <text> [role]  replace the text of the first editable widget that matches
    ui.py setnamedvalue <name> <value>  set a named slider or spin button
    ui.py setvalue <role> <n> <value>   set the value of the n-th (0-based) widget with that role

Names match case-insensitively by substring. Exits non-zero if nothing matched.
"""
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402

APP = "massiveeq"


def app():
    desktop = Atspi.get_desktop(0)
    for i in range(desktop.get_child_count()):
        child = desktop.get_child_at_index(i)
        if child and APP in (child.get_name() or "").lower():
            return child
    sys.exit(f"{APP} is not on the accessibility bus")


def walk(node, depth=0):
    yield node, depth
    for i in range(node.get_child_count()):
        child = node.get_child_at_index(i)
        if child is not None:
            yield from walk(child, depth + 1)


def label(node):
    name = node.get_name() or ""
    if not name and "button" in node.get_role_name() and node.get_child_count():
        # Buttons often carry their text on a child label.
        name = " ".join(filter(None, (child.get_name() for child, _ in walk(node) if child is not node)))
    return name


def find(name, role=None):
    needle = name.lower()
    matches = [
        node
        for node, _ in walk(app())
        if (not role or node.get_role_name() == role) and needle in label(node).lower()
    ]
    if not matches:
        sys.exit(f"no widget named {name!r}" + (f" with role {role!r}" if role else ""))
    # Containers aggregate their children's text; prefer the widget itself.
    exact = [node for node in matches if label(node).lower() == needle]
    return (exact or sorted(matches, key=lambda node: len(label(node))))[0]


def main(argv):
    if not argv or argv[0] == "dump":
        for node, depth in walk(app()):
            states = node.get_state_set()
            checked = states.contains(Atspi.StateType.CHECKED) or states.contains(Atspi.StateType.PRESSED)
            print(f"{'  ' * depth}{node.get_role_name()}: {label(node)!r}{' [checked]' if checked else ''}")
    elif argv[0] == "click":
        node = find(argv[1], argv[2] if len(argv) > 2 else None)
        actions = node.get_action_iface()
        if actions is None or actions.get_n_actions() == 0:
            sys.exit(f"{label(node)!r} has no actions")
        actions.do_action(0)
    elif argv[0] == "setnamedvalue":
        needle = argv[1].lower()
        matches = [node for node, _ in walk(app()) if node.get_role_name() in ("slider", "spin button") and needle in label(node).lower()]
        if not matches:
            sys.exit(f"no numeric widget named {argv[1]!r}")
        exact = [node for node in matches if label(node).lower() == needle]
        node = (exact or sorted(matches, key=lambda node: len(label(node))))[0]
        value = node.get_value_iface()
        if value is None:
            sys.exit(f"{label(node)!r} has no numeric value")
        value.set_current_value(float(argv[2]))
    elif argv[0] == "select":
        node = find(argv[1], "list item")
        parent = node.get_parent()
        selection = parent.get_selection_iface() if parent else None
        if selection is None:
            sys.exit(f"{label(node)!r} is not in a selectable list")
        selection.select_child(node.get_index_in_parent())
    elif argv[0] == "settext":
        node = find(argv[1], argv[3] if len(argv) > 3 else None)
        editable = node.get_editable_text_iface()
        if editable is None:
            sys.exit(f"{label(node)!r} is not editable")
        editable.set_text_contents(argv[2])
    elif argv[0] == "setvalue":
        matches = [node for node, _ in walk(app()) if node.get_role_name() == argv[1]]
        if int(argv[2]) >= len(matches):
            sys.exit(f"only {len(matches)} widgets with role {argv[1]!r}")
        value = matches[int(argv[2])].get_value_iface()
        if value is None:
            sys.exit("that widget has no value")
        value.set_current_value(float(argv[3]))
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main(sys.argv[1:])
