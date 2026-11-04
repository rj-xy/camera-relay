#!/usr/bin/env bash
# Root part of the built-in camera setup on a Dell XPS 14 DA14260 (ov08x40 on IPU7):
# CVS driver, libcamera + PipeWire plugin, GStreamer tools. Removes the old v4l2loopback
# /dev/video60 setup (used until 2026-11-04 11:40 AEST).
# Safe to re-run. Usage: bash setup-root.sh   (asks for the sudo password)
set -uo pipefail
ok()   { echo "🟢 $*"; }
fail() { echo "🔴 $*"; echo; echo "🔴 FAILED"; exit 1; }

[ "$(id -u)" -eq 0 ] || exec sudo bash "$0" "$@"

echo "🌱 Installing packages"
pkgs=(
  linux-modules-vision-generic        # intel_cvs: hands the sensor over from the CVS chip (INTC10E1)
  libcamera-tools libspa-0.2-libcamera gstreamer1.0-libcamera
  gstreamer1.0-tools gstreamer1.0-pipewire  # gst-launch-1.0 and pipewiresink for camera-relay.service
)
apt-get install -y "${pkgs[@]}" >/tmp/camera-setup-apt.log 2>&1 \
  || fail "apt install failed, see /tmp/camera-setup-apt.log"
ok "Installed: ${pkgs[*]}"

echo "🌱 Removing the old v4l2loopback /dev/video60 setup"
rm -f /etc/modprobe.d/v4l2loopback-camera-relay.conf /etc/modules-load.d/v4l2loopback-camera-relay.conf
if [ -d /sys/module/v4l2loopback ]; then
  modprobe -r v4l2loopback || echo "🟠 v4l2loopback in use, it goes at the next reboot"
fi
ok "No v4l2loopback config left"

command -v gst-launch-1.0 >/dev/null || fail "gst-launch-1.0 missing"
gst-inspect-1.0 libcamerasrc >/dev/null 2>&1 || fail "GStreamer libcamerasrc missing"
gst-inspect-1.0 pipewiresink >/dev/null 2>&1 || fail "GStreamer pipewiresink missing"
ok "GStreamer elements present"

[ -d /sys/module/intel_cvs ] && ok "intel_cvs loaded" || echo "🟠 intel_cvs not loaded yet: reboot once"

echo
echo "🟢 SUCCESS"
