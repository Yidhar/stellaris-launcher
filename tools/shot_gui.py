"""Starts the launcher window, waits for it to appear, saves a screenshot of its client area and closes it again (a development tool).

    python tools/shot_gui.py [--exe target/debug/stellaris-launcher.exe] [--out work/gui.png] [--wait 4] [--idle 20]

Waits until the user has been idle for --idle seconds first (a new window takes the focus), through the desktop_guard module of the sibling
stellaris-live2d repository if it is next to this one.
"""
import argparse
import ctypes
import ctypes.wintypes as w
import os
import subprocess
import sys
import time

user32 = ctypes.WinDLL("user32", use_last_error=True)
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def idle_seconds():
    class LASTINPUTINFO(ctypes.Structure):
        _fields_ = [("cbSize", w.UINT), ("dwTime", w.DWORD)]

    info = LASTINPUTINFO(cbSize=ctypes.sizeof(LASTINPUTINFO))
    user32.GetLastInputInfo(ctypes.byref(info))
    return (ctypes.windll.kernel32.GetTickCount() - info.dwTime) / 1000.0


def find_window(title, pid):
    found = []

    @ctypes.WINFUNCTYPE(w.BOOL, w.HWND, w.LPARAM)
    def each(hwnd, _):
        owner = w.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd):
            buf = ctypes.create_unicode_buffer(256)
            user32.GetWindowTextW(hwnd, buf, 256)
            if title in buf.value:
                found.append(hwnd)
        return True

    user32.EnumWindows(each, 0)
    return found[0] if found else None


def save_window(hwnd, width, height, path):
    """The window's own picture (PrintWindow with PW_RENDERFULLCONTENT), not a grab of the screen: other windows on top of it, which on a
    working machine can be a chat, never end up in the file."""
    from PIL import Image

    gdi32 = ctypes.WinDLL("gdi32")
    hdc = user32.GetWindowDC(hwnd)
    mem = gdi32.CreateCompatibleDC(hdc)
    bmp = gdi32.CreateCompatibleBitmap(hdc, width, height)
    gdi32.SelectObject(mem, bmp)
    user32.PrintWindow(hwnd, mem, 2)

    class BITMAPINFOHEADER(ctypes.Structure):
        _fields_ = [("biSize", w.DWORD), ("biWidth", w.LONG), ("biHeight", w.LONG), ("biPlanes", w.WORD), ("biBitCount", w.WORD), ("biCompression", w.DWORD),
                    ("biSizeImage", w.DWORD), ("biXPelsPerMeter", w.LONG), ("biYPelsPerMeter", w.LONG), ("biClrUsed", w.DWORD), ("biClrImportant", w.DWORD)]

    info = BITMAPINFOHEADER(biSize=ctypes.sizeof(BITMAPINFOHEADER), biWidth=width, biHeight=-height, biPlanes=1, biBitCount=32, biCompression=0)
    buf = ctypes.create_string_buffer(width * height * 4)
    gdi32.GetDIBits(mem, bmp, 0, height, buf, ctypes.byref(info), 0)
    gdi32.DeleteObject(bmp)
    gdi32.DeleteDC(mem)
    user32.ReleaseDC(hwnd, hdc)
    Image.frombuffer("RGBA", (width, height), buf, "raw", "BGRA", 0, 1).convert("RGB").save(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--exe", default=os.path.join(ROOT, "target", "debug", "stellaris-launcher.exe"))
    ap.add_argument("--out", default=os.path.join(ROOT, "work", "gui.png"))
    ap.add_argument("--wait", type=float, default=4.0, help="seconds to let it draw before the shot")
    ap.add_argument("--idle", type=float, default=20.0)
    ap.add_argument("--gui-args", default="", help="arguments for the window, e.g. \"--tab=1\"")
    a = ap.parse_args()
    user32.SetProcessDPIAware()
    deadline = time.time() + 540
    while idle_seconds() < a.idle:
        if time.time() > deadline:
            raise SystemExit("the user did not become idle; nothing was started")
        time.sleep(2)
    p = subprocess.Popen([a.exe, *a.gui_args.split()])
    try:
        hwnd = None
        for _ in range(60):
            hwnd = find_window("Stellaris Launcher", p.pid)
            if hwnd:
                break
            time.sleep(0.5)
        if not hwnd:
            raise SystemExit("the window did not appear")
        time.sleep(a.wait)
        rect = w.RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(rect))
        width, height = rect.right - rect.left, rect.bottom - rect.top
        os.makedirs(os.path.dirname(a.out), exist_ok=True)
        save_window(hwnd, width, height, a.out)
        print("saved", a.out, (width, height))
    finally:
        user32.PostMessageW(hwnd, 0x0010, 0, 0) if hwnd else None  # WM_CLOSE
        time.sleep(1)
        if p.poll() is None:
            p.terminate()


if __name__ == "__main__":
    main()
