//! Start on login: an XDG autostart entry, `~/.config/autostart/camera-relay.desktop`, which KDE,
//! GNOME and most other desktops run at login. The file is the setting: no copy in config.toml.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const FILE: &str = "camera-relay.desktop";

/// What happens at login.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Nothing
    Off,
    /// The app starts, with its tray icon; the camera stays off
    App,
    /// The app starts and turns the camera on
    Camera,
}

/// Prints the setting, or changes it.
pub fn run(mode: Option<Mode>) -> Result<()> {
    let path = path()?;
    let mode = match mode {
        Some(mode) => {
            set(&path, mode)?;
            mode
        }
        None => get(&path),
    };
    let what = match mode {
        Mode::Off => "off",
        Mode::App => "the app starts, camera off",
        Mode::Camera => "the app starts and turns the camera on",
    };
    println!("🟢 Start on login: {what}");
    Ok(())
}

pub fn path() -> Result<PathBuf> {
    let config = match std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let home = std::env::var_os("HOME").context("HOME isn't set")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(config.join("autostart").join(FILE))
}

pub fn get(path: &Path) -> Mode {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Mode::Off;
    };
    let line = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key).map(str::trim).map(str::to_owned))
    };
    if line("Hidden=").as_deref() == Some("true") {
        Mode::Off
    } else if line("Exec=").is_some_and(|exec| exec.ends_with(" --start")) {
        Mode::Camera
    } else {
        Mode::App
    }
}

pub fn set(path: &Path, mode: Mode) -> Result<()> {
    let start = match mode {
        Mode::Off => {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(e).with_context(|| format!("can't delete {}", path.display()))
                }
                _ => Ok(()),
            };
        }
        Mode::App => "",
        Mode::Camera => " --start",
    };
    let exe = std::env::current_exe().context("can't find this program's path")?;
    // Linux adds this when the file was replaced since it started, as `update` does.
    let exe = exe.to_string_lossy();
    let exe = exe.strip_suffix(" (deleted)").unwrap_or(&exe);
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Camera Relay\n\
         Comment=Virtual webcam with live picture adjustments\n\
         Exec={} run{start}\n\
         Icon=camera-web\n\
         NoDisplay=true\n\
         X-GNOME-Autostart-enabled=true\n",
        quote(exe)
    );
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
    }
    std::fs::write(path, entry).with_context(|| format!("can't write {}", path.display()))
}

/// Quotes a path for an `Exec=` line, as the desktop entry spec asks.
fn quote(path: &str) -> String {
    if !path.contains(|c: char| c.is_whitespace() || "\"'\\`$;&|<>()*?#~".contains(c)) {
        return path.to_owned();
    }
    let mut quoted = String::from('"');
    for c in path.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_mode_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("autostart").join(FILE);
        assert_eq!(get(&path), Mode::Off);
        for mode in [Mode::Camera, Mode::App, Mode::Off] {
            set(&path, mode).unwrap();
            assert_eq!(get(&path), mode);
        }
        set(&path, Mode::Off).unwrap();
    }

    #[test]
    fn odd_paths_are_quoted() {
        assert_eq!(quote("/usr/bin/camera-relay"), "/usr/bin/camera-relay");
        assert_eq!(quote("/home/a b/$x"), r#""/home/a b/\$x""#);
    }
}
