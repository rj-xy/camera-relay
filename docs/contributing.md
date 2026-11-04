# Contributing

Start with [architecture.md](architecture.md) (pipeline, threads, config) and [AGENTS.md](../AGENTS.md) (the rules the code keeps). Before touching the pipeline, read the hardware gotchas in [legacy-relay/README.md](../legacy-relay/README.md).

## Build

Rust from <https://rustup.rs> (stable; edition 2024 needs 1.85 or newer), plus the development packages:

| Distribution | Command |
| --- | --- |
| Debian, Ubuntu | `sudo apt install build-essential pkg-config libgtk-4-dev libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev gstreamer1.0-plugins-good gstreamer1.0-pipewire gstreamer1.0-libcamera` |
| Fedora | `sudo dnf install gcc pkgconf-pkg-config gtk4-devel gstreamer1-devel gstreamer1-plugins-base-devel gstreamer1-plugins-good pipewire-gstreamer libcamera-gstreamer` |
| Arch, Manjaro | `sudo pacman -S --needed base-devel gtk4 gstreamer gst-plugins-base gst-plugins-good gst-plugin-pipewire gst-plugin-libcamera` |
| openSUSE | `sudo zypper install gcc pkg-config gtk4-devel gstreamer-devel gstreamer-plugins-base-devel gstreamer-plugins-good gstreamer-plugin-pipewire gstreamer-plugins-libcamera` |

```bash
cargo build
cargo run -- run --start      # the app, camera on; Ctrl+C quits
cargo run -- settings         # from a second terminal
```

No camera? Put `source = "test"` under `[camera]` in `~/.config/camera-relay/config.toml`. To keep your real settings apart, point the app at another folder: `CAMERA_RELAY_CONFIG_DIR=/tmp/cr cargo run`.

More logging: `CAMERA_RELAY_LOG=debug`.

## Check

CI runs these on every pull request; run them before you push:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
shellcheck install.sh
```

Then try the change on a real camera: <https://webcamtests.com> shows what a browser gets.

## Release

1. Bump `version` in `Cargo.toml`, run `cargo build` so `Cargo.lock` follows, and commit both.
2. Tag and push: `git tag v0.2.0 && git push origin v0.2.0`.

The [release workflow](../.github/workflows/release.yml) runs the checks, stops if the tag doesn't match `Cargo.toml`, builds on Ubuntu 24.04 and publishes `camera-relay-x86_64-linux.tar.gz`, its `.sha256` and `install.sh`. The one-line install and `camera-relay update` fetch the latest of these.
