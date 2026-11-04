//! The app's files under `~/.config/camera-relay/`:
//!
//! ```text
//! config.toml            # active profile, camera, output format
//! profiles/default.toml  # picture adjustments, one file per profile
//! ```

mod named;
pub mod store;

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::model::Adjustments;
pub use named::{NamedStore, validate_name};

pub const DEFAULT_PROFILE: &str = "default";

/// Where the files live. `CAMERA_RELAY_CONFIG_DIR` overrides the XDG location (tests, trying things out).
#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub dir: PathBuf,
}

impl ConfigPaths {
    pub fn from_env() -> Result<Self> {
        let var = |name| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let dir = if let Some(dir) = var("CAMERA_RELAY_CONFIG_DIR") {
            dir
        } else if let Some(xdg) = var("XDG_CONFIG_HOME") {
            xdg.join("camera-relay")
        } else if let Some(home) = var("HOME") {
            home.join(".config/camera-relay")
        } else {
            bail!("can't find the config folder: neither XDG_CONFIG_HOME nor HOME is set");
        };
        Ok(Self { dir })
    }

    pub fn app_config(&self) -> PathBuf {
        self.dir.join("config.toml")
    }

    pub fn profiles(&self) -> NamedStore<Adjustments> {
        NamedStore::new(self.dir.join("profiles"))
    }

    /// A path from a config file: absolute as is, relative to the config folder otherwise.
    /// Folder of the user's own plugins, one folder each.
    pub fn plugins(&self) -> PathBuf {
        self.dir.join("plugins")
    }

    pub fn resolve(&self, path: &str) -> PathBuf {
        self.dir.join(path)
    }
}

/// `config.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_profile: String,
    pub camera: CameraConfig,
    pub output: OutputConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            active_profile: DEFAULT_PROFILE.to_owned(),
            camera: CameraConfig::default(),
            output: OutputConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Libcamera,
    V4l2,
    /// GStreamer's test pattern, for trying the app without a camera.
    Test,
}

/// The real camera. Changes here take effect the next time the relay starts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CameraConfig {
    pub source: SourceKind,
    /// libcamera camera name or V4L2 device path; empty picks the first camera.
    pub device: String,
    /// Size to read from the camera; 0 lets the camera choose. The OV08X40 needs 2560×1440:
    /// smaller sizes switch it to a binned mode that crashes libcamera.
    pub capture_width: u32,
    pub capture_height: u32,
    /// libcamera soft-ISP tuning file, relative to the config folder; empty for none.
    /// Read once when the app starts.
    pub tuning_file: String,
    /// Extra properties set on the source element, as `gst-launch-1.0` would take them.
    /// A new config.toml gets the legacy relay's libcamera colour; a file without this table
    /// sets none, so `source = "test"` or `"v4l2"` works without clearing it.
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
}

impl Default for CameraConfig {
    fn default() -> Self {
        Self {
            source: SourceKind::Libcamera,
            device: String::new(),
            capture_width: 2560,
            capture_height: 1440,
            tuning_file: "ov08x40.yaml".to_owned(),
            // The legacy relay's colour, tuned together with ov08x40.yaml.
            properties: BTreeMap::from([("saturation".to_owned(), "1.3".to_owned())]),
        }
    }
}

/// The virtual camera that apps see. One fixed size and frame rate: Chromium skips PipeWire
/// cameras that only list ranges.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub format: String,
    /// The camera name apps show.
    pub name: String,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            framerate: 30,
            format: "YUY2".to_owned(),
            name: "Built-in Camera".to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_file_fills_in_defaults() {
        let config: AppConfig =
            toml::from_str("active_profile = \"zoom\"\n[output]\nwidth = 640").unwrap();
        assert_eq!(config.active_profile, "zoom");
        assert_eq!(config.output.width, 640);
        assert_eq!(config.output.height, 720);
        assert_eq!(config.camera, CameraConfig::default());
    }

    #[test]
    fn camera_table_without_properties_sets_none() {
        let config: AppConfig = toml::from_str("[camera]\nsource = \"test\"").unwrap();
        assert_eq!(config.camera.source, SourceKind::Test);
        assert_eq!(config.camera.capture_width, 2560);
        assert!(config.camera.properties.is_empty());
    }

    #[test]
    fn default_config_round_trips() {
        let text = toml::to_string_pretty(&AppConfig::default()).unwrap();
        assert_eq!(
            toml::from_str::<AppConfig>(&text).unwrap(),
            AppConfig::default()
        );
    }
}
