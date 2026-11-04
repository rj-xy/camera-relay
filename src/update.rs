//! `camera-relay update`: swaps this binary for the latest GitHub release.
//! Uses `curl`, `sha256sum` and `tar` (the installer needs them too) rather than an HTTP crate.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

const REPO: &str = "rj-xy/camera-relay";

pub fn run() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let tag = latest_tag()?;
    let latest = parse_version(&tag).with_context(|| format!("odd release tag {tag}"))?;
    if parse_version(current).is_some_and(|current| current >= latest) {
        println!("🟢 Up to date: v{current}");
        return Ok(());
    }

    let exe = std::env::current_exe()
        .and_then(|exe| exe.canonicalize())
        .context("can't find my own executable")?;
    println!("🌱 Updating v{current} → {tag} ({})", exe.display());
    let dir = std::env::temp_dir().join(format!("camera-relay-update-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let result = download_and_replace(&tag, &dir, &exe);
    let _ = std::fs::remove_dir_all(&dir);
    result?;

    println!("🟢 Updated to {tag}. Restart the app to use it: tray menu → Quit, then camera-relay");
    Ok(())
}

/// The tag `releases/latest` redirects to, read from the redirect so no API token or JSON is needed.
fn latest_tag() -> Result<String> {
    let url = format!("https://github.com/{REPO}/releases/latest");
    let output = Command::new("curl")
        .args(["-fsSL", "-o", "/dev/null", "-w", "%{url_effective}", &url])
        .output()
        .context("can't run curl: install it first")?;
    if !output.status.success() {
        bail!(
            "can't reach {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let effective = String::from_utf8_lossy(&output.stdout);
    match effective.rsplit_once("/releases/tag/") {
        Some((_, tag)) => Ok(tag.to_owned()),
        None => bail!("no release published yet at {url}"),
    }
}

fn download_and_replace(tag: &str, dir: &Path, exe: &Path) -> Result<()> {
    let asset = asset_name();
    let base = format!("https://github.com/{REPO}/releases/download/{tag}");
    for file in [asset.clone(), format!("{asset}.sha256")] {
        run_in(
            dir,
            Command::new("curl").args(["-fsSLO", &format!("{base}/{file}")]),
        )
        .with_context(|| format!("can't download {file}: is there a build for this machine?"))?;
    }
    run_in(
        dir,
        Command::new("sha256sum").args(["--quiet", "-c", &format!("{asset}.sha256")]),
    )
    .context("the download doesn't match its checksum")?;
    run_in(dir, Command::new("tar").args(["-xzf", &asset]))?;

    // Copy next to the target, then rename over it: atomic, and fine while the old one runs.
    let staged = exe.with_file_name(".camera-relay.new");
    std::fs::copy(dir.join("camera-relay"), &staged)
        .and_then(|_| std::fs::rename(&staged, exe))
        .with_context(|| {
            format!(
                "can't replace {}: if a package manager installed it, update it there",
                exe.display()
            )
        })
}

fn run_in(dir: &Path, command: &mut Command) -> Result<()> {
    let status = command.current_dir(dir).status()?;
    if !status.success() {
        bail!("{command:?} failed ({status})");
    }
    Ok(())
}

fn asset_name() -> String {
    format!("camera-relay-{}-linux.tar.gz", std::env::consts::ARCH)
}

/// `v1.2.3` or `1.2.3` as numbers, so 0.10.0 sorts after 0.9.0.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let mut parts = text.strip_prefix('v').unwrap_or(text).split('.');
    let mut next = || parts.next()?.parse().ok();
    let version = (next()?, next()?, next()?);
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0"), Some((0, 10, 0)));
        assert!(parse_version("v0.10.0") > parse_version("v0.9.9"));
    }

    #[test]
    fn odd_tags_are_rejected() {
        for tag in ["latest", "v1.2", "v1.2.3.4", "v1.2.x", ""] {
            assert_eq!(parse_version(tag), None, "{tag}");
        }
    }
}
