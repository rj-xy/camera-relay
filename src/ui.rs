//! The app's user interface on the GLib main thread: the tray icon and the settings window.
//! Other threads (D-Bus, the tray's menu, GStreamer) never touch either; they send a [`UiEvent`]
//! and this module acts on it here.

use std::rc::Rc;
use std::sync::Arc;

use gtk::glib;
use gtk4 as gtk;

use crate::controller::Controller;
use crate::settings::SettingsWindow;
use crate::tray::{Tray, TrayActions, TrayState};

/// Preview frames waiting for the window: older ones are dropped rather than queued.
const PREVIEW_BACKLOG: usize = 2;

pub enum UiEvent {
    /// The controller's state changed.
    Changed,
    ShowSettings,
    SettingsClosed,
    Toggle,
    UseProfile(String),
    Quit,
}

/// Sends [`UiEvent`]s to the main thread, from any thread.
#[derive(Clone)]
pub struct UiSender(async_channel::Sender<UiEvent>);

impl UiSender {
    pub fn send(&self, event: UiEvent) {
        // Fails only once the UI has stopped, when there's nothing left to tell.
        let _ = self.0.try_send(event);
    }
}

struct Ui {
    controller: Controller,
    main_loop: glib::MainLoop,
    events: UiSender,
    tray: Option<Tray>,
    window: Option<Rc<SettingsWindow>>,
    gtk_ready: bool,
}

/// The channel [`start`] reads; senders can be handed out before the UI starts.
pub fn channel() -> (UiSender, UiReceiver) {
    let (sender, receiver) = async_channel::unbounded();
    (UiSender(sender), UiReceiver(receiver))
}

pub struct UiReceiver(async_channel::Receiver<UiEvent>);

/// Puts up the tray icon (when `tray` is set and a tray host exists) and starts handling
/// events on the default main context. Call on the main thread, before running `main_loop`.
pub fn start(
    controller: Controller,
    main_loop: glib::MainLoop,
    events: UiSender,
    UiReceiver(receiver): UiReceiver,
    tray: bool,
) {
    {
        let events = events.clone();
        controller.subscribe(Arc::new(move || events.send(UiEvent::Changed)));
    }
    let tray = tray
        .then(|| {
            Tray::spawn(
                "camera-relay",
                "Camera relay",
                crate::icon::webcam,
                tray_state(&controller),
                tray_actions(&events),
            )
        })
        .flatten();

    let mut ui = Ui {
        controller,
        main_loop,
        events: events.clone(),
        tray,
        window: None,
        gtk_ready: false,
    };
    glib::spawn_future_local(async move {
        while let Ok(event) = receiver.recv().await {
            // Collapse a burst of changes (a slider drag) into one refresh.
            let mut changed = false;
            for event in
                std::iter::once(event).chain(std::iter::from_fn(|| receiver.try_recv().ok()))
            {
                match event {
                    UiEvent::Changed => changed = true,
                    event => ui.handle(event),
                }
            }
            if changed {
                ui.refresh();
            }
        }
    });
}

impl Ui {
    fn handle(&mut self, event: UiEvent) {
        match event {
            UiEvent::Changed => self.refresh(),
            UiEvent::ShowSettings => self.show_settings(),
            UiEvent::SettingsClosed => {
                self.controller.set_preview(None);
                self.window = None;
            }
            UiEvent::Toggle => {
                if let Err(e) = self.controller.toggle() {
                    tracing::error!("🔴 Can't start the relay: {e:#}");
                }
            }
            UiEvent::UseProfile(name) => {
                if let Err(e) = self.controller.use_profile(&name) {
                    tracing::error!("🔴 {e:#}");
                }
            }
            UiEvent::Quit => {
                if let Some(window) = self.window.take() {
                    window.close();
                }
                if let Some(tray) = &self.tray {
                    tray.shut_down();
                }
                self.main_loop.quit();
            }
        }
    }

    fn refresh(&mut self) {
        let state = tray_state(&self.controller);
        if let Some(tray) = &mut self.tray {
            tray.update(state);
        }
        if let Some(window) = &self.window {
            window.refresh();
        }
    }

    fn show_settings(&mut self) {
        if let Some(window) = &self.window {
            window.present();
            return;
        }
        if !self.gtk_ready {
            if let Err(e) = gtk::init() {
                tracing::error!("🔴 Can't open the settings window: {e}");
                return;
            }
            gtk::Window::set_default_icon_name("camera-web");
            fix_font_rendering();
            self.gtk_ready = true;
        }

        let events = self.events.clone();
        let window = SettingsWindow::new(self.controller.clone(), move || {
            events.send(UiEvent::SettingsClosed);
        });
        window.present();
        let window = Rc::new(window);
        self.start_preview(&window);
        self.window = Some(window);
    }

    /// Carries preview frames from the pipeline's thread to the window. Ends when the window
    /// closes: `set_preview(None)` drops the sender.
    fn start_preview(&self, window: &Rc<SettingsWindow>) {
        let (sender, receiver) = async_channel::bounded(PREVIEW_BACKLOG);
        self.controller.set_preview(Some(Arc::new(move |frame| {
            let _ = sender.try_send(frame);
        })));
        let window = Rc::downgrade(window);
        glib::spawn_future_local(async move {
            while let Ok(frame) = receiver.recv().await {
                let Some(window) = window.upgrade() else {
                    break;
                };
                window.show_frame(frame);
            }
        });
    }
}

/// GTK 4.16+ draws text with unhinted metrics but slightly hinted glyphs. At fractional scales
/// (KDE at 1.6 or 1.15) that cuts the top pixel row off capitals in entries and buttons
/// (GNOME/gtk#8395). Manual rendering hints the metrics too. Set by name: older GTK lacks it.
fn fix_font_rendering() {
    use gtk::prelude::ObjectExt;
    let Some(settings) = gtk::Settings::default() else {
        return;
    };
    let manual = settings
        .find_property("gtk-font-rendering")
        .and_then(|spec| glib::EnumClass::with_type(spec.value_type()))
        .and_then(|class| class.to_value_by_nick("manual"));
    if let Some(manual) = manual {
        settings.set_property_from_value("gtk-font-rendering", &manual);
    }
}

fn tray_state(controller: &Controller) -> TrayState {
    let status = controller.status();
    TrayState {
        running: status.running,
        profile: status.profile,
        profiles: controller.profiles().unwrap_or_default(),
    }
}

fn tray_actions(events: &UiSender) -> TrayActions {
    let toggle = events.clone();
    let use_profile = events.clone();
    let settings = events.clone();
    let quit = events.clone();
    TrayActions {
        toggle: Box::new(move || toggle.send(UiEvent::Toggle)),
        use_profile: Box::new(move |name| use_profile.send(UiEvent::UseProfile(name.to_owned()))),
        settings: Box::new(move || settings.send(UiEvent::ShowSettings)),
        quit: Box::new(move || quit.send(UiEvent::Quit)),
    }
}
