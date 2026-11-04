#!/usr/bin/env bash
# Start/stop the built-in camera relay. Usage: camera-relay [toggle|start|stop|status]
set -euo pipefail
svc=camera-relay.service
notify() { command -v notify-send >/dev/null && notify-send -a "Camera relay" -i camera-web "$1" || true; echo "$1"; }
case "${1:-toggle}" in
  start)  systemctl --user start "$svc"; notify "🟢 Camera relay on" ;;
  stop)   systemctl --user stop "$svc";  notify "🔴 Camera relay off" ;;
  status) systemctl --user status "$svc" --no-pager ;;
  toggle) if systemctl --user is-active -q "$svc"; then "$0" stop; else "$0" start; fi ;;
  *) echo "Usage: camera-relay [toggle|start|stop|status]"; exit 1 ;;
esac
