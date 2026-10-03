#!/usr/bin/env python3
"""Asks every top-level window on $DISPLAY to close, as a window manager's close button does:
a WM_DELETE_WINDOW client message to each window that takes one. Emulators that write their
settings only on a clean quit (Cemu, melonDS) need this; a signal just stops them.

There is no window manager under Xvfb, and xdotool's `windowclose` destroys the window instead
of asking, so this talks to Xlib directly through ctypes."""
import ctypes
import ctypes.util
import sys

Window = ctypes.c_ulong
Atom = ctypes.c_ulong


class ClientMessage(ctypes.Structure):
    _fields_ = [
        ("type", ctypes.c_int),
        ("serial", ctypes.c_ulong),
        ("send_event", ctypes.c_int),
        ("display", ctypes.c_void_p),
        ("window", Window),
        ("message_type", Atom),
        ("format", ctypes.c_int),
        ("data", ctypes.c_long * 5),
    ]


class XEvent(ctypes.Union):
    _fields_ = [("xclient", ClientMessage), ("pad", ctypes.c_long * 24)]


def main():
    x = ctypes.cdll.LoadLibrary(ctypes.util.find_library("X11") or "libX11.so.6")
    x.XOpenDisplay.restype = ctypes.c_void_p
    x.XDefaultRootWindow.restype = Window
    x.XDefaultRootWindow.argtypes = [ctypes.c_void_p]
    x.XInternAtom.restype = Atom
    x.XInternAtom.argtypes = [ctypes.c_void_p, ctypes.c_char_p, ctypes.c_int]
    x.XQueryTree.argtypes = [ctypes.c_void_p, Window, ctypes.POINTER(Window), ctypes.POINTER(Window),
                             ctypes.POINTER(ctypes.POINTER(Window)), ctypes.POINTER(ctypes.c_uint)]
    x.XGetWMProtocols.argtypes = [ctypes.c_void_p, Window, ctypes.POINTER(ctypes.POINTER(Atom)),
                                  ctypes.POINTER(ctypes.c_int)]
    x.XSendEvent.argtypes = [ctypes.c_void_p, Window, ctypes.c_int, ctypes.c_long,
                             ctypes.POINTER(XEvent)]

    d = x.XOpenDisplay(None)
    if not d:
        sys.exit("close-windows: no display")
    root = x.XDefaultRootWindow(d)
    wm_protocols = x.XInternAtom(d, b"WM_PROTOCOLS", 0)
    wm_delete = x.XInternAtom(d, b"WM_DELETE_WINDOW", 0)

    root_ret, parent_ret = Window(), Window()
    children = ctypes.POINTER(Window)()
    n = ctypes.c_uint()
    x.XQueryTree(d, root, ctypes.byref(root_ret), ctypes.byref(parent_ret), ctypes.byref(children),
                 ctypes.byref(n))
    asked = 0
    for i in range(n.value):
        w = children[i]
        protocols = ctypes.POINTER(Atom)()
        count = ctypes.c_int()
        if not x.XGetWMProtocols(d, w, ctypes.byref(protocols), ctypes.byref(count)):
            continue
        if wm_delete not in [protocols[j] for j in range(count.value)]:
            continue
        ev = XEvent()
        ev.xclient.type = 33  # ClientMessage
        ev.xclient.window = w
        ev.xclient.message_type = wm_protocols
        ev.xclient.format = 32
        ev.xclient.data[0] = wm_delete
        x.XSendEvent(d, w, 0, 0, ctypes.byref(ev))
        asked += 1
    x.XFlush(d)
    print(f"close-windows: asked {asked} window(s) to close", file=sys.stderr)


main()
