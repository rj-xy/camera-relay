# Architecture

Plan only; nothing here is built yet except the [legacy relay](../legacy-relay/README.md).

## Processes

One binary, `camera-relay`:

| Command | What it does |
| --- | --- |
| `camera-relay` (no arguments) | Runs the app: tray icon, pipeline manager, D-Bus service. Opens no window |
| `camera-relay settings` | Asks the running app to open its settings window (starts the app if needed) |
| `camera-relay start`, `stop`, `toggle`, `status` | Control the relay over D-Bus. Replaces `legacy-relay/camera-relay.sh` |
| `camera-relay profile <name>` | Switch profile |

The D-Bus name is `io.github.rj_xy.CameraRelay` (session bus). The tray, the settings window and the CLI are all clients of the same pipeline manager.

## Pipeline

Taken from the legacy relay. The parts in **bold** exist because of bugs found on the first laptop: keep them.

```
libcamerasrc (or v4l2src for other cameras)
  → caps: **2560×1440**            (libcamera aborts in the 1928×1088 binned mode of the OV08X40)
  → videocrop                      (zoom and pan: crops before scaling, so zoom keeps detail)
  → videoflip                      (flip, rotate)
  → videoconvertscale              (to the output size)
  → videobalance                   (brightness, contrast, hue, saturation)
  → gamma
  → caps: **one fixed format**     (1280×720 YUY2 30 fps: Chromium skips PipeWire cameras that only list ranges)
  → tee
      ├─ queue → pipewiresink mode=provide  (node "Built-in Camera", Video/Source, role Camera)
      └─ queue (leaky) → gtk4paintablesink  (only while the settings window is open)
```

- **Live changes:** `videobalance`, `videocrop`, `gamma` and `videoflip` properties change while the pipeline runs, so sliders act at once with no flicker.
- **Restart needed:** output size or frame rate, the source camera, the libcamera tuning file.
- **Tuning file:** `LIBCAMERA_SIMPLE_TUNING_FILE` points at a libcamera soft-ISP tuning file (black level, colour matrix); see [legacy-relay/ov08x40.yaml](../legacy-relay/ov08x40.yaml). Later: generate it from sliders.
- **PipeWire hygiene:** the legacy install also hides PipeWire's own libcamera camera and the raw `ipu7` capture devices with a WirePlumber rule ([legacy-relay/51-camera-relay.conf](../legacy-relay/51-camera-relay.conf)); the app's installer should keep doing that.

## Config

Plain TOML under `~/.config/camera-relay/`:

```
config.toml            # start_app_on_login, start_relay_on_login, active_profile, camera, output size
profiles/default.toml  # brightness, contrast, hue, saturation, gamma, crop {left,right,top,bottom}, flip
profiles/zoom.toml     # your own, created in the settings window
```

Start on login is an XDG autostart entry (`~/.config/autostart/camera-relay.desktop`), which both KDE and GNOME honour.

## Settings window

- Left: sliders grouped as Picture (brightness, contrast, hue, saturation, gamma), Crop (zoom, pan x, pan y; later drag a rectangle on the preview), Orientation. Right: live preview.
- Top: profile picker with New, Duplicate, Rename, Delete. **Reset** restores the profile to its saved values.
- General tab: the two start-on-login switches, camera choice, output size.

## Tray

- StatusNotifierItem via `ksni`. Two icons: on and off (theme icons `camera-web` and `camera-off` first, own SVGs if the theme lacks them).
- Left click toggles. Right click: **On/Off**, **Profile ▸** (radio list), **Settings…**, **Quit**.
- Without a tray host (plain GNOME without the extension), the app logs that once and carries on; use the CLI or the settings window.

## Display servers

Nothing here depends on X11 or Wayland: capture and output go through PipeWire, the window is GTK4, the tray is D-Bus. Test on KDE Plasma (Wayland) and GNOME (Wayland) first, then X11.

## Milestones

1. 🟠 **M0 skeleton** (this repo: docs, legacy relay, empty crate).
2. ⬜ **M1 headless relay** in Rust: the pipeline above, config file, D-Bus service, `start`/`stop`/`toggle`/`status`.
3. ⬜ **M2 tray**: icon, on/off state, menu.
4. ⬜ **M3 settings window**: sliders and live preview.
5. ⬜ **M4 profiles**: create, switch, save; tray submenu.
6. ⬜ **M5 start on login**, general settings.
7. ⬜ **M6 crop with a draggable rectangle**, more filters.
8. ⬜ **M7 packaging** (`.deb` first) and CI.

## Ideas, not planned

- Switch profile by app: PipeWire shows which app opened the camera, so a profile could follow it.
- Background blur or replacement (needs a segmentation model).
- Colour-temperature slider that rewrites the libcamera tuning file.
