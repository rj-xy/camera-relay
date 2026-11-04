//! Generic TOML file storage: load with defaults, save atomically.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Reads `path`. A missing file is created with `T::default()`. A file that doesn't parse is
/// renamed to `<name>.broken-<unix time>` and replaced with defaults, so a typo never stops the app.
pub fn load_or_create<T>(path: &Path) -> Result<T>
where
    T: DeserializeOwned + Serialize + Default,
{
    match load(path) {
        Ok(Some(value)) => Ok(value),
        Ok(None) => {
            let value = T::default();
            save(path, &value)?;
            tracing::info!("🌱 Created {}", path.display());
            Ok(value)
        }
        Err(e) => {
            let backup = broken_name(path);
            fs::rename(path, &backup)
                .with_context(|| format!("can't move the broken file {} aside", path.display()))?;
            tracing::warn!(
                "🟠 {e:#}. Moved it to {} and started from defaults",
                backup.display()
            );
            let value = T::default();
            save(path, &value)?;
            Ok(value)
        }
    }
}

/// Reads `path`; `Ok(None)` when the file doesn't exist.
pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("can't read {}", path.display())),
    };
    let value = toml::from_str(&text).with_context(|| format!("{} isn't valid", path.display()))?;
    Ok(Some(value))
}

/// Writes `value` to `path` through a temporary file and a rename, so a crash never leaves half a file.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("can't create {}", dir.display()))?;
    }
    let text = toml::to_string_pretty(value)
        .with_context(|| format!("can't serialise {}", path.display()))?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, text).with_context(|| format!("can't write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("can't replace {}", path.display()))?;
    Ok(())
}

fn broken_name(path: &Path) -> PathBuf {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".broken-{secs}"));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    #[serde(default)]
    struct Sample {
        name: String,
        level: u32,
    }

    #[test]
    fn creates_missing_file_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/sample.toml");
        let value: Sample = load_or_create(&path).unwrap();
        assert_eq!(value, Sample::default());
        assert!(path.exists());
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.toml");
        let value = Sample {
            name: "test".into(),
            level: 3,
        };
        save(&path, &value).unwrap();
        assert_eq!(load::<Sample>(&path).unwrap(), Some(value));
        assert!(!path.with_extension("toml.tmp").exists());
    }

    #[test]
    fn broken_file_is_kept_aside_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.toml");
        fs::write(&path, "level = \"not a number\"").unwrap();
        let value: Sample = load_or_create(&path).unwrap();
        assert_eq!(value, Sample::default());
        let backups: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("sample.toml.broken-")
            })
            .collect();
        assert_eq!(backups.len(), 1);
    }
}
