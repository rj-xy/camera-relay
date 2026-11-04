# Language and framework choice

Decided 2026-11-05. Picked from Rust, Go and Zig: a compiled, fast language with the easiest Linux GUI and tray story.

**Pick: Rust**, with GStreamer (`gstreamer-rs`), GTK4 (`gtk4-rs`) and `ksni` for the tray.

## Languages

| | Rust 🟢 | Go 🟠 | Zig 🔴 |
| --- | --- | --- | --- |
| GStreamer | `gstreamer-rs`: official, maintained by GStreamer developers, covers the whole API and writing elements | `go-gst`: cgo bindings, thinner and less maintained | none; call the C API by hand |
| GTK4 | `gtk4-rs`: official, well documented | `gotk4`: generated, works, smaller community | none; C interop only |
| Live preview in a window | `gst-plugin-gtk4` (`gtk4paintablesink`) is written in Rust, same ecosystem | possible, but you wire the sink yourself | by hand |
| Tray (StatusNotifierItem) | `ksni`: pure Rust over D-Bus, works on KDE and GNOME | `fyne.io/systray` and others work, but pull in a second toolkit's loop | by hand over D-Bus |
| Single static-ish binary, fast start | yes | yes | yes |
| Maturity for this job | high | medium | low (pre-1.0, no GUI ecosystem) |

The deciding fact: the working relay is already a GStreamer pipeline, and Rust is the only one of the three where GStreamer and GTK4 are both first-class.

## UI toolkit

| | Verdict |
| --- | --- |
| **GTK4** (`gtk4-rs`) 🟢 | Best live-preview story (`gtk4paintablesink`), runs on Wayland and X11, works on KDE and GNOME. Looks slightly foreign on KDE, which is acceptable for a small settings window |
| Qt6 (`cxx-qt`, QML) 🟠 | Native on KDE, fine on GNOME, and `qml6glsink` gives a preview. The Rust bindings are younger and the build is heavier. The fallback if GTK4 looks too out of place |
| `egui` / `iced` / Slint 🔴 | Easy to start, but no clean way to put a GStreamer GL preview in them, and Slint has no tray |
| Electron / Tauri 🔴 | Too heavy; webview video preview of a PipeWire stream is awkward |

## Tray on GNOME

GTK4 dropped `GtkStatusIcon`, so the tray is a separate StatusNotifierItem (`ksni`). KDE shows it natively. GNOME shows it only with the AppIndicator extension, which Ubuntu enables by default; other GNOME distributions need it installed. The app must still work without a tray (settings window and CLI).
