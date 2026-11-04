//! The app's state: config, active profile and the running relay. The D-Bus service, the tray
//! and the settings window all drive this one object. Every change is saved at once, so the app
//! comes back the same after a restart or reboot.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use anyhow::{Context, Result, bail};

use crate::config::{AppConfig, ConfigPaths, DEFAULT_PROFILE, SourceKind, store, validate_name};
use crate::model::{Adjustments, Aspect, VideoSize};
use crate::pipeline::{
    ElementSpec, OutputFormat, PreviewSink, PreviewTap, Relay, RelayEvent, RelaySpec,
};
use crate::plugins::Catalog;

/// Width of the preview frames; the height follows the output's aspect ratio.
const PREVIEW_WIDTH: u32 = 640;

/// Called after any change to the state, on whichever thread made it.
pub type Listener = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub running: bool,
    pub profile: String,
    /// Why the relay last stopped on its own, cleared when it plays again.
    pub error: Option<String>,
}

struct State {
    paths: ConfigPaths,
    config: AppConfig,
    adjustments: Adjustments,
    relay: Option<Relay>,
    /// Counts relays started, so events from an old pipeline are ignored.
    generation: u64,
    last_error: Option<String>,
    preview: Option<PreviewSink>,
    preview_tap: PreviewTap,
}

struct Inner {
    /// Found once, when the app starts.
    catalog: Catalog,
    state: Mutex<State>,
    listeners: Mutex<Vec<Listener>>,
}

#[derive(Clone)]
pub struct Controller {
    inner: Arc<Inner>,
}

impl Controller {
    /// Reads the config and the active profile, creating them with defaults when missing.
    pub fn load(paths: ConfigPaths) -> Result<Self> {
        let (config, adjustments) = load_files(&paths)?;
        let catalog = Catalog::load(&paths.plugins());
        tracing::info!(
            "🌱 Config in {}, profile {}",
            paths.dir.display(),
            config.active_profile
        );
        let state = State {
            paths,
            config,
            adjustments,
            relay: None,
            generation: 0,
            last_error: None,
            preview: None,
            preview_tap: PreviewTap::Output,
        };
        Ok(Self {
            inner: Arc::new(Inner {
                catalog,
                state: Mutex::new(state),
                listeners: Mutex::default(),
            }),
        })
    }

    /// Calls `listener` after every change: relay on or off, profile, adjustments.
    pub fn subscribe(&self, listener: Listener) {
        lock(&self.inner.listeners).push(listener);
    }

    /// The plugins found when the app started.
    pub fn catalog(&self) -> &Catalog {
        &self.inner.catalog
    }

    pub fn config(&self) -> AppConfig {
        self.lock().config.clone()
    }

    pub fn adjustments(&self) -> Adjustments {
        self.lock().adjustments.clone()
    }

    pub fn status(&self) -> Status {
        let state = self.lock();
        Status {
            running: state.relay.is_some(),
            profile: state.config.active_profile.clone(),
            error: state.last_error.clone(),
        }
    }

    /// Starts the relay; does nothing when it already runs. Re-reads the files first, so edits
    /// made by hand while the relay was off take effect.
    pub fn start(&self) -> Result<()> {
        let result = self.start_locked();
        if let Err(e) = &result {
            self.lock().last_error = Some(format!("{e:#}"));
        }
        self.notify();
        result
    }

    fn start_locked(&self) -> Result<()> {
        let mut state = self.lock();
        if state.relay.is_some() {
            return Ok(());
        }
        match load_files(&state.paths) {
            Ok((config, adjustments)) => {
                state.config = config;
                state.adjustments = adjustments;
            }
            Err(e) => tracing::warn!("🟠 Keeping the settings in memory: {e:#}"),
        }

        self.launch(&mut state)
    }

    /// Builds the relay from the state and plays it.
    fn launch(&self, state: &mut State) -> Result<()> {
        let mut spec = relay_spec(&state.config, state.adjustments.aspect);
        spec.filters = self.inner.catalog.filters(&state.adjustments);
        state.generation += 1;
        let generation = state.generation;
        let weak = Arc::downgrade(&self.inner);
        let relay = Relay::new(&spec, &state.adjustments, move |event| {
            on_event(&weak, generation, event);
        })?;
        self.apply_plugins(&relay, &state.adjustments);
        relay.set_preview(state.preview.clone(), state.preview_tap);
        relay.play()?;
        tracing::info!(
            "🟢 Relay on: {} {} → {} at {} fps, profile {}",
            spec.source.factory,
            spec.capture
                .map(|s| s.to_string())
                .unwrap_or_else(|| "any size".into()),
            spec.output.size,
            spec.output.framerate,
            state.config.active_profile
        );
        state.relay = Some(relay);
        state.last_error = None;
        Ok(())
    }

    pub fn stop(&self) {
        let relay = self.lock().relay.take();
        if relay.is_some() {
            drop(relay);
            tracing::info!("🔴 Relay off");
            self.notify();
        }
    }

    /// Rebuilds the running relay, for changes the pipeline can't take live (the output size).
    /// Holds the lock throughout, so no one sees the relay off in between.
    fn restart_locked(&self, state: &mut State) -> Result<()> {
        tracing::info!("🌱 Restarting the relay for the new output size or plugins");
        // The camera must be free before the new pipeline opens it.
        state.relay = None;
        let result = self.launch(state);
        if let Err(e) = &result {
            state.last_error = Some(format!("{e:#}"));
        }
        result
    }

    /// Turns the relay on or off; returns whether it runs now.
    pub fn toggle(&self) -> Result<bool> {
        if self.status().running {
            self.stop();
            Ok(false)
        } else {
            self.start()?;
            Ok(true)
        }
    }

    pub fn profiles(&self) -> Result<Vec<String>> {
        self.lock().paths.profiles().names()
    }

    /// Switches to profile `name` and remembers it. A new name starts as a copy of the current
    /// profile; returns true when it was created.
    pub fn use_profile(&self, name: &str) -> Result<bool> {
        let result = self.use_profile_locked(name);
        self.notify();
        result
    }

    fn use_profile_locked(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        let mut state = self.lock();
        let profiles = state.paths.profiles();
        let (adjustments, created) = match profiles.load(name)? {
            Some(adjustments) => (adjustments, false),
            None => {
                profiles.save(name, &state.adjustments)?;
                tracing::info!(
                    "🌱 Created profile {name} from {}",
                    state.config.active_profile
                );
                (state.adjustments.clone(), true)
            }
        };
        let mut config = state.config.clone();
        config.active_profile = name.to_owned();
        store::save(&state.paths.app_config(), &config)?;
        let restart = state.relay.is_some() && needs_restart(&state.adjustments, &adjustments);
        state.config = config;
        state.adjustments = adjustments;
        tracing::info!("🟢 Profile {name}");
        if restart {
            self.restart_locked(&mut state)?;
        } else if let Some(relay) = &state.relay {
            self.apply(relay, &state.adjustments);
        }
        Ok(created)
    }

    /// Renames profile `from` to `to`, and keeps it active if it was.
    pub fn rename_profile(&self, from: &str, to: &str) -> Result<()> {
        let result = self.rename_profile_locked(from, to);
        self.notify();
        result
    }

    fn rename_profile_locked(&self, from: &str, to: &str) -> Result<()> {
        validate_name(to)?;
        let mut state = self.lock();
        let profiles = state.paths.profiles();
        if !profiles.contains(from) {
            bail!("no profile {from:?}");
        }
        profiles.rename(from, to)?;
        if state.config.active_profile == from {
            let mut config = state.config.clone();
            config.active_profile = to.to_owned();
            store::save(&state.paths.app_config(), &config)?;
            state.config = config;
        }
        tracing::info!("🟢 Profile {from} renamed to {to}");
        Ok(())
    }

    /// Deletes profile `name`. Deleting the active one switches to `default` (or the first one
    /// left) first; the last profile stays.
    pub fn delete_profile(&self, name: &str) -> Result<()> {
        let result = self.delete_profile_locked(name);
        self.notify();
        result
    }

    fn delete_profile_locked(&self, name: &str) -> Result<()> {
        let names = self.profiles()?;
        if !names.iter().any(|n| n == name) {
            bail!("no profile {name:?}");
        }
        let others: Vec<&String> = names.iter().filter(|n| *n != name).collect();
        let Some(&first) = others.first() else {
            bail!("{name} is the only profile, so it stays");
        };
        if self.status().profile == name {
            let next = others
                .iter()
                .find(|n| **n == DEFAULT_PROFILE)
                .unwrap_or(&first);
            self.use_profile_locked(next)?;
        }
        self.lock().paths.profiles().delete(name)?;
        tracing::info!("🟢 Profile {name} deleted");
        Ok(())
    }

    /// Changes one adjustment of the active profile, live, and saves the profile.
    pub fn set(&self, key: &str, value: &str) -> Result<Adjustments> {
        let mut parse_error = None;
        let catalog = &self.inner.catalog;
        let adjustments = self.update_adjustments(|a| {
            let before = a.clone();
            if let Err(e) = a.set(key, value).and_then(|()| catalog.validate(key, a)) {
                *a = before;
                parse_error = Some(e);
            }
        });
        if let Some(e) = parse_error {
            return Err(e);
        }
        let adjustments = adjustments?;
        tracing::info!("🟢 {} {key} = {value}", self.status().profile);
        Ok(adjustments)
    }

    /// Changes the active profile's adjustments with `change`, live, and saves the profile.
    /// Nothing changes when `change` leaves them as they were. A new aspect ratio or a quarter
    /// turn refits the crop to the output's shape; a new aspect ratio or plugin list restarts
    /// the relay.
    pub fn update_adjustments(&self, change: impl FnOnce(&mut Adjustments)) -> Result<Adjustments> {
        let result = self.update_adjustments_locked(change);
        if !matches!(result, Ok((_, false))) {
            self.notify();
        }
        result.map(|(adjustments, _)| adjustments)
    }

    /// Returns the new adjustments and whether they changed.
    fn update_adjustments_locked(
        &self,
        change: impl FnOnce(&mut Adjustments),
    ) -> Result<(Adjustments, bool)> {
        let mut state = self.lock();
        let old = &state.adjustments;
        let mut new = old.clone();
        change(&mut new);
        let mut new = new.clamped();
        if let Some(frame) = frame_size(&state)
            && (new.aspect != old.aspect
                || new.orientation.output_size(frame) != old.orientation.output_size(frame))
        {
            new.crop = new.crop.refit(frame, new.orientation, new.aspect);
        }
        if new == *old {
            return Ok((new, false));
        }
        let restart = state.relay.is_some() && needs_restart(old, &new);
        state
            .paths
            .profiles()
            .save(&state.config.active_profile, &new)?;
        state.adjustments = new.clone();
        if restart {
            self.restart_locked(&mut state)?;
        } else if let Some(relay) = &state.relay {
            self.apply(relay, &new);
        }
        Ok((new, true))
    }

    /// Sends preview frames to `sink` while the relay runs, or stops them with `None` (which
    /// also sets the tap back to the output).
    pub fn set_preview(&self, sink: Option<PreviewSink>) {
        let mut state = self.lock();
        if sink.is_none() {
            state.preview_tap = PreviewTap::Output;
        }
        if let Some(relay) = &state.relay {
            relay.set_preview(sink.clone(), state.preview_tap);
        }
        state.preview = sink;
    }

    /// Chooses what the preview shows: the output, or the whole picture for choosing a crop.
    pub fn set_preview_tap(&self, tap: PreviewTap) {
        let mut state = self.lock();
        state.preview_tap = tap;
        if let Some(relay) = &state.relay {
            relay.set_preview(state.preview.clone(), tap);
        }
    }

    /// The size of the frames the crop works on: the capture size from the config, or what the
    /// camera chose. `None` when neither is known.
    pub fn frame_size(&self) -> Option<VideoSize> {
        frame_size(&self.lock())
    }

    /// Changes the running relay to `adjustments`, live.
    fn apply(&self, relay: &Relay, adjustments: &Adjustments) {
        relay.apply(adjustments);
        self.apply_plugins(relay, adjustments);
    }

    fn apply_plugins(&self, relay: &Relay, adjustments: &Adjustments) {
        for (filter, element, property, value) in self.inner.catalog.properties(adjustments) {
            if let Err(e) = relay.set_filter_property(&filter, &element, &property, value) {
                tracing::warn!("🟠 {e:#}");
            }
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        lock(&self.inner.state)
    }

    /// Tells every listener; call without holding the state lock, so they can read the state.
    fn notify(&self) {
        notify(&self.inner);
    }
}

/// Changes the pipeline can't take live: the output size, or which plugins run.
fn needs_restart(old: &Adjustments, new: &Adjustments) -> bool {
    old.aspect != new.aspect || old.plugins != new.plugins
}

fn frame_size(state: &State) -> Option<VideoSize> {
    let camera = &state.config.camera;
    if camera.capture_width > 0 && camera.capture_height > 0 {
        return Some(VideoSize {
            width: camera.capture_width,
            height: camera.capture_height,
        });
    }
    state.relay.as_ref().and_then(Relay::input_size)
}

/// A mutex is only poisoned by a panic, which the panic hook has logged: carry on with the data.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn notify(inner: &Inner) {
    let listeners = lock(&inner.listeners).clone();
    for listener in listeners {
        listener();
    }
}

fn on_event(inner: &Weak<Inner>, generation: u64, event: RelayEvent) {
    let Some(inner) = inner.upgrade() else { return };
    let stopped = handle_event(&inner, generation, event);
    if let Some(relay) = stopped {
        // Dropped outside the lock: stopping the pipeline can take a moment.
        drop(relay);
        notify(&inner);
    }
}

/// Returns the relay when the event stopped it.
fn handle_event(inner: &Inner, generation: u64, event: RelayEvent) -> Option<Relay> {
    let mut state = lock(&inner.state);
    if state.generation != generation || state.relay.is_none() {
        return None;
    }
    match event {
        RelayEvent::Playing => {
            tracing::debug!("Pipeline playing");
            None
        }
        RelayEvent::Error(message) => {
            tracing::error!("🔴 Relay stopped: {message}");
            state.last_error = Some(message);
            state.relay.take()
        }
        RelayEvent::EndOfStream => {
            tracing::error!("🔴 Relay stopped: the camera stream ended");
            state.last_error = Some("the camera stream ended".to_owned());
            state.relay.take()
        }
    }
}

/// Reads `config.toml` and its active profile. A missing active profile falls back to `default`.
fn load_files(paths: &ConfigPaths) -> Result<(AppConfig, Adjustments)> {
    let mut config: AppConfig = store::load_or_create(&paths.app_config())?;
    let profiles = paths.profiles();
    // `contains` is false for invalid names too.
    if config.active_profile != DEFAULT_PROFILE && !profiles.contains(&config.active_profile) {
        tracing::warn!(
            "🟠 Profile {:?} not found, using {DEFAULT_PROFILE}",
            config.active_profile
        );
        config.active_profile = DEFAULT_PROFILE.to_owned();
        store::save(&paths.app_config(), &config)?;
    }
    let adjustments = profiles
        .load_or_create(&config.active_profile)
        .with_context(|| format!("can't load profile {}", config.active_profile))?;
    Ok((config, adjustments))
}

/// Turns the config file into the pipeline description. The output is the largest size of
/// `aspect` inside the configured one.
pub fn relay_spec(config: &AppConfig, aspect: Aspect) -> RelaySpec {
    let camera = &config.camera;
    let mut source = match camera.source {
        SourceKind::Libcamera => ElementSpec::new("libcamerasrc"),
        SourceKind::V4l2 => ElementSpec::new("v4l2src"),
        SourceKind::Test => ElementSpec::new("videotestsrc").property("is-live", "true"),
    };
    if !camera.device.is_empty() {
        match camera.source {
            SourceKind::Libcamera => source = source.property("camera-name", &camera.device),
            SourceKind::V4l2 => source = source.property("device", &camera.device),
            SourceKind::Test => {}
        }
    }
    for (name, value) in &camera.properties {
        source = source.property(name, value);
    }
    let capture = (camera.capture_width > 0 && camera.capture_height > 0).then_some(VideoSize {
        width: camera.capture_width,
        height: camera.capture_height,
    });

    let output = &config.output;
    let stream_properties = format!(
        "props,media.class=Video/Source,media.role=Camera,node.name=camera-relay,node.description=\"{}\"",
        output.name.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let sink = ElementSpec::new("pipewiresink")
        .property("mode", "provide")
        .property("client-name", "camera-relay")
        .property("stream-properties", stream_properties);

    let size = aspect.fit(VideoSize {
        width: output.width,
        height: output.height,
    });
    // Even height, as most raw formats want.
    let preview_height = (u64::from(PREVIEW_WIDTH) * u64::from(size.height)
        / u64::from(size.width.max(1))) as u32
        & !1;
    RelaySpec {
        source,
        capture,
        output: OutputFormat {
            size,
            framerate: output.framerate,
            format: output.format.clone(),
        },
        sink,
        filters: Vec::new(),
        preview: Some(VideoSize {
            width: PREVIEW_WIDTH,
            height: preview_height.max(2),
        }),
    }
}

/// Fails early with a clear message when the config can't work at all.
pub fn check_config(config: &AppConfig) -> Result<()> {
    let o = &config.output;
    if o.width == 0 || o.height == 0 || o.framerate == 0 {
        bail!("output width, height and framerate must all be above 0 in config.toml");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller() -> (tempfile::TempDir, Controller) {
        let dir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            dir: dir.path().to_owned(),
        };
        (dir, Controller::load(paths).unwrap())
    }

    #[test]
    fn first_load_writes_default_files() {
        let (dir, c) = controller();
        assert!(dir.path().join("config.toml").is_file());
        assert!(dir.path().join("profiles/default.toml").is_file());
        assert_eq!(
            c.status(),
            Status {
                running: false,
                profile: "default".into(),
                error: None
            }
        );
    }

    #[test]
    fn settings_survive_a_restart() {
        let (dir, c) = controller();
        c.set("brightness", "0.2").unwrap();
        assert!(c.use_profile("zoom").unwrap());
        c.set("gamma", "1.5").unwrap();
        assert!(!c.use_profile("default").unwrap());
        c.use_profile("zoom").unwrap();
        drop(c);

        let again = Controller::load(ConfigPaths {
            dir: dir.path().to_owned(),
        })
        .unwrap();
        assert_eq!(again.status().profile, "zoom");
        let a = again.adjustments();
        assert_eq!(
            (a.brightness, a.gamma),
            (0.2, 1.5),
            "zoom started as a copy of default"
        );
        assert_eq!(again.profiles().unwrap(), ["default", "zoom"]);
    }

    #[test]
    fn missing_active_profile_falls_back_to_default() {
        let (dir, c) = controller();
        c.use_profile("teams").unwrap();
        std::fs::remove_file(dir.path().join("profiles/teams.toml")).unwrap();
        let again = Controller::load(ConfigPaths {
            dir: dir.path().to_owned(),
        })
        .unwrap();
        assert_eq!(again.status().profile, "default");
    }

    #[test]
    fn profiles_rename_and_delete() {
        let (dir, c) = controller();
        c.use_profile("zoom").unwrap();
        c.rename_profile("zoom", "meet").unwrap();
        assert_eq!(c.status().profile, "meet", "the active one stays active");
        assert!(c.rename_profile("meet", "default").is_err(), "taken");
        assert!(c.rename_profile("nothing", "x").is_err());

        c.delete_profile("meet").unwrap();
        assert_eq!(c.status().profile, "default");
        assert_eq!(c.profiles().unwrap(), ["default"]);
        assert!(c.delete_profile("default").is_err(), "the last one stays");

        let again = Controller::load(ConfigPaths {
            dir: dir.path().to_owned(),
        })
        .unwrap();
        assert_eq!(again.status().profile, "default");
    }

    #[test]
    fn listeners_hear_each_change_once() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let (_dir, c) = controller();
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        c.subscribe(Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        c.set("brightness", "0.1").unwrap();
        c.set("brightness", "0.1").unwrap(); // no change, no call
        c.use_profile("zoom").unwrap();
        c.stop(); // not running, no call
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn bad_input_changes_nothing() {
        let (_dir, c) = controller();
        assert!(c.set("brightness", "lots").is_err());
        assert!(c.use_profile("../escape").is_err());
        assert_eq!(c.adjustments(), Adjustments::default());
    }

    #[test]
    fn spec_matches_the_legacy_relay() {
        let spec = relay_spec(&AppConfig::default(), Aspect::Wide);
        assert_eq!(spec.source.factory, "libcamerasrc");
        assert_eq!(
            spec.capture,
            Some(VideoSize {
                width: 2560,
                height: 1440
            })
        );
        assert_eq!(
            spec.output.size,
            VideoSize {
                width: 1280,
                height: 720
            }
        );
        let props = &spec.sink.properties;
        assert!(props.contains(&("mode".into(), "provide".into())));
        assert!(
            props
                .iter()
                .any(|(k, v)| k == "stream-properties" && v.contains("\"Built-in Camera\""))
        );
    }

    #[test]
    fn aspect_sets_the_output_and_refits_the_crop() {
        let spec = relay_spec(&AppConfig::default(), Aspect::Square);
        assert_eq!(spec.output.size.to_string(), "720×720");
        assert_eq!(spec.preview.unwrap().to_string(), "640×640");

        let (_dir, c) = controller();
        let a = c
            .update_adjustments(|a| a.aspect = Aspect::Standard)
            .unwrap();
        // The default 2560×1440 capture loses its sides to become 4:3.
        assert_eq!((a.crop.left, a.crop.right), (320, 320));
        let a = c.set("orientation", "rotate90").unwrap();
        let kept = a.crop.to_rect(c.frame_size().unwrap());
        assert_eq!(kept.height / kept.width, 4.0 / 3.0, "{a:?}");
    }
}
