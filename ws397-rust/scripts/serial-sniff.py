#!/usr/bin/env python3
"""Read the ESP32 serial console for N seconds and print it.

Usage: serial-sniff.py [seconds]   (default 5)
The board must not be held open by another monitor.
"""
import os, sys, time, select, termios, tty, glob

port = glob.glob("/dev/ttyACM*")
if not port:
    sys.exit("no /dev/ttyACM* found")
port = os.environ.get("SERIAL_PORT", port[0])
secs = float(sys.argv[1]) if len(sys.argv) > 1 else 5.0

fd = os.open(port, os.O_RDWR | os.O_NOCTTY | os.O_NONBLOCK)
try:
    attrs = termios.tcgetattr(fd)
    attrs[0] &= ~(termios.IGNBRK | termios.BRKINT | termios.PARMRK)
    termios.tcsetattr(fd, termios.TCSANOW, attrs)
except termios.error:
    pass

buf = bytearray()
deadline = time.time() + secs
while time.time() < deadline:
    r, _, _ = select.select([fd], [], [], 0.2)
    if r:
        try:
            buf += os.read(fd, 4096)
        except BlockingIOError:
            pass
os.close(fd)
sys.stdout.write(buf.decode("utf-8", "replace"))
