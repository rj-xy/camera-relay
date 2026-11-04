#!/usr/bin/env bash
# Installs camera-relay from the latest GitHub release into ~/.local/bin. No sudo, except to
# install missing system packages, which it asks about first.
#
#   curl -fsSL https://github.com/rj-xy/camera-relay/releases/latest/download/install.sh | bash
#
# CAMERA_RELAY_VERSION=0.1.0 picks a release; CAMERA_RELAY_BIN_DIR picks where the binary goes.
set -euo pipefail

repo=rj-xy/camera-relay
version=${CAMERA_RELAY_VERSION:-latest}
bin_dir=${CAMERA_RELAY_BIN_DIR:-$HOME/.local/bin}
apps_dir=${XDG_DATA_HOME:-$HOME/.local/share}/applications

ok() { echo "🟢 $*"; }
warn() { echo "🟠 $*"; }
fail() {
  echo "🔴 $*" >&2
  echo "🔴 FAILED" >&2
  exit 1
}

# --- Machine ---------------------------------------------------------------------------------

[[ $(uname -s) == Linux ]] || fail "camera-relay runs on Linux only"
arch=$(uname -m)
[[ $arch == x86_64 ]] || fail "No prebuilt binary for $arch yet: build from source, see https://github.com/$repo#build-from-source"
for tool in curl tar sha256sum; do
  command -v "$tool" >/dev/null || fail "$tool is missing: install it first"
done

if [[ -f $HOME/.config/systemd/user/camera-relay.service ]]; then
  fail "The legacy shell relay is installed and uses the same command name. Remove it first:
   systemctl --user disable --now camera-relay
   rm ~/.config/systemd/user/camera-relay.service ~/.local/bin/camera-relay"
fi

# --- System packages -------------------------------------------------------------------------

# shellcheck disable=SC1091
[[ -r /etc/os-release ]] && . /etc/os-release
distro=" ${ID:-} ${ID_LIKE:-} "
case $distro in
  *" debian "* | *" ubuntu "*)
    packages=(libgtk-4-1 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-pipewire gstreamer1.0-libcamera)
    has() { [[ $(dpkg-query -W -f='${db:Status-Status}' "$1" 2>/dev/null) == installed ]]; }
    install_cmd=(sudo sh -c "apt-get update && apt-get install -y ${packages[*]}")
    ;;
  *" fedora "*)
    packages=(gtk4 gstreamer1-plugins-base gstreamer1-plugins-good pipewire-gstreamer libcamera-gstreamer)
    has() { rpm -q "$1" >/dev/null 2>&1; }
    install_cmd=(sudo dnf install -y "${packages[@]}")
    ;;
  *" arch "*)
    packages=(gtk4 gst-plugins-base gst-plugins-good gst-plugin-pipewire gst-plugin-libcamera)
    has() { pacman -Q "$1" >/dev/null 2>&1; }
    install_cmd=(sudo pacman -S --needed --noconfirm "${packages[@]}")
    ;;
  *" suse "* | *" opensuse "*)
    packages=(libgtk-4-1 gstreamer-plugins-base gstreamer-plugins-good gstreamer-plugin-pipewire gstreamer-plugins-libcamera)
    has() { rpm -q "$1" >/dev/null 2>&1; }
    install_cmd=(sudo zypper install -y "${packages[@]}")
    ;;
  *)
    packages=()
    warn "Unknown distribution (${ID:-none}): check the requirements in https://github.com/$repo#requirements"
    ;;
esac

missing=()
for package in "${packages[@]}"; do
  has "$package" || missing+=("$package")
done
if ((${#missing[@]})); then
  warn "Missing packages: ${missing[*]}"
  answer=n
  # Piped into bash, stdin is this script: ask on the terminal instead.
  if { exec 3</dev/tty; } 2>/dev/null; then
    read -r -u 3 -p "   Install them now with sudo? [Y/n] " answer || answer=n
    answer=${answer:-y}
  fi
  if [[ $answer == [Yy]* ]]; then
    "${install_cmd[@]}" </dev/tty || fail "Package install failed"
    ok "Packages installed"
  else
    warn "Skipped. Install them later with: ${install_cmd[*]}"
  fi
elif ((${#packages[@]})); then
  ok "System packages present"
fi

# --- Binary ----------------------------------------------------------------------------------

asset=camera-relay-$arch-linux.tar.gz
if [[ $version == latest ]]; then
  base=https://github.com/$repo/releases/latest/download
else
  base=https://github.com/$repo/releases/download/v${version#v}
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "🌱 Downloading $base/$asset"
(cd "$tmp" && curl -fsSLO "$base/$asset" && curl -fsSLO "$base/$asset.sha256") ||
  fail "Download failed: is there a release at https://github.com/$repo/releases?"
(cd "$tmp" && sha256sum --quiet -c "$asset.sha256") || fail "Checksum mismatch: download again"
tar -xzf "$tmp/$asset" -C "$tmp"

install -Dm755 "$tmp/camera-relay" "$bin_dir/camera-relay"
ok "Installed $bin_dir/camera-relay ($("$bin_dir/camera-relay" --version 2>/dev/null || echo "version unknown"))"

mkdir -p "$apps_dir"
cat >"$apps_dir/camera-relay.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Camera Relay
Comment=Virtual webcam with live picture adjustments
Exec=$bin_dir/camera-relay settings
Icon=camera-web
Categories=AudioVideo;Video;
Keywords=webcam;camera;virtual;
EOF
ok "Menu entry: Camera Relay"

# --- Checks ----------------------------------------------------------------------------------

if not_found=$(ldd "$bin_dir/camera-relay" 2>/dev/null | grep "not found"); then
  warn "Missing libraries, so it won't start yet:"
  echo "$not_found"
fi
case ":$PATH:" in
  *":$bin_dir:"*) ;;
  *) warn "$bin_dir isn't on your PATH: add it, or run $bin_dir/camera-relay" ;;
esac

echo
echo "🪵 Start it:   camera-relay            (tray icon; or open Camera Relay from the menu)"
echo "🪵 Settings:   camera-relay settings"
echo "🪵 Update:     camera-relay update"
echo "🟢 SUCCESS"
