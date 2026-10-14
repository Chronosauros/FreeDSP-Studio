FreeDSP Studio for Linux (x86_64)

Run it straight from this folder:  ./freedsp-studio
Or install it:                      ./install.sh   (or sudo ./install.sh for all users)

Linux won't let a normal user open the cable without a udev rule. install.sh puts
70-freedsp-studio.rules in /etc/udev/rules.d for you. If you'd rather do it by hand:

  sudo cp 70-freedsp-studio.rules /etc/udev/rules.d/
  sudo udevadm control --reload-rules && sudo udevadm trigger

then unplug and replug the cable.

Needs: glibc 2.35 or newer, ALSA (libasound.so.2), libudev, a Wayland or X11 desktop
with OpenGL. File dialogs use the XDG desktop portal; dropping files on the window
works without it.

https://github.com/EffectiveEquivalent/FreeDSP-Studio
