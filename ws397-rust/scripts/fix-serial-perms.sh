#!/usr/bin/env bash
# Re-apply world-writable mode to the current ESP32-S3 node (udev rule covers
# vid 303a only on the interface we matched earlier; re-enumeration resets mode).
for n in /dev/ttyACM*; do
  [ -e "$n" ] || continue
  chmod 666 "$n" 2>/dev/null || sudo chmod 666 "$n"
done
