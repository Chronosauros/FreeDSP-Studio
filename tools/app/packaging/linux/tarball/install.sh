#!/bin/sh
# Installs FreeDSP Studio from this folder.
#
#   ./install.sh          for you only: ~/.local/bin, ~/.local/share
#   sudo ./install.sh     for everyone: /usr/local/bin, /usr/local/share
#
# Either way the udev rule (so the app can open the cable without root) goes in
# /etc/udev/rules.d, which needs sudo; you'll be asked for it if you're not root.
set -e
cd "$(dirname "$0")"
if [ "$(id -u)" = 0 ]; then
  prefix=/usr/local
  sudo=""
else
  prefix="$HOME/.local"
  sudo="sudo"
fi
install -Dm755 freedsp-studio "$prefix/bin/freedsp-studio"
install -Dm644 freedsp-studio.desktop "$prefix/share/applications/freedsp-studio.desktop"
install -Dm644 freedsp-studio.png "$prefix/share/icons/hicolor/128x128/apps/freedsp-studio.png"
$sudo install -Dm644 70-freedsp-studio.rules /etc/udev/rules.d/70-freedsp-studio.rules
$sudo udevadm control --reload-rules || true
$sudo udevadm trigger --subsystem-match=hidraw || true
echo "Installed to $prefix/bin/freedsp-studio. Unplug and replug the cable before you start it."
case ":$PATH:" in *":$prefix/bin:"*) ;; *) echo "Note: $prefix/bin isn't on your PATH." ;; esac
