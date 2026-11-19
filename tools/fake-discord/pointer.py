#!/usr/bin/env python3
"""A real pointer and keyboard for the headless compositor, through mutter's
RemoteDesktop D-Bus API (inside the session). `pointer.py serve FIFO` keeps
one remote-desktop session open and does what lines written to the FIFO say:
`move X Y`, `press`, `release`, `click X Y`, `scroll N` (notches, negative up)
and `key NAME` (`Escape`, `Return`, `Tab`, `BackSpace` or a character).
X and Y are window coordinates of the 1280x900 virtual monitor: the pointer
goes to the corner first and moves from there, so where it was doesn't
matter. One session for the whole run, because events sent right after one
is started or stopped get lost."""
import os
import sys
import time

from gi.repository import Gio, GLib

BUS = Gio.bus_get_sync(Gio.BusType.SESSION)
BUTTON_LEFT = 0x110
KEYS = {"Escape": 0xFF1B, "Return": 0xFF0D, "Tab": 0xFF09, "BackSpace": 0xFF08}


def call(path, interface, method, args=None, reply="()"):
    return BUS.call_sync("org.gnome.Mutter.RemoteDesktop", path, interface, method, args, GLib.VariantType(reply),
                         Gio.DBusCallFlags.NONE, -1)


class Remote:
    interface = "org.gnome.Mutter.RemoteDesktop.Session"

    def __init__(self):
        created = call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop", "CreateSession",
                       reply="(o)")
        self.session = created.unpack()[0]
        call(self.session, self.interface, "Start")
        time.sleep(0.5)
        # The first key sent is lost, as the compositor makes its keyboard for it.
        for down in (True, False):
            self.notify("NotifyKeyboardKeysym", "(ub)", 0xFFE1, down)
        time.sleep(0.3)

    def notify(self, method, signature, *values):
        call(self.session, self.interface, method, GLib.Variant(signature, values))

    def motion(self, dx, dy):
        self.notify("NotifyPointerMotionRelative", "(dd)", float(dx), float(dy))

    def button(self, down):
        self.notify("NotifyPointerButton", "(ib)", BUTTON_LEFT, down)

    def move(self, x, y):
        for _ in range(4):
            self.motion(-2000, -2000)
        for _ in range(int(x) // 50):
            self.motion(50, 0)
        self.motion(int(x) % 50, 0)
        for _ in range(int(y) // 50):
            self.motion(0, 50)
        self.motion(0, int(y) % 50)
        time.sleep(0.3)

    def run(self, words):
        command, args = words[0], words[1:]
        if command == "move":
            self.move(*args)
        elif command == "press":
            self.button(True)
        elif command == "release":
            self.button(False)
        elif command == "click":
            self.move(*args)
            self.button(True)
            time.sleep(0.08)
            self.button(False)
        elif command == "key":
            keysym = KEYS.get(args[0]) or ord(args[0])
            for down in (True, False):
                self.notify("NotifyKeyboardKeysym", "(ub)", keysym, down)
        elif command == "scroll":
            for _ in range(abs(int(args[0]))):
                self.notify("NotifyPointerAxisDiscrete", "(ui)", 0, 1 if int(args[0]) > 0 else -1)


def serve(fifo):
    remote = Remote()
    os.mkfifo(fifo)
    while True:
        with open(fifo) as lines:
            for line in lines:
                words = line.split()
                if words == ["quit"]:
                    return
                if words:
                    remote.run(words)


if len(sys.argv) == 3 and sys.argv[1] == "serve":
    serve(sys.argv[2])
