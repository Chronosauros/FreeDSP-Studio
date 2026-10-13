#!/bin/sh
# Pick up the udev rule so a cable that's already plugged in works without a reboot.
if command -v udevadm >/dev/null 2>&1; then
  udevadm control --reload-rules || true
  udevadm trigger --subsystem-match=hidraw || true
fi
exit 0
