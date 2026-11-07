import sys
import os
from urllib.parse import quote
import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, Gtk, GLib

role, action_name = sys.argv[1:]
action = Gdk.DragAction.COPY if action_name != "move" else Gdk.DragAction.MOVE
window = Gtk.Window(title="native-gtk")
window.set_default_size(240, 240)
label = Gtk.Label(label="GTK native peer")
surface = Gtk.EventBox()
surface.add(label)
window.add(surface)
mime = "text/uri-list" if action_name in ("move", "file-copy", "prepared-copy") else "text/plain;charset=utf-8"
if role == "destination-reject-mime":
    mime = "application/x-unsupported"
if role == "destination-reject-action":
    action = Gdk.DragAction.MOVE
temporary_mime = "application/x-enzui-temporary-files-v1"
targets = [Gtk.TargetEntry.new(mime, 0, 0)]
if role == "source" and action_name == "file-copy":
    targets.append(Gtk.TargetEntry.new(temporary_mime, 0, 1))


def report(message):
    print(message, flush=True)


surface.add_events(Gdk.EventMask.ENTER_NOTIFY_MASK | Gdk.EventMask.BUTTON_PRESS_MASK)
surface.connect("button-press-event", lambda *_: report("peer-pressed") or False)
surface.connect("enter-notify-event", lambda *_: report("peer-entered") or False)


def provide(_widget, _context, selection, _info, _time):
    payload = b"file:///tmp/native-from-gtk\r\n" if action_name in ("move", "file-copy", "prepared-copy") else b"native-from-gtk"
    selection.set(selection.get_target(), 8, payload)
    report("peer-data-sent")


def receive(_widget, context, _x, _y, selection, _info, time):
    expected = b"file:///tmp/native-from-enzui\r\n" if action_name in ("move", "file-copy", "prepared-copy") else b"native-from-enzui"
    if action_name == "selection-copy":
        expected = b"nativefromenzui"
    if action_name == "prepared-copy":
        assert temporary_mime in [target.name() for target in context.list_targets()]
        path = os.path.join(os.environ["XDG_RUNTIME_DIR"], "prepared-file")
        expected = ("file://" + quote(path) + "\r\n").encode()
        assert open(path, "rb").read() == b"owned attachment export"
    assert selection.get_data() == expected
    assert context.get_selected_action() == action
    report("peer-received")

    def commit():
        if not os.path.exists(os.path.join(os.environ["XDG_RUNTIME_DIR"], "commit-peer-drop")):
            return True
        Gtk.drag_finish(context, True, action == Gdk.DragAction.MOVE, time)
        report("peer-committed")
        return False

    GLib.timeout_add(10, commit)


if role == "source":
    surface.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, targets, action)
    surface.connect("drag-begin", lambda *_: report("peer-started"))
    surface.connect("drag-data-get", provide)
    surface.connect("drag-failed", lambda *_: report("peer-cancelled") or False)
    surface.connect("drag-end", lambda _w, context: report("peer-ended"))
else:
    surface.drag_dest_set(Gtk.DestDefaults.MOTION | Gtk.DestDefaults.HIGHLIGHT, targets, action)

    def dropped(widget, context, _x, _y, time):
        target = widget.drag_dest_find_target(context, None)
        if target is None or context.get_selected_action() != action:
            return False
        widget.drag_get_data(context, target, time)
        return True

    surface.connect("drag-drop", dropped)
    surface.connect("drag-data-received", receive)

window.connect("destroy", Gtk.main_quit)
window.connect("map-event", lambda *_: report("peer-ready"))
window.show_all()
Gtk.main()
