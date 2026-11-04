//! A folder of TOML files addressed by name (`<dir>/<name>.toml`), such as the profiles.

use std::fs;
use std::marker::PhantomData;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::store;

pub struct NamedStore<T> {
    dir: PathBuf,
    _value: PhantomData<fn() -> T>,
}

impl<T> NamedStore<T>
where
    T: DeserializeOwned + Serialize + Default,
{
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            _value: PhantomData,
        }
    }

    pub fn contains(&self, name: &str) -> bool {
        self.path(name).is_ok_and(|p| p.is_file())
    }

    /// Reads `name`, creating it with defaults when missing.
    pub fn load_or_create(&self, name: &str) -> Result<T> {
        store::load_or_create(&self.path(name)?)
    }

    /// Reads `name`; `Ok(None)` when there's no such file.
    pub fn load(&self, name: &str) -> Result<Option<T>> {
        store::load(&self.path(name)?)
    }

    pub fn save(&self, name: &str, value: &T) -> Result<()> {
        store::save(&self.path(name)?, value)
    }

    /// Renames `from` to `to`; fails when `to` exists already.
    pub fn rename(&self, from: &str, to: &str) -> Result<()> {
        let (from_path, to_path) = (self.path(from)?, self.path(to)?);
        if to_path.exists() {
            bail!("{to} exists already");
        }
        fs::rename(&from_path, &to_path)
            .with_context(|| format!("can't rename {}", from_path.display()))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        fs::remove_file(&path).with_context(|| format!("can't delete {}", path.display()))
    }

    /// All names in the folder, sorted.
    pub fn names(&self) -> Result<Vec<String>> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e).with_context(|| format!("can't list {}", self.dir.display())),
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let stem = name.strip_suffix(".toml")?;
                validate_name(stem).is_ok().then(|| stem.to_owned())
            })
            .collect();
        names.sort();
        Ok(names)
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        validate_name(name)?;
        Ok(self.dir.join(format!("{name}.toml")))
    }
}

/// Names become file names: letters, digits, `-` and `_` only, so no `..` or `/` can sneak in.
pub fn validate_name(name: &str) -> Result<()> {
    let ok_char = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_';
    if name.is_empty() || name.len() > 64 || !name.chars().all(ok_char) {
        bail!("{name:?} isn't a valid name: use 1 to 64 letters, digits, '-' or '_'");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Adjustments;

    #[test]
    fn rejects_path_tricks() {
        for bad in ["", "..", "a/b", "../etc", "with space", "dot.toml"] {
            assert!(validate_name(bad).is_err(), "{bad:?} should be rejected");
        }
        for good in ["default", "Zoom", "teams-2", "my_profile"] {
            validate_name(good).unwrap();
        }
    }

    #[test]
    fn lists_saved_names_sorted() {
        let dir = tempfile::tempdir().unwrap();
        let store = NamedStore::<Adjustments>::new(dir.path().join("profiles"));
        assert!(store.names().unwrap().is_empty());
        store.save("zoom", &Adjustments::default()).unwrap();
        store.load_or_create("default").unwrap();
        fs::write(dir.path().join("profiles/notes.txt"), "ignored").unwrap();
        assert_eq!(store.names().unwrap(), ["default", "zoom"]);
        assert!(store.contains("zoom"));
        assert!(!store.contains("teams"));
        assert_eq!(store.load("teams").unwrap(), None);
    }
}
