#!/usr/bin/env bash
# User part of the built-in camera setup: relay service, toggle, KDE launcher,
# WirePlumber rules, Edge launcher with PipeWire camera. Run setup-root.sh first.
# Safe to re-run. Usage: bash install.sh
set -uo pipefail
here=$(cd "$(dirname "$0")" && pwd)
ok()   { echo "🟢 $*"; }
fail() { echo "🔴 $*"; echo; echo "🔴 FAILED"; exit 1; }

command -v gst-launch-1.0 >/dev/null || fail "gst-launch-1.0 missing: run setup-root.sh first"
gst-inspect-1.0 pipewiresink >/dev/null 2>&1 || fail "pipewiresink missing: run setup-root.sh first"
[ -e /sys/bus/i2c/drivers/ov08x40/i2c-OVTI08F4:00 ] || fail "ov08x40 sensor not bound: run setup-root.sh, then reboot"

echo "🌱 Installing files"
install -Dm644 "$here/camera-relay.service"   ~/.config/systemd/user/camera-relay.service
install -Dm644 "$here/ov08x40.yaml"          ~/.config/camera-relay/ov08x40.yaml
install -Dm644 "$here/51-camera-relay.conf"   ~/.config/wireplumber/wireplumber.conf.d/51-camera-relay.conf
install -Dm755 "$here/camera-relay.sh"        ~/.local/bin/camera-relay
mkdir -p ~/.local/share/applications
sed "s#^Exec=camera-relay#Exec=$HOME/.local/bin/camera-relay#" "$here/camera-relay.desktop" > ~/.local/share/applications/camera-relay.desktop
ok "Relay service, tuning file, WirePlumber rules, ~/.local/bin/camera-relay, KDE launcher"

edge=/usr/share/applications/microsoft-edge.desktop
if [ -f "$edge" ]; then
  sed -E 's#^Exec=/usr/bin/microsoft-edge-stable#Exec=/usr/bin/microsoft-edge-stable --enable-features=WebRtcPipeWireCamera#' \
    "$edge" > ~/.local/share/applications/microsoft-edge.desktop
  ok "Edge launcher override with --enable-features=WebRtcPipeWireCamera"
else
  echo "🟠 Edge not installed, skipped its launcher override"
fi

systemctl --user daemon-reload || fail "systemctl --user daemon-reload failed"
kbuildsycoca6 >/dev/null 2>&1 || true
ok "systemd and KDE menu reloaded"

if wpctl status 2>/dev/null | grep -q '\[libcamera\]'; then
  systemctl --user restart wireplumber && sleep 3
  ok "WirePlumber restarted (libcamera camera removed from PipeWire)"
fi
wpctl status | grep -q '\[libcamera\]' && fail "PipeWire still has the libcamera camera (it crashes PipeWire)"
ok "PipeWire's libcamera camera is off"

echo
echo "🟢 SUCCESS"
echo "🪵 Turn the camera on with: camera-relay start (or KDE menu: Camera Relay (on/off))"
echo "🪵 Brave: enable brave://flags/#enable-webrtc-pipewire-camera by hand (no launcher override)"
