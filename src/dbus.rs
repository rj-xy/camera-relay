//! The session-bus service the app offers, and the client proxy the CLI uses.

use zbus::{fdo, interface, proxy};

use crate::controller::Controller;

pub const BUS_NAME: &str = "io.github.rj_xy.CameraRelay";
pub const OBJECT_PATH: &str = "/io/github/rj_xy/CameraRelay";

/// Serves `controller` on the session bus. `show_settings` opens the settings window. Fails when
/// another instance already owns the name.
pub async fn serve(
    controller: Controller,
    show_settings: impl Fn() + Send + Sync + 'static,
) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()
        .name(BUS_NAME)
        // zbus lets a newer instance take the name by default; one instance at a time instead.
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .serve_at(
            OBJECT_PATH,
            Service {
                controller,
                show_settings: Box::new(show_settings),
            },
        )
        .build()
        .await
}

struct Service {
    controller: Controller,
    show_settings: Box<dyn Fn() + Send + Sync>,
}

fn failed(e: anyhow::Error) -> fdo::Error {
    fdo::Error::Failed(format!("{e:#}"))
}

#[interface(name = "io.github.rj_xy.CameraRelay1")]
impl Service {
    fn start(&self) -> fdo::Result<()> {
        self.controller.start().map_err(failed)
    }

    fn stop(&self) {
        self.controller.stop();
    }

    /// Returns whether the relay runs now.
    fn toggle(&self) -> fdo::Result<bool> {
        self.controller.toggle().map_err(failed)
    }

    /// Switches profile, creating it as a copy of the current one when new. Returns whether it was created.
    fn use_profile(&self, name: &str) -> fdo::Result<bool> {
        self.controller.use_profile(name).map_err(failed)
    }

    fn rename_profile(&self, from: &str, to: &str) -> fdo::Result<()> {
        self.controller.rename_profile(from, to).map_err(failed)
    }

    /// Deletes a profile; deleting the active one switches to `default` first.
    fn delete_profile(&self, name: &str) -> fdo::Result<()> {
        self.controller.delete_profile(name).map_err(failed)
    }

    /// Changes one setting of the active profile, such as `brightness` to `0.1`.
    fn set(&self, key: &str, value: &str) -> fdo::Result<()> {
        self.controller.set(key, value).map(|_| ()).map_err(failed)
    }

    /// Opens the settings window, or brings it to the front.
    fn show_settings(&self) {
        (self.show_settings)();
    }

    /// The active profile's settings, as TOML.
    fn adjustments(&self) -> fdo::Result<String> {
        toml::to_string_pretty(&self.controller.adjustments()).map_err(|e| failed(e.into()))
    }

    #[zbus(property)]
    fn running(&self) -> bool {
        self.controller.status().running
    }

    #[zbus(property)]
    fn active_profile(&self) -> String {
        self.controller.status().profile
    }

    /// Why the relay last stopped on its own; empty when it didn't.
    #[zbus(property)]
    fn last_error(&self) -> String {
        self.controller.status().error.unwrap_or_default()
    }

    #[zbus(property)]
    fn profiles(&self) -> fdo::Result<Vec<String>> {
        self.controller.profiles().map_err(failed)
    }
}

#[proxy(
    interface = "io.github.rj_xy.CameraRelay1",
    default_service = "io.github.rj_xy.CameraRelay",
    default_path = "/io/github/rj_xy/CameraRelay"
)]
pub trait CameraRelay {
    async fn start(&self) -> zbus::Result<()>;
    async fn stop(&self) -> zbus::Result<()>;
    async fn toggle(&self) -> zbus::Result<bool>;
    async fn use_profile(&self, name: &str) -> zbus::Result<bool>;
    async fn rename_profile(&self, from: &str, to: &str) -> zbus::Result<()>;
    async fn delete_profile(&self, name: &str) -> zbus::Result<()>;
    async fn set(&self, key: &str, value: &str) -> zbus::Result<()>;
    async fn adjustments(&self) -> zbus::Result<String>;
    async fn show_settings(&self) -> zbus::Result<()>;

    #[zbus(property)]
    fn running(&self) -> zbus::Result<bool>;
    #[zbus(property)]
    fn active_profile(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn last_error(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn profiles(&self) -> zbus::Result<Vec<String>>;
}
