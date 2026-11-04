# Architecture

All milestones below are built: the relay, tray, settings window, profiles, start on login, crop, plugins and releases.

## Processes

One binary, `camera-relay`:

| Command | What it does |
| --- | --- |
| `camera-relay` (no arguments) | Runs the app: tray icon, pipeline manager, D-Bus service. Opens no window |
| `camera-relay run --start` | Same, and turns the relay on at once |
| `camera-relay run --no-tray` | Same, without the tray icon |
| `camera-relay settings` | Asks the running app to open its settings window (starts the app if needed) |
| `camera-relay start`, `stop`, `toggle`, `status` | Control the relay over D-Bus. Replaces `legacy-relay/camera-relay.sh` |
| `camera-relay profile [<name>]` | List profiles, or switch to one (a new name starts as a copy of the current profile) |
| `camera-relay rename-profile <from> <to>`, `delete-profile <name>` | Rename or delete a profile; deleting the active one switches to `default` first, and the last profile stays |
| `camera-relay set <key> <value>` | Change one setting of the active profile (adjustments, `plugins`, `<plugin>.<setting>`), live, and save it |
| `camera-relay autostart [off\|app\|camera]` | Start on login (no D-Bus: it writes the autostart file) |
| `camera-relay update` | Downloads the latest GitHub release, checks its SHA-256 and replaces the running binary |

The D-Bus name is `io.github.rj_xy.CameraRelay` (session bus). The tray, the settings window and the CLI are all clients of the same pipeline manager.

GTK and the tray icon are only touched on the GLib main thread. D-Bus calls, tray menu clicks and GStreamer callbacks come in on other threads; they send a `UiEvent` down a channel and `src/ui.rs` acts on it there. A burst of changes (a slider drag) becomes one refresh.

## Pipeline

Taken from the legacy relay. The parts in **bold** exist because of bugs found on the first laptop: keep them.

```
libcamerasrc (or v4l2src for other cameras)
  → caps: **2560×1440**            (libcamera aborts in the 1928×1088 binned mode of the OV08X40)
  → tee ─ queue (leaky, 1 frame) → valve → videoconvertscale → caps: 640 wide RGBA → videoflip → appsink
  │                                (source preview for the crop tool: the whole picture, turned like the output)
  → videocrop                      (the crop: before scaling, so a small area keeps its detail)
  → videoflip                      (flip, rotate)
  → videoconvertscale              (to the output size, black borders keep the aspect ratio)
  → videorate
  → plugin bins, in series        (each `videoconvert ! <plugin.toml pipeline> ! videoconvert`, named plugin-<id>)
  → videobalance                   (brightness, contrast, hue, saturation)
  → gamma
  → caps: **one fixed format**     (1280×720 YUY2 30 fps by default: Chromium skips PipeWire cameras that only list ranges)
  → tee
      ├─ queue → pipewiresink mode=provide  (node "Built-in Camera", Video/Source, role Camera)
      └─ queue (leaky, 1 frame) → valve → videoconvertscale → caps: 640 wide RGBA → appsink
                                   (the valve opens only while the settings window is open)
```

The source tee and its branch exist only when the app has a preview; a valve opens only while the window shows that preview.

Frames flow through the tee whether or not an app reads the camera, so the preview works on its own. The window turns each appsink frame into a `GdkMemoryTexture`: no `gtk4paintablesink` plugin needed, and GTK stays out of the pipeline module.

- **Live changes:** `videobalance`, `videocrop`, `gamma` and `videoflip` properties change while the pipeline runs, so sliders act at once with no flicker.
- **Plugins:** the profile's list of plugins becomes bins in the pipeline, so turning one on or off restarts the relay. Their sliders set element properties live (ints rounded). A plugin that can't build is skipped, not fatal. Format: [plugins.md](plugins.md).
- **Restart needed:** output size or frame rate, the source camera, the libcamera tuning file, the plugin list. The profile's aspect ratio sets the output size, so changing it restarts the relay (under the controller's lock, so nothing sees it off in between).
- **Aspect ratio:** one of 16:9, 4:3, 1:1 per profile. The output is the largest size of that shape inside `[output]` (1280×720 gives 1280×720, 960×720, 720×720): still one fixed size, as Chromium needs. Custom ratios are left out on purpose: browsers handle the presets, and odd sizes are untested.
- **Crop shape:** a new aspect ratio, or a quarter turn, refits the crop to the output's shape (same centre, about the same area). Crops from the window are snapped to the exact shape with even edges, so scaling adds no black bars.
- **Tuning file:** `LIBCAMERA_SIMPLE_TUNING_FILE` points at a libcamera soft-ISP tuning file (black level, colour matrix); see [legacy-relay/ov08x40.yaml](../legacy-relay/ov08x40.yaml).
- **PipeWire hygiene:** the legacy install also hides PipeWire's own libcamera camera and the raw `ipu7` capture devices with a WirePlumber rule ([legacy-relay/51-camera-relay.conf](../legacy-relay/51-camera-relay.conf)). `install.sh` doesn't add it, being specific to IPU7 laptops: when it finds the legacy relay it asks to remove only the service and binary, so the rule stays. The README tells a fresh IPU7 install to copy it.

## Config

Plain TOML under `~/.config/camera-relay/` (`CAMERA_RELAY_CONFIG_DIR` overrides it). The app writes both files with defaults on first run and saves every change at once, so settings survive restarts and reboots.

```
config.toml            # active_profile, [camera] source and capture size, [output] size, rate, name
profiles/default.toml  # brightness, contrast, hue, saturation, gamma, orientation, aspect, [crop],
                       # plugins = [...], [plugin_settings.<id>]
profiles/zoom.toml     # your own: camera-relay profile zoom
plugins/<id>/          # user plugins: plugin.toml, optional *.so
```

- Saves are atomic (temporary file, then rename). A file that doesn't parse is renamed to `<name>.broken-<unix time>` and replaced with defaults.
- A missing active profile falls back to `default`.
- `[camera] source = "test"` uses GStreamer's test pattern: try the app with no camera.
- Start on login is not in config: the XDG autostart file (`~/.config/autostart/camera-relay.desktop`) is the setting. `Exec=… run` starts the app, `Exec=… run --start` the camera too. KDE and GNOME both honour it.

## Logs and crashes

Logs go to the console and to the systemd journal (`journalctl -t camera-relay`). Level: `CAMERA_RELAY_LOG=debug` (or `RUST_LOG`). A panic hook logs every panic with a backtrace, and `main` turns errors and panics into a logged message and a non-zero exit code.

## Settings window

- Header bar: on/off switch at the start; **☰** menu at the end with **Start on login**, **Turn the camera on at login** and the version.
- Left, top to bottom:
  - **Profile**: picker, **+** (new profile, a copy of the current one), **⋮** (rename, delete), **Reset** (neutral picture, orientation and whole-picture crop, plugins off; keeps the aspect ratio).
  - Tabs, so the column stays short next to the preview:
    - **Picture**: brightness, contrast, saturation, hue, gamma.
    - **Framing**: aspect ratio (with the output size it gives), crop (**Select area**, **Whole picture**), orientation.
    - **Plugins**: a switch per plugin and its sliders; a plugin that can't run is greyed out with the reason.
- Right: live preview, or "The camera is off".
- Every change applies live and saves at once.
- **Select area** switches the preview to the source tap and draws the crop over it, the outside dimmed. Drag outside the crop to draw a new one from that corner, drag inside to move it, drag a corner to resize from the opposite one. The ratio stays locked; the crop saves when the drag ends. It works in the turned picture, so it matches what the output shows.
- GTK starts the first time the window opens, so an app that never shows it never loads a display connection.

## Tray

- StatusNotifierItem via `ksni`. The icon is drawn in code ([src/icon.rs](../src/icon.rs)) as pixmaps at 22, 32, 48 and 64 px, so it looks the same with any icon theme: a webcam with a light outline (shows on dark and light panels), a grey eye when off and a glowing red eye when on.
- Left click toggles. Right click: **Camera on** (checkmark), **Profile ▸** (radio list), **Settings…**, **Quit**. The tooltip shows on/off and the profile.
- Without a tray host (plain GNOME without the extension), the app logs that once and carries on; use the CLI or the settings window.

## Display servers

Nothing here depends on X11 or Wayland: capture and output go through PipeWire, the window is GTK4, the tray is D-Bus. Test on KDE Plasma (Wayland) and GNOME (Wayland) first, then X11.

## Milestones

1. 🟢 **M0 skeleton**: docs, legacy relay, empty crate.
2. 🟢 **M1 headless relay** in Rust: the pipeline above, config file, D-Bus service, `start`/`stop`/`toggle`/`status`.
3. 🟢 **M2 tray**: icon, on/off state, menu.
4. 🟢 **M3 settings window**: sliders and live preview.
5. 🟢 **M4 profiles**: create, switch, rename, delete, from the CLI, the tray and the window.
6. 🟢 **M5 start on login**: off, the app, or the app with the camera on.
7. 🟢 **M6 crop and filters**: crop tool with aspect ratio presets, plugins (Clean up, Soften, user plugins).
8. 🟢 **M7 releases and CI**: checks on every pull request, release binaries on a tag, the one-line installer and `camera-relay update`.
