//! `camera-relay run`: the long-running app. Owns the controller, serves it on D-Bus, puts up
//! the tray icon and runs the GLib main loop that delivers pipeline events and drives the UI.

use anyhow::{Context, Result, anyhow};
use gstreamer as gst;
use gstreamer::glib;

use crate::config::{CameraConfig, ConfigPaths, SourceKind};
use crate::controller::{Controller, check_config, relay_spec};
use crate::model::Aspect;
use crate::ui::{self, UiEvent};
use crate::{dbus, pipeline};

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

/// Runs the app until Quit or a signal. `tray: false` skips the tray icon.
pub fn run(start_relay: bool, tray: bool) -> Result<()> {
    let paths = ConfigPaths::from_env()?;
    let controller = Controller::load(paths.clone())?;
    let config = controller.config();
    check_config(&config)?;
    // Before GStreamer and D-Bus start their threads.
    use_tuning_file(&paths, &config.camera);

    // The app id GTK and the desktop see (Wayland app_id, window grouping).
    glib::set_prgname(Some("camera-relay"));
    glib::set_application_name("Camera relay");
    gst::init().context("can't start GStreamer")?;
    if let Err(e) = pipeline::check_elements(&relay_spec(&config, Aspect::default())) {
        tracing::warn!("🟠 {e:#}");
    }

    let main_loop = glib::MainLoop::new(None, false);
    let (ui_sender, ui_receiver) = ui::channel();
    let show_settings = {
        let ui = ui_sender.clone();
        move || ui.send(UiEvent::ShowSettings)
    };
    // Claims the bus name first: a second copy of the app stops here, before showing a tray icon.
    let _connection =
        zbus::block_on(dbus::serve(controller.clone(), show_settings)).map_err(|e| match e {
            zbus::Error::NameTaken => anyhow!("camera-relay is already running"),
            e => anyhow!(e).context("can't register on the D-Bus session bus"),
        })?;
    ui::start(
        controller.clone(),
        main_loop.clone(),
        ui_sender,
        ui_receiver,
        tray,
    );

    for signal in [SIGHUP, SIGINT, SIGTERM] {
        let main_loop = main_loop.clone();
        glib_unix::unix_signal_add(signal, move || {
            tracing::info!("Signal {signal}, quitting");
            main_loop.quit();
            glib::ControlFlow::Break
        });
    }

    tracing::info!(
        "🟢 camera-relay {} ready on D-Bus as {}",
        env!("CARGO_PKG_VERSION"),
        dbus::BUS_NAME
    );
    if start_relay && let Err(e) = controller.start() {
        tracing::error!("🔴 Can't start the relay: {e:#}");
    }

    main_loop.run();
    controller.stop();
    tracing::info!("Stopped");
    Ok(())
}

/// Points libcamera at the soft-ISP tuning file. libcamera reads it from the environment.
fn use_tuning_file(paths: &ConfigPaths, camera: &CameraConfig) {
    const VAR: &str = "LIBCAMERA_SIMPLE_TUNING_FILE";
    if camera.source != SourceKind::Libcamera || camera.tuning_file.is_empty() {
        return;
    }
    if let Some(path) = std::env::var_os(VAR) {
        tracing::info!("Tuning file from {VAR}: {}", path.to_string_lossy());
        return;
    }
    let path = paths.resolve(&camera.tuning_file);
    if !path.is_file() {
        tracing::warn!(
            "🟠 Tuning file {} not found: colours may be off",
            path.display()
        );
        return;
    }
    tracing::info!("Tuning file {}", path.display());
    // SAFETY: called from `run` before GStreamer or zbus start any thread, so nothing reads the
    // environment concurrently.
    unsafe { std::env::set_var(VAR, &path) };
}
