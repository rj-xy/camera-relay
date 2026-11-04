# camera-relay

A virtual webcam for Linux with a tray icon, live picture adjustments and per-app profiles.

It reads a real camera (built for the Dell XPS 14 DA14260's OmniVision OV08X40 sensor on Intel IPU7, but any libcamera or V4L2 camera is the goal), cleans up the picture, and offers the result to browsers and call apps as **Built-in Camera** through PipeWire.

> 🟠 **Status: skeleton.** The app doesn't exist yet. What works today is the shell-script relay in [legacy-relay/](legacy-relay/), which the app will replace. Its [README](legacy-relay/README.md) holds the full hardware bring-up notes and the gotchas found on the way (a libcamera crash below 2560×1440, Chromium skipping cameras that only list size ranges, the missing `intel_cvs` driver).

## What it will do

- 🔘 **Tray icon** (KDE and GNOME): click to turn the relay on or off, an icon that shows which. Right click for the profile list, **Settings…** and **Quit**.
- 🎚️ **Adjustments**, live while a call is running: brightness, contrast, hue, saturation, gamma, white balance, sharpness, flip and rotate, and **cropping** (zoom and pan, later a draggable rectangle).
- 👀 **Live preview** of the processed stream inside the settings window, only while that window is open.
- 🗂️ **Profiles**: a `Default` profile, plus your own (Zoom, Teams, Slack, Discord…), each with its own crop, colour and exposure settings. Switch from the tray menu.
- 🚀 **Start on login**: two separate switches, one for the app and one for turning the relay on.
- ⌨️ **Command line** for shortcuts and scripts: `camera-relay toggle`, `start`, `stop`, `status`, `profile <name>`.

## Why this shape

| Piece | Choice | Why |
| --- | --- | --- |
| Language | Rust | First-class GStreamer and GTK4 bindings, one fast binary. [Why not Go or Zig](docs/language-choice.md) |
| Video | GStreamer | The legacy relay is already a GStreamer pipeline; `videobalance`, `videocrop`, `gamma` and friends do the adjustments live |
| Output | PipeWire (`pipewiresink`) | Browsers read cameras through PipeWire on Wayland and X11 alike |
| Tray | StatusNotifierItem (`ksni`) | Native on KDE Plasma. GNOME needs the AppIndicator extension (Ubuntu ships it on by default) |
| Settings window | GTK4 + `gtk4paintablesink` | Live preview with no copying of frames |

Design details, the pipeline and the milestones: [docs/architecture.md](docs/architecture.md).

## Use the legacy relay today

```bash
bash legacy-relay/setup-root.sh   # packages and the intel_cvs driver, asks for sudo
bash legacy-relay/install.sh      # user service, menu entry, WirePlumber rules
camera-relay toggle               # on and off
```

Read [legacy-relay/README.md](legacy-relay/README.md) first: it is written for one specific laptop.

## Build (once there's code)

Ubuntu or Kubuntu 26.04:

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  gstreamer1.0-plugins-good gstreamer1.0-plugins-bad gstreamer1.0-libcamera gstreamer1.0-pipewire gstreamer1.0-gtk4
cargo build
```

## Licence

MIT, see [LICENSE](LICENSE).
