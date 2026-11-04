# AGENTS.md

This repo is public. Never commit sensitive data to it.

- No API keys, tokens, credentials, private keys, email addresses, real names, or machine hostnames.
- Test fixtures use obviously fake values.

## Project

`camera-relay`: a Rust app that turns a real Linux camera into a PipeWire virtual webcam with a tray icon, live picture adjustments, cropping and per-app profiles. Read [README.md](README.md), then [docs/architecture.md](docs/architecture.md) (pipeline, config, milestones) and [docs/language-choice.md](docs/language-choice.md).

**Status: M0 to M7 built.** The app runs the pipeline, a tray icon, a GTK4 settings window (tabs: Picture, Framing, Plugins) with live preview, a D-Bus service and the CLI, with profiles, plugins, start on login, `update` and GitHub releases. Modules: `model` (plain data), `config` (generic TOML stores), `pipeline` (generic GStreamer relay), `plugins` (plugin catalog and the built-in elements), `controller` (state, ties them together, notifies listeners), `tray` (generic StatusNotifierItem), `icon` (the tray icon, drawn in code), `settings` (the GTK window), `ui` (main-thread glue for tray and window), `autostart`, `update`, `dbus`, `cli`, `daemon`, `logging`. Only the GLib main thread touches GTK or the tray: other threads send a `UiEvent`. The shell-script relay in [legacy-relay/](legacy-relay/) is what this app replaces; its [README](legacy-relay/README.md) lists hardware gotchas that the Rust pipeline must keep (read it before touching the pipeline). Docs for users and contributors: [docs/plugins.md](docs/plugins.md), [docs/contributing.md](docs/contributing.md).

## Stack

Rust (edition 2024), `gstreamer-rs`, `gtk4-rs` with `gst-plugin-gtk4` for the preview, `ksni` for the tray, `zbus` for D-Bus, `serde` + `toml` for config. Add crates only when the milestone needs them.

## Commands

```bash
cargo build
cargo run -- status
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

System packages for the GTK4 and GStreamer build are listed in the README. Check an element exists before using it: `gst-inspect-1.0 <element>`.

## Rules

- Milestones are in `docs/architecture.md`. Work one at a time; tick it there when it's done.
- Live adjustments (balance, crop, gamma, flip) must change element properties on the running pipeline, never restart it. Output size, frame rate and camera changes restart it.
- Keep the fixed output format (one size, one frame rate): Chromium skips PipeWire cameras that only list ranges.
- Always capture at 2560×1440 on the OV08X40: lower sizes crash libcamera.
- The app must work with no tray host (GNOME without AppIndicator): settings window and CLI stay usable.
- Heavy builds and tests: one at a time, in the foreground. Cap memory with `systemd-run --user --scope -p MemoryMax=6G cargo build` on small machines.
- Test on real hardware: `cam -l` lists cameras, <https://webcamtests.com> checks the output in a browser.

## Git

- Default branch `main`. One logical change per commit, with a message that says why. Fix-ups are `git commit --fixup <sha>`, squashed before pushing (`git rebase -i --autosquash`).
- Commit only the files you changed (`git add <paths>`, never `git add -A`): other sessions may edit the tree.
- Don't commit `target/`, local camera dumps, or recordings.
