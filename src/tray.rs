//! The tray icon (StatusNotifierItem): shows whether the relay is on, toggles it on click, and
//! offers profiles, Settings… and Quit in its menu. Knows nothing about the rest of the app: it
//! shows a [`TrayState`] and calls back through [`TrayActions`].

use std::sync::Arc;

use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, RadioGroup, RadioItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip};

/// What the icon and the menu show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayState {
    pub running: bool,
    pub profile: String,
    pub profiles: Vec<String>,
}

/// Draws the icon at a size in pixels, on or off, as ARGB32 bytes in network order.
pub type DrawIcon = fn(size: u32, on: bool) -> Vec<u8>;

/// Icon sizes handed to the tray host, which picks the one that fits its panel.
const ICON_SIZES: [u32; 4] = [22, 32, 48, 64];

/// What the menu does. Called on the tray's own thread: hand the work on, don't block.
pub struct TrayActions {
    pub toggle: Box<dyn Fn() + Send + Sync>,
    pub use_profile: Box<dyn Fn(&str) + Send + Sync>,
    pub settings: Box<dyn Fn() + Send + Sync>,
    pub quit: Box<dyn Fn() + Send + Sync>,
}

/// A tray icon on screen. Dropping it leaves the icon up; call [`Tray::shut_down`] to remove it.
pub struct Tray {
    handle: ksni::blocking::Handle<Item>,
    shown: TrayState,
}

impl Tray {
    /// Puts the icon up. `None` when there is no tray host (such as GNOME without the
    /// AppIndicator extension); the reason is logged.
    pub fn spawn(
        id: &str,
        title: &str,
        draw: DrawIcon,
        state: TrayState,
        actions: TrayActions,
    ) -> Option<Self> {
        let icons = |on| {
            ICON_SIZES
                .iter()
                .map(|&size| ksni::Icon {
                    width: size as i32,
                    height: size as i32,
                    data: draw(size, on),
                })
                .collect()
        };
        let item = Item {
            id: id.to_owned(),
            title: title.to_owned(),
            icons: Arc::new([icons(false), icons(true)]),
            state: state.clone(),
            actions: Arc::new(actions),
        };
        match item.spawn() {
            Ok(handle) => Some(Self {
                handle,
                shown: state,
            }),
            Err(e) => {
                tracing::warn!(
                    "🟠 No tray icon ({e}). Use `camera-relay settings` or the other commands instead"
                );
                None
            }
        }
    }

    /// Shows `state`; does nothing when it is what the icon shows already.
    pub fn update(&mut self, state: TrayState) {
        if state == self.shown {
            return;
        }
        self.shown = state.clone();
        self.handle.update(move |item| item.state = state);
    }

    pub fn shut_down(&self) {
        let _ = self.handle.shutdown();
    }
}

struct Item {
    id: String,
    title: String,
    /// Off, then on.
    icons: Arc<[Vec<ksni::Icon>; 2]>,
    state: TrayState,
    actions: Arc<TrayActions>,
}

impl ksni::Tray for Item {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.title.clone()
    }

    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        self.icons[usize::from(self.state.running)].clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let state = if self.state.running { "on" } else { "off" };
        ToolTip {
            title: self.title.clone(),
            description: format!("{state} · profile {}", self.state.profile),
            ..Default::default()
        }
    }

    /// Left click.
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.actions.toggle)();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let profiles = &self.state.profiles;
        let selected = profiles
            .iter()
            .position(|p| *p == self.state.profile)
            .unwrap_or(usize::MAX);
        vec![
            CheckmarkItem {
                label: "Camera on".into(),
                checked: self.state.running,
                activate: Box::new(|item: &mut Self| (item.actions.toggle)()),
                ..Default::default()
            }
            .into(),
            SubMenu {
                label: "Profile".into(),
                submenu: vec![
                    RadioGroup {
                        selected,
                        select: Box::new(|item: &mut Self, index| {
                            if let Some(name) = item.state.profiles.get(index) {
                                (item.actions.use_profile)(name);
                            }
                        }),
                        options: profiles
                            .iter()
                            .map(|name| RadioItem {
                                label: name.clone(),
                                ..Default::default()
                            })
                            .collect(),
                    }
                    .into(),
                ],
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: "Settings…".into(),
                icon_name: "configure".into(),
                activate: Box::new(|item: &mut Self| (item.actions.settings)()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit".into(),
                icon_name: "application-exit".into(),
                activate: Box::new(|item: &mut Self| (item.actions.quit)()),
                ..Default::default()
            }
            .into(),
        ]
    }
}
