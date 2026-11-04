//! Plugins: effects that run in series between the picture adjustments and the output.
//!
//! A plugin is a folder with a `plugin.toml`: a name, a chain of GStreamer elements as
//! `gst-launch-1.0` takes it, and sliders bound to element properties. The app ships two
//! (in `plugins/` in the repo, built in); more go in `~/.config/camera-relay/plugins/<id>/`,
//! where a compiled GStreamer element (`*.so`) can sit next to the `plugin.toml`. Format and
//! examples: docs/plugins.md.

mod denoise;
mod soften;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, MutexGuard, Once, PoisonError};

use anyhow::{Context, Result, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_video as gst_video;
use serde::Deserialize;

use crate::config::validate_name;
use crate::model::Adjustments;
use crate::pipeline::FilterSpec;

/// Plugins built into the binary, by id. The same files are in `plugins/` in the repo.
const BUNDLED: [(&str, &str); 2] = [
    ("cleanup", include_str!("../../plugins/cleanup/plugin.toml")),
    ("soften", include_str!("../../plugins/soften/plugin.toml")),
];

#[derive(Clone, Debug, Deserialize)]
pub struct Plugin {
    /// The folder name; bundled plugins have their own.
    #[serde(skip)]
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// GStreamer elements in series, such as `videomedian name=median filtersize=5`.
    pub pipeline: String,
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// The folder it was loaded from; `None` when built in.
    #[serde(skip)]
    pub folder: Option<PathBuf>,
    /// Why it can't run (a missing element, a typo), shown instead of its switch.
    #[serde(skip)]
    pub problem: Option<String>,
}

/// One slider: a number that sets `property` on the element called `element`.
#[derive(Clone, Debug, Deserialize)]
pub struct Setting {
    pub key: String,
    pub label: String,
    pub element: String,
    pub property: String,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl Plugin {
    /// The name of its bin in the pipeline.
    pub fn filter_name(&self) -> String {
        format!("plugin-{}", self.id)
    }

    pub fn setting(&self, key: &str) -> Option<&Setting> {
        self.settings.iter().find(|s| s.key == key)
    }

    /// The setting's value in `adjustments`, or its default, inside its range.
    pub fn value(&self, adjustments: &Adjustments, setting: &Setting) -> f64 {
        adjustments
            .plugin_settings
            .get(&self.id)
            .and_then(|values| values.get(&setting.key))
            .copied()
            .unwrap_or(setting.default)
            .clamp(setting.min, setting.max)
    }
}

/// Every plugin found when the app started, sorted by name.
#[derive(Debug, Default)]
pub struct Catalog {
    plugins: Vec<Plugin>,
    /// Where user plugins go.
    dir: PathBuf,
}

impl Catalog {
    /// Loads the built-in plugins and those in `dir`. A plugin that can't run is kept, with its
    /// problem, so the window can say why.
    pub fn load(dir: &Path) -> Self {
        let mut found = BTreeMap::new();
        if let Err(e) = gst::init() {
            tracing::error!("🔴 Plugins off: can't start GStreamer: {e}");
            return Self {
                plugins: Vec::new(),
                dir: dir.to_owned(),
            };
        }
        register_elements();
        for (id, text) in BUNDLED {
            match parse(id, text, None) {
                Ok(plugin) => {
                    found.insert(id.to_owned(), plugin);
                }
                Err(e) => tracing::error!("🔴 Built-in plugin {id}: {e:#}"),
            }
        }
        for folder in folders(dir) {
            let id = folder.file_name().unwrap_or_default().to_string_lossy();
            let loaded = validate_name(&id).and_then(|()| {
                let file = folder.join("plugin.toml");
                let text = std::fs::read_to_string(&file)
                    .with_context(|| format!("can't read {}", file.display()))?;
                // Load any compiled elements first, so the pipeline can use them.
                gst::Registry::get().scan_path(&folder);
                parse(&id, &text, Some(folder.clone()))
            });
            match loaded {
                Ok(plugin) => {
                    found.insert(id.into_owned(), plugin);
                }
                Err(e) => tracing::warn!("🟠 Plugin {}: {e:#}", folder.display()),
            }
        }

        let mut plugins: Vec<Plugin> = found.into_values().collect();
        plugins.sort_by(|a, b| a.name.cmp(&b.name));
        for plugin in &mut plugins {
            plugin.problem = check(plugin).err().map(|e| format!("{e:#}"));
            match &plugin.problem {
                Some(problem) => tracing::warn!("🟠 Plugin {} can't run: {problem}", plugin.id),
                None => tracing::debug!("Plugin {} ready", plugin.id),
            }
        }
        Self {
            plugins,
            dir: dir.to_owned(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn all(&self) -> &[Plugin] {
        &self.plugins
    }

    pub fn get(&self, id: &str) -> Option<&Plugin> {
        self.plugins.iter().find(|p| p.id == id)
    }

    /// Checks a value [`Adjustments::set`] just took for `key`, and keeps a plugin setting
    /// inside its range.
    pub fn validate(&self, key: &str, adjustments: &mut Adjustments) -> Result<()> {
        if key == "plugins" {
            for id in &adjustments.plugins {
                match self.get(id) {
                    None => bail!("no plugin {id:?}; installed: {}", self.ids().join(", ")),
                    Some(Plugin {
                        problem: Some(problem),
                        ..
                    }) => bail!("plugin {id} can't run: {problem}"),
                    Some(_) => {}
                }
            }
            return Ok(());
        }
        let Some((id, key)) = key.split_once('.') else {
            return Ok(());
        };
        if id == "crop" {
            return Ok(());
        }
        let Some(plugin) = self.get(id) else {
            bail!("no plugin {id:?}; installed: {}", self.ids().join(", "));
        };
        let Some(setting) = plugin.setting(key) else {
            let keys: Vec<_> = plugin.settings.iter().map(|s| s.key.as_str()).collect();
            bail!(
                "plugin {id} has no setting {key:?}; it has: {}",
                keys.join(", ")
            );
        };
        let value = plugin.value(adjustments, setting);
        adjustments
            .plugin_settings
            .entry(id.to_owned())
            .or_default()
            .insert(key.to_owned(), value);
        Ok(())
    }

    /// The pipeline pieces for the plugins `adjustments` turns on, in order. Skips any that
    /// can't run, so one broken plugin doesn't stop the camera.
    pub fn filters(&self, adjustments: &Adjustments) -> Vec<FilterSpec> {
        self.active(adjustments)
            .map(|plugin| FilterSpec {
                name: plugin.filter_name(),
                description: plugin.pipeline.clone(),
            })
            .collect()
    }

    /// Every (filter, element, property, value) the active plugins' sliders set.
    pub fn properties(&self, adjustments: &Adjustments) -> Vec<(String, String, String, f64)> {
        self.active(adjustments)
            .flat_map(|plugin| {
                plugin.settings.iter().map(|setting| {
                    (
                        plugin.filter_name(),
                        setting.element.clone(),
                        setting.property.clone(),
                        plugin.value(adjustments, setting),
                    )
                })
            })
            .collect()
    }

    fn active<'a>(&'a self, adjustments: &'a Adjustments) -> impl Iterator<Item = &'a Plugin> {
        adjustments
            .plugins
            .iter()
            .filter_map(|id| match self.get(id) {
                Some(plugin) if plugin.problem.is_none() => Some(plugin),
                Some(_) => None,
                None => {
                    tracing::warn!("🟠 Plugin {id} isn't installed: skipped");
                    None
                }
            })
    }

    fn ids(&self) -> Vec<&str> {
        self.plugins.iter().map(|p| p.id.as_str()).collect()
    }
}

fn parse(id: &str, text: &str, folder: Option<PathBuf>) -> Result<Plugin> {
    let mut plugin: Plugin = toml::from_str(text).context("plugin.toml doesn't parse")?;
    plugin.id = id.to_owned();
    plugin.folder = folder;
    Ok(plugin)
}

/// Plugin folders in `dir`, sorted. None when `dir` doesn't exist.
fn folders(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.join("plugin.toml").is_file())
        .collect();
    folders.sort();
    folders
}

/// Builds the plugin's chain once, to find missing elements and settings that point nowhere.
fn check(plugin: &Plugin) -> Result<()> {
    let bin = gst::parse::bin_from_description(&plugin.pipeline, true)
        .map_err(|e| anyhow::anyhow!("{e}"))
        .context("the pipeline doesn't build (a missing GStreamer element?)")?;
    for s in &plugin.settings {
        let element = bin
            .by_name(&s.element)
            .with_context(|| format!("setting {}: no element named {:?}", s.key, s.element))?;
        if element.find_property(&s.property).is_none() {
            bail!(
                "setting {}: {} has no property {:?}",
                s.key,
                s.element,
                s.property
            );
        }
        if !(s.min < s.max && (s.min..=s.max).contains(&s.default)) {
            bail!(
                "setting {}: needs min < max and the default between them",
                s.key
            );
        }
    }
    Ok(())
}

/// Registers the app's own GStreamer elements, once.
pub fn register_elements() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for (name, ty) in [
            ("camerarelaydenoise", denoise::Denoise::static_type()),
            ("camerarelaysoften", soften::Soften::static_type()),
        ] {
            if let Err(e) = gst::Element::register(None, name, gst::Rank::NONE, ty) {
                tracing::error!("🔴 Can't register {name}: {e}");
            }
        }
    });
}

/// The pads of the built-in elements: YUY2 in and out, the format the relay sends.
fn yuy2_templates() -> &'static [gst::PadTemplate] {
    static TEMPLATES: LazyLock<Vec<gst::PadTemplate>> = LazyLock::new(|| {
        let caps = gst_video::VideoCapsBuilder::new()
            .format(gst_video::VideoFormat::Yuy2)
            .build();
        [gst::PadDirection::Src, gst::PadDirection::Sink]
            .into_iter()
            .map(|direction| {
                let name = if direction == gst::PadDirection::Src {
                    "src"
                } else {
                    "sink"
                };
                gst::PadTemplate::new(name, direction, gst::PadPresence::Always, &caps)
                    .expect("valid pad template")
            })
            .collect()
    });
    &TEMPLATES
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_plugins_load_and_a_broken_one_says_why() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("broken");
        std::fs::create_dir(&broken).unwrap();
        std::fs::write(
            broken.join("plugin.toml"),
            "name = \"Broken\"\npipeline = \"no-such-element\"\n",
        )
        .unwrap();

        let catalog = Catalog::load(dir.path());
        for id in ["cleanup", "soften"] {
            let plugin = catalog.get(id).unwrap();
            assert_eq!(plugin.problem, None, "{id}");
        }
        assert!(catalog.get("broken").unwrap().problem.is_some());

        let mut a = Adjustments::default();
        a.set("plugins", "cleanup,broken").unwrap();
        assert_eq!(catalog.filters(&a).len(), 1, "the broken one is skipped");
        a.set("cleanup.strength", "7").unwrap();
        catalog.validate("cleanup.strength", &mut a).unwrap();
        assert_eq!(a.plugin_settings["cleanup"]["strength"], 1.0, "clamped");
        a.set("cleanup.nope", "1").unwrap();
        assert!(catalog.validate("cleanup.nope", &mut a).is_err());
    }
}
