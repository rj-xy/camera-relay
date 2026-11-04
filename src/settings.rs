//! The settings window: on/off switch, profiles, picture sliders, aspect ratio, a crop tool,
//! orientation, plugins and a live preview. GTK lives on the main thread only; [`crate::ui`] opens this on
//! demand and feeds it changes and preview frames.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{cairo, gdk, glib};
use gtk4 as gtk;

use crate::autostart::{self, Mode};
use crate::controller::Controller;
use crate::model::{Adjustments, Aspect, Crop, Orientation, Rect, VideoSize};
use crate::pipeline::{PreviewFrame, PreviewTap};

const PREVIEW_PAGE: &str = "preview";
const OFF_PAGE: &str = "off";

/// How close to a corner of the crop a drag grabs that corner, in screen pixels.
const CORNER_GRAB: f64 = 14.0;
/// A drag shorter than this, in screen pixels, is a click: the crop stays as it was.
const MIN_DRAG: f64 = 6.0;
/// The smallest crop, as a share of the largest one.
const MIN_CROP_SHARE: f64 = 0.125;

/// Labels for [`Orientation::ALL`], in the same order.
const ORIENTATION_LABELS: [&str; 6] = [
    "Normal",
    "Rotate 90° right",
    "Rotate 180°",
    "Rotate 90° left",
    "Mirror",
    "Upside down",
];

pub struct SettingsWindow {
    window: gtk::Window,
    w: Rc<Widgets>,
}

/// Everything [`SettingsWindow::refresh`] updates.
struct Widgets {
    controller: Controller,
    /// True while the window copies the state into its widgets, so their change handlers don't
    /// write it straight back.
    syncing: Cell<bool>,
    switch: gtk::Switch,
    profiles: gtk::StringList,
    profile: gtk::DropDown,
    brightness: gtk::Scale,
    contrast: gtk::Scale,
    hue: gtk::Scale,
    saturation: gtk::Scale,
    gamma: gtk::Scale,
    aspect: gtk::DropDown,
    crop_mode: gtk::ToggleButton,
    crop_reset: gtk::Button,
    crop_hint: gtk::Label,
    orientation: gtk::DropDown,
    plugins: Vec<PluginRow>,
    preview: gtk::Stack,
    picture: gtk::Picture,
    /// Drawn over the preview in crop mode: the crop, and the drags that change it.
    crop_area: gtk::DrawingArea,
    /// The crop on show, in pixels of the turned frame (as the output sees it).
    crop_rect: Cell<Option<Rect>>,
    drag: Cell<Option<Drag>>,
    status: gtk::Label,
}

/// A plugin's switch and sliders.
struct PluginRow {
    id: String,
    switch: gtk::Switch,
    /// Each setting's key and slider.
    sliders: Vec<(String, gtk::Scale)>,
}

/// What a drag on the preview is doing to the crop.
#[derive(Clone, Copy, Debug)]
enum Drag {
    /// Moving the whole crop, which started here.
    Move(Rect),
    /// Drawing the crop out from this fixed corner.
    Corner((f64, f64)),
}

/// Where the turned frame sits on the crop overlay, which matches the preview picture.
#[derive(Clone, Copy, Debug)]
struct View {
    frame: VideoSize,
    orientation: Orientation,
    aspect: Aspect,
    /// The frame's size after turning, in frame pixels.
    width: f64,
    height: f64,
    /// Screen pixels per frame pixel.
    scale: f64,
    /// Top left of the picture on screen; the picture keeps its shape and centres.
    left: f64,
    top: f64,
}

impl View {
    fn to_frame(self, (x, y): (f64, f64)) -> (f64, f64) {
        ((x - self.left) / self.scale, (y - self.top) / self.scale)
    }

    fn to_screen(self, r: Rect) -> Rect {
        Rect {
            x: self.left + r.x * self.scale,
            y: self.top + r.y * self.scale,
            width: r.width * self.scale,
            height: r.height * self.scale,
        }
    }

    fn whole(self) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: self.width,
            height: self.height,
        }
    }

    /// Index of the crop corner near frame point `p`, if any.
    fn corner_at(self, rect: Rect, p: (f64, f64)) -> Option<usize> {
        let grab = CORNER_GRAB / self.scale;
        rect.corners()
            .iter()
            .position(|c| (c.0 - p.0).abs() <= grab && (c.1 - p.1).abs() <= grab)
    }

    /// The crop that keeps `rect`, snapped to the exact aspect ratio.
    fn crop(self, rect: Rect) -> Crop {
        let turned = VideoSize {
            width: self.width as u32,
            height: self.height as u32,
        };
        let rect = self.aspect.snap(rect, turned);
        Crop::from_rect(
            self.orientation.rect_to_source(self.frame, rect),
            self.frame,
        )
    }
}

impl SettingsWindow {
    /// Builds the window, hidden. `on_close` runs when the user closes it.
    pub fn new(controller: Controller, on_close: impl Fn() + 'static) -> Self {
        let window = gtk::Window::builder()
            .title("Camera relay")
            .default_width(1040)
            .default_height(540)
            .build();

        let switch = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .tooltip_text("Camera on or off")
            .build();
        let app_menu = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Start on login")
            .build();
        let header = gtk::HeaderBar::new();
        header.pack_start(&switch);
        header.pack_end(&app_menu);
        window.set_titlebar(Some(&header));

        let profiles = gtk::StringList::new(&[]);
        let profile = gtk::DropDown::builder()
            .model(&profiles)
            .hexpand(true)
            .tooltip_text("Profile")
            .build();
        let new_profile = gtk::MenuButton::builder()
            .icon_name("list-add-symbolic")
            .tooltip_text("New profile, copied from this one")
            .build();
        let profile_menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Rename or delete this profile")
            .build();
        let reset = gtk::Button::builder()
            .label("Reset")
            .tooltip_text(
                "Back to the neutral picture, orientation and crop, plugins off; keeps the aspect ratio",
            )
            .build();
        let profile_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        profile_row.set_margin_bottom(12);
        profile_row.append(&profile);
        profile_row.append(&new_profile);
        profile_row.append(&profile_menu);
        profile_row.append(&reset);

        let picture_grid = grid();
        let brightness = slider(&picture_grid, 0, "Brightness", -1.0, 1.0, 0.0);
        let contrast = slider(&picture_grid, 1, "Contrast", 0.0, 2.0, 1.0);
        let saturation = slider(&picture_grid, 2, "Saturation", 0.0, 2.0, 1.0);
        let hue = slider(&picture_grid, 3, "Hue", -1.0, 1.0, 0.0);
        let gamma = slider(&picture_grid, 4, "Gamma", 0.2, 3.0, 1.0);

        let framing_grid = grid();
        let output = controller.config().output;
        let output_box = VideoSize {
            width: output.width,
            height: output.height,
        };
        let aspect_labels: Vec<String> = Aspect::ALL
            .iter()
            .map(|a| format!("{a}  ({})", a.fit(output_box)))
            .collect();
        let aspect_labels: Vec<&str> = aspect_labels.iter().map(String::as_str).collect();
        let aspect = gtk::DropDown::from_strings(&aspect_labels);
        aspect.set_hexpand(true);
        aspect.set_tooltip_text(Some("Shape and size the virtual camera sends"));
        framing_grid.attach(&label("Aspect ratio"), 0, 0, 1, 1);
        framing_grid.attach(&aspect, 1, 0, 1, 1);
        let crop_mode = gtk::ToggleButton::builder()
            .label("Select area")
            .hexpand(true)
            .tooltip_text("Choose the part of the picture to send, on the preview")
            .build();
        let crop_reset = gtk::Button::builder()
            .label("Whole picture")
            .hexpand(true)
            .tooltip_text("The largest area of this aspect ratio, centred")
            .build();
        let crop_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        crop_row.set_homogeneous(true);
        crop_row.append(&crop_mode);
        crop_row.append(&crop_reset);
        framing_grid.attach(&label("Crop"), 0, 1, 1, 1);
        framing_grid.attach(&crop_row, 1, 1, 1, 1);
        let crop_hint = gtk::Label::builder()
            .label("Drag on the preview to pick an area. Drag inside it to move it, or a corner to resize it.")
            .wrap(true)
            .xalign(0.0)
            // Wraps to the column's width instead of widening it.
            .max_width_chars(1)
            .visible(false)
            .build();
        crop_hint.add_css_class("dim-label");
        framing_grid.attach(&crop_hint, 1, 2, 1, 1);
        let orientation = gtk::DropDown::from_strings(&ORIENTATION_LABELS);
        orientation.set_hexpand(true);
        framing_grid.attach(&label("Orientation"), 0, 3, 1, 1);
        framing_grid.attach(&orientation, 1, 3, 1, 1);

        let (plugins_grid, plugins) = plugins_grid(&controller);

        // One section at a time keeps the column short, so the preview gets the height.
        let sections = gtk::Stack::builder()
            .vhomogeneous(false)
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        sections.add_titled(&picture_grid, Some("picture"), "Picture");
        sections.add_titled(&framing_grid, Some("framing"), "Framing");
        sections.add_titled(&plugins_grid, Some("plugins"), "Plugins");
        let tabs = gtk::StackSwitcher::builder()
            .stack(&sections)
            .hexpand(true)
            .margin_bottom(6)
            .build();

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 6);
        controls.append(&heading("Profile"));
        controls.append(&profile_row);
        controls.append(&tabs);
        controls.append(&sections);
        let scroller = gtk::ScrolledWindow::builder()
            .child(&controls)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .build();

        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .hexpand(true)
            .vexpand(true)
            .build();
        let crop_area = gtk::DrawingArea::builder().visible(false).build();
        crop_area.set_cursor_from_name(Some("crosshair"));
        let overlay = gtk::Overlay::builder().child(&picture).build();
        overlay.add_overlay(&crop_area);
        let off = gtk::Label::new(Some("The camera is off"));
        off.add_css_class("dim-label");
        let preview = gtk::Stack::new();
        preview.add_named(&overlay, Some(PREVIEW_PAGE));
        preview.add_named(&off, Some(OFF_PAGE));
        preview.set_size_request(480, 270);

        let body = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        body.set_margin_top(12);
        body.set_margin_bottom(6);
        body.set_margin_start(12);
        body.set_margin_end(12);
        body.append(&scroller);
        body.append(&preview);
        let status = gtk::Label::builder()
            .xalign(0.0)
            .wrap(true)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(8)
            .build();
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        page.append(&body);
        page.append(&status);
        window.set_child(Some(&page));

        let w = Rc::new(Widgets {
            controller,
            syncing: Cell::new(false),
            switch,
            profiles,
            profile,
            brightness,
            contrast,
            hue,
            saturation,
            gamma,
            aspect,
            crop_mode,
            crop_reset,
            crop_hint,
            orientation,
            plugins,
            preview,
            picture,
            crop_area,
            crop_rect: Cell::new(None),
            drag: Cell::new(None),
            status,
        });
        connect(&w, &new_profile, &reset);
        profile_menu.set_popover(Some(&profile_popover(&w)));
        app_menu.set_popover(Some(&app_popover(&w)));
        connect_crop(&w);
        window.connect_close_request(move |_| {
            on_close();
            glib::Propagation::Proceed
        });

        let this = Self { window, w };
        this.refresh();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn close(&self) {
        self.window.close();
    }

    /// Copies the app's current state into the widgets.
    pub fn refresh(&self) {
        let w = &self.w;
        let controller = &w.controller;
        let status = controller.status();
        let a = controller.adjustments();
        w.syncing.set(true);

        w.switch.set_active(status.running);
        let names = controller.profiles().unwrap_or_default();
        let current: Vec<String> = (0..w.profiles.n_items())
            .filter_map(|i| w.profiles.string(i).map(Into::into))
            .collect();
        if current != names {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            w.profiles.splice(0, w.profiles.n_items(), &names);
        }
        if let Some(index) = names.iter().position(|n| *n == status.profile) {
            w.profile.set_selected(index as u32);
        }

        set_value(&w.brightness, a.brightness);
        set_value(&w.contrast, a.contrast);
        set_value(&w.hue, a.hue);
        set_value(&w.saturation, a.saturation);
        set_value(&w.gamma, a.gamma);
        if let Some(index) = Aspect::ALL.iter().position(|x| *x == a.aspect) {
            w.aspect.set_selected(index as u32);
        }
        let frame = controller.frame_size();
        w.crop_reset.set_sensitive(frame.is_some());
        w.crop_mode.set_sensitive(status.running && frame.is_some());
        if !status.running {
            // Fires its toggled handler, which puts the preview back on the output.
            w.crop_mode.set_active(false);
        }
        if w.drag.get().is_none() {
            w.show_crop();
        }
        if let Some(index) = Orientation::ALL.iter().position(|o| *o == a.orientation) {
            w.orientation.set_selected(index as u32);
        }
        for row in &w.plugins {
            let Some(plugin) = controller.catalog().get(&row.id) else {
                continue;
            };
            let on = plugin.problem.is_none() && a.plugins.contains(&row.id);
            row.switch.set_active(on);
            for (key, scale) in &row.sliders {
                if let Some(setting) = plugin.setting(key) {
                    set_value(scale, plugin.value(&a, setting));
                }
                scale.set_sensitive(on);
            }
        }

        w.preview.set_visible_child_name(if status.running {
            PREVIEW_PAGE
        } else {
            OFF_PAGE
        });
        if !status.running {
            w.picture.set_paintable(None::<&gdk::Paintable>);
        }
        match status.error {
            Some(error) => w.status.set_text(&format!("🔴 {error}")),
            None => w.status.set_text(""),
        }

        w.syncing.set(false);
    }

    pub fn show_frame(&self, frame: PreviewFrame) {
        let (Ok(width), Ok(height)) = (i32::try_from(frame.width), i32::try_from(frame.height))
        else {
            return;
        };
        let texture = gdk::MemoryTexture::new(
            width,
            height,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_owned(frame.data),
            frame.stride,
        );
        self.w.picture.set_paintable(Some(&texture));
    }
}

/// Writes a slider's value into the adjustments.
type SetValue = fn(&mut Adjustments, f64);

/// Wires each widget's change to the controller.
fn connect(w: &Rc<Widgets>, new_profile: &gtk::MenuButton, reset: &gtk::Button) {
    let weak = Rc::downgrade(w);
    w.switch.connect_state_set(move |_, on| {
        if let Some(w) = weak.upgrade()
            && !w.syncing.get()
        {
            let result = if on {
                w.controller.start()
            } else {
                w.controller.stop();
                Ok(())
            };
            w.report(result);
        }
        glib::Propagation::Proceed
    });

    let weak = Rc::downgrade(w);
    w.profile.connect_selected_notify(move |dropdown| {
        let Some(w) = weak.upgrade() else { return };
        if w.syncing.get() {
            return;
        }
        if let Some(name) = w.profiles.string(dropdown.selected()) {
            w.report(w.controller.use_profile(&name).map(|_| ()));
        }
    });

    new_profile.set_popover(Some(&new_profile_popover(w)));

    let weak = Rc::downgrade(w);
    reset.connect_clicked(move |_| {
        let Some(w) = weak.upgrade() else { return };
        let frame = w.controller.frame_size();
        w.change(|a| {
            *a = Adjustments {
                aspect: a.aspect,
                ..Adjustments::default()
            };
            if let Some(frame) = frame {
                a.crop = Crop::default().refit(frame, a.orientation, a.aspect);
            }
        });
    });

    let sliders: [(&gtk::Scale, SetValue); 5] = [
        (&w.brightness, |a, v| a.brightness = v),
        (&w.contrast, |a, v| a.contrast = v),
        (&w.hue, |a, v| a.hue = v),
        (&w.saturation, |a, v| a.saturation = v),
        (&w.gamma, |a, v| a.gamma = v),
    ];
    for (scale, set) in sliders {
        let weak = Rc::downgrade(w);
        scale.connect_value_changed(move |scale| {
            if let Some(w) = weak.upgrade() {
                w.change(|a| set(a, scale.value()));
            }
        });
    }

    let weak = Rc::downgrade(w);
    w.aspect.connect_selected_notify(move |dropdown| {
        let Some(w) = weak.upgrade() else { return };
        if let Some(&aspect) = Aspect::ALL.get(dropdown.selected() as usize) {
            // The controller refits the crop and restarts the relay at the new size.
            w.change(|a| a.aspect = aspect);
        }
    });

    let weak = Rc::downgrade(w);
    w.crop_reset.connect_clicked(move |_| {
        let Some(w) = weak.upgrade() else { return };
        let Some(frame) = w.controller.frame_size() else {
            return;
        };
        w.change(|a| a.crop = Crop::default().refit(frame, a.orientation, a.aspect));
    });

    let weak = Rc::downgrade(w);
    w.orientation.connect_selected_notify(move |dropdown| {
        let Some(w) = weak.upgrade() else { return };
        if let Some(&orientation) = Orientation::ALL.get(dropdown.selected() as usize) {
            w.change(|a| a.orientation = orientation);
        }
    });

    for row in &w.plugins {
        let weak = Rc::downgrade(w);
        let id = row.id.clone();
        row.switch.connect_active_notify(move |switch| {
            let Some(w) = weak.upgrade() else { return };
            let on = switch.is_active();
            // Plugins run in the order they were turned on; the controller restarts the relay.
            w.change(|a| {
                a.plugins.retain(|p| *p != id);
                if on {
                    a.plugins.push(id.clone());
                }
            });
        });
        for (key, scale) in &row.sliders {
            let weak = Rc::downgrade(w);
            let (id, key) = (row.id.clone(), key.clone());
            scale.connect_value_changed(move |scale| {
                let Some(w) = weak.upgrade() else { return };
                w.change(|a| {
                    a.plugin_settings
                        .entry(id.clone())
                        .or_default()
                        .insert(key.clone(), scale.value());
                });
            });
        }
    }
}

/// One switch per plugin with its sliders below it, or why it can't run.
fn plugins_grid(controller: &Controller) -> (gtk::Grid, Vec<PluginRow>) {
    let grid = grid();
    let mut rows = Vec::new();
    let mut row = 0;
    for plugin in controller.catalog().all() {
        let name = label(&plugin.name);
        let switch = gtk::Switch::builder().halign(gtk::Align::Start).build();
        if !plugin.description.is_empty() {
            name.set_tooltip_text(Some(&plugin.description));
            switch.set_tooltip_text(Some(&plugin.description));
        }
        grid.attach(&name, 0, row, 1, 1);
        grid.attach(&switch, 1, row, 1, 1);
        row += 1;
        if let Some(problem) = &plugin.problem {
            switch.set_sensitive(false);
            let why = gtk::Label::builder()
                .label(format!("🟠 Can't run: {problem}"))
                .wrap(true)
                .xalign(0.0)
                .max_width_chars(1)
                .build();
            why.add_css_class("dim-label");
            grid.attach(&why, 1, row, 1, 1);
            row += 1;
        }
        let mut sliders = Vec::new();
        for setting in &plugin.settings {
            let scale = slider(
                &grid,
                row,
                &format!("    {}", setting.label),
                setting.min,
                setting.max,
                setting.default,
            );
            scale.set_increments(
                (setting.max - setting.min) / 100.0,
                (setting.max - setting.min) / 10.0,
            );
            sliders.push((setting.key.clone(), scale));
            row += 1;
        }
        rows.push(PluginRow {
            id: plugin.id.clone(),
            switch,
            sliders,
        });
    }
    let dir = controller.catalog().dir();
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let short = match home.as_deref().and_then(|home| dir.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    };
    let hint = gtk::Label::builder()
        .label(format!("Add more in {short}"))
        .tooltip_text(dir.display().to_string())
        .ellipsize(gtk::pango::EllipsizeMode::Middle)
        .xalign(0.0)
        .build();
    hint.add_css_class("dim-label");
    grid.attach(&hint, 0, row, 2, 1);
    (grid, rows)
}

/// Wires the crop tool: the mode button, drawing the crop, and dragging it on the preview.
fn connect_crop(w: &Rc<Widgets>) {
    let weak = Rc::downgrade(w);
    w.crop_mode.connect_toggled(move |button| {
        let Some(w) = weak.upgrade() else { return };
        let on = button.is_active();
        w.controller.set_preview_tap(if on {
            PreviewTap::Source
        } else {
            PreviewTap::Output
        });
        w.crop_area.set_visible(on);
        w.crop_hint.set_visible(on);
        w.drag.set(None);
        w.show_crop();
    });

    let weak = Rc::downgrade(w);
    w.crop_area.set_draw_func(move |_, cr, width, height| {
        if let Some(w) = weak.upgrade() {
            w.draw_crop(cr, width, height);
        }
    });

    let motion = gtk::EventControllerMotion::new();
    let weak = Rc::downgrade(w);
    motion.connect_motion(move |_, x, y| {
        let Some(w) = weak.upgrade() else { return };
        if w.drag.get().is_some() {
            return;
        }
        let (Some(view), Some(rect)) = (w.view(), w.crop_rect.get()) else {
            return;
        };
        let p = view.to_frame((x, y));
        let cursor = match view.corner_at(rect, p) {
            Some(0 | 2) => "nwse-resize",
            Some(_) => "nesw-resize",
            None if rect.contains(p.0, p.1) => "move",
            None => "crosshair",
        };
        w.crop_area.set_cursor_from_name(Some(cursor));
    });
    w.crop_area.add_controller(motion);

    let gesture = gtk::GestureDrag::new();
    let weak = Rc::downgrade(w);
    gesture.connect_drag_begin(move |gesture, x, y| {
        let Some(w) = weak.upgrade() else { return };
        let Some(view) = w.view() else {
            gesture.set_state(gtk::EventSequenceState::Denied);
            return;
        };
        let rect = w.crop_rect.get().unwrap_or_else(|| view.whole());
        let p = view.to_frame((x, y));
        let drag = match view.corner_at(rect, p) {
            // Resize from the opposite corner.
            Some(i) => Drag::Corner(rect.corners()[(i + 2) % 4]),
            None if rect.contains(p.0, p.1) => Drag::Move(rect),
            None => Drag::Corner(p),
        };
        w.drag.set(Some(drag));
    });
    let weak = Rc::downgrade(w);
    gesture.connect_drag_update(move |gesture, dx, dy| {
        let Some(w) = weak.upgrade() else { return };
        let (Some(view), Some(drag), Some((x, y))) =
            (w.view(), w.drag.get(), gesture.start_point())
        else {
            return;
        };
        if dx.hypot(dy) < MIN_DRAG {
            return;
        }
        let ratio = view.aspect.ratio();
        let rect = match drag {
            Drag::Move(start) => {
                start.moved(dx / view.scale, dy / view.scale, view.width, view.height)
            }
            Drag::Corner(anchor) => Rect::from_corner(
                anchor,
                view.to_frame((x + dx, y + dy)),
                ratio,
                (view.width, view.height),
                view.width.min(view.height * ratio) * MIN_CROP_SHARE,
            ),
        };
        w.crop_rect.set(Some(rect));
        w.crop_area.queue_draw();
    });
    let weak = Rc::downgrade(w);
    gesture.connect_drag_end(move |_, dx, dy| {
        let Some(w) = weak.upgrade() else { return };
        let drag = w.drag.take();
        let (Some(view), Some(_), Some(rect)) = (w.view(), drag, w.crop_rect.get()) else {
            return;
        };
        if dx.hypot(dy) >= MIN_DRAG {
            let crop = view.crop(rect);
            w.change(|a| a.crop = crop);
        }
        // Shows the snapped crop, or puts back the old one after a click.
        w.show_crop();
    });
    w.crop_area.add_controller(gesture);
}

fn new_profile_popover(w: &Rc<Widgets>) -> gtk::Popover {
    let entry = gtk::Entry::builder()
        .placeholder_text("Name, such as zoom")
        .max_length(64)
        .build();
    let create = gtk::Button::with_label("Create");
    create.add_css_class("suggested-action");
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&entry);
    row.append(&create);
    let popover = gtk::Popover::builder().child(&row).build();

    let submit = {
        let weak = Rc::downgrade(w);
        let entry = entry.clone();
        let popover = popover.clone();
        move || {
            let Some(w) = weak.upgrade() else { return };
            let name = entry.text().trim().to_owned();
            if name.is_empty() {
                return;
            }
            let result = w.controller.use_profile(&name).map(|_| ());
            if result.is_ok() {
                entry.set_text("");
                popover.popdown();
            }
            w.report(result);
        }
    };
    let on_click = submit.clone();
    create.connect_clicked(move |_| on_click());
    entry.connect_activate(move |_| submit());
    popover
}

/// Rename and Delete for the active profile.
fn profile_popover(w: &Rc<Widgets>) -> gtk::Popover {
    let entry = gtk::Entry::builder().max_length(64).hexpand(true).build();
    let rename = gtk::Button::with_label("Rename");
    let rename_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    rename_row.append(&entry);
    rename_row.append(&rename);
    let delete = gtk::Button::with_label("Delete this profile");
    delete.add_css_class("destructive-action");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 12);
    column.append(&rename_row);
    column.append(&delete);
    let popover = gtk::Popover::builder().child(&column).build();

    // Fills in the profile as the menu opens; the last one can't go.
    let weak = Rc::downgrade(w);
    let (shown_entry, shown_delete) = (entry.clone(), delete.clone());
    popover.connect_show(move |_| {
        let Some(w) = weak.upgrade() else { return };
        shown_entry.set_text(&w.controller.status().profile);
        let count = w.controller.profiles().map_or(0, |p| p.len());
        shown_delete.set_sensitive(count > 1);
        shown_delete.set_tooltip_text((count <= 1).then_some("The only profile stays"));
    });

    let submit = {
        let weak = Rc::downgrade(w);
        let (entry, popover) = (entry.clone(), popover.clone());
        move || {
            let Some(w) = weak.upgrade() else { return };
            let from = w.controller.status().profile;
            let to = entry.text().trim().to_owned();
            if to.is_empty() || to == from {
                popover.popdown();
                return;
            }
            let result = w.controller.rename_profile(&from, &to);
            if result.is_ok() {
                popover.popdown();
            }
            w.report(result);
        }
    };
    let on_click = submit.clone();
    rename.connect_clicked(move |_| on_click());
    entry.connect_activate(move |_| submit());

    let weak = Rc::downgrade(w);
    let shown = popover.clone();
    delete.connect_clicked(move |_| {
        let Some(w) = weak.upgrade() else { return };
        shown.popdown();
        let name = w.controller.status().profile;
        w.report(w.controller.delete_profile(&name));
    });
    popover
}

/// Start on login, and the version.
fn app_popover(w: &Rc<Widgets>) -> gtk::Popover {
    let app = gtk::CheckButton::with_label("Start on login");
    let camera = gtk::CheckButton::with_label("Turn the camera on at login");
    camera.set_margin_start(24);
    let version = label(&format!("camera-relay {}", env!("CARGO_PKG_VERSION")));
    version.add_css_class("dim-label");
    let column = gtk::Box::new(gtk::Orientation::Vertical, 6);
    column.append(&app);
    column.append(&camera);
    column.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    column.append(&version);
    let popover = gtk::Popover::builder().child(&column).build();

    // True while the menu copies the file's setting in, so the handlers don't write it back.
    let syncing = Rc::new(Cell::new(false));
    let (shown_app, shown_camera, shown_syncing) = (app.clone(), camera.clone(), syncing.clone());
    popover.connect_show(move |_| {
        let mode = autostart::path().map_or(Mode::Off, |path| autostart::get(&path));
        shown_syncing.set(true);
        shown_app.set_active(mode != Mode::Off);
        shown_camera.set_active(mode == Mode::Camera);
        shown_camera.set_sensitive(mode != Mode::Off);
        shown_syncing.set(false);
    });

    for button in [&app, &camera] {
        let weak = Rc::downgrade(w);
        let (app, camera, syncing) = (app.clone(), camera.clone(), syncing.clone());
        button.connect_toggled(move |_| {
            let Some(w) = weak.upgrade() else { return };
            if syncing.get() {
                return;
            }
            let mode = match (app.is_active(), camera.is_active()) {
                (false, _) => Mode::Off,
                (true, false) => Mode::App,
                (true, true) => Mode::Camera,
            };
            camera.set_sensitive(mode != Mode::Off);
            w.report(autostart::path().and_then(|path| autostart::set(&path, mode)));
        });
    }
    popover
}

impl Widgets {
    /// Applies a change the user made, unless the window is copying the state in.
    fn change(&self, change: impl FnOnce(&mut Adjustments)) {
        if self.syncing.get() {
            return;
        }
        self.report(self.controller.update_adjustments(change).map(|_| ()));
    }

    /// Puts the profile's crop on the overlay.
    fn show_crop(&self) {
        let a = self.controller.adjustments();
        let rect = self
            .controller
            .frame_size()
            .map(|frame| a.orientation.rect_to_output(frame, a.crop.to_rect(frame)));
        self.crop_rect.set(rect);
        self.crop_area.queue_draw();
    }

    /// Where the frame sits on the crop overlay now; `None` before the frame size is known.
    fn view(&self) -> Option<View> {
        let frame = self.controller.frame_size()?;
        let a = self.controller.adjustments();
        let turned = a.orientation.output_size(frame);
        let (width, height) = (f64::from(turned.width), f64::from(turned.height));
        let (area_w, area_h) = (
            f64::from(self.crop_area.width()),
            f64::from(self.crop_area.height()),
        );
        let scale = (area_w / width).min(area_h / height);
        (scale > 0.0).then_some(View {
            frame,
            orientation: a.orientation,
            aspect: a.aspect,
            width,
            height,
            scale,
            left: (area_w - width * scale) / 2.0,
            top: (area_h - height * scale) / 2.0,
        })
    }

    /// Dims the picture outside the crop and outlines it, with a handle on each corner.
    fn draw_crop(&self, cr: &cairo::Context, _width: i32, _height: i32) {
        let (Some(view), Some(rect)) = (self.view(), self.crop_rect.get()) else {
            return;
        };
        let whole = view.to_screen(view.whole());
        let r = view.to_screen(rect);
        cr.set_fill_rule(cairo::FillRule::EvenOdd);
        cr.rectangle(whole.x, whole.y, whole.width, whole.height);
        cr.rectangle(r.x, r.y, r.width, r.height);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55);
        let _ = cr.fill();
        cr.set_source_rgb(1.0, 1.0, 1.0);
        cr.set_line_width(2.0);
        cr.rectangle(r.x, r.y, r.width, r.height);
        let _ = cr.stroke();
        for (x, y) in r.corners() {
            cr.rectangle(x - 5.0, y - 5.0, 10.0, 10.0);
        }
        let _ = cr.fill();
    }

    /// Shows an error under the controls; the next refresh clears it.
    fn report(&self, result: anyhow::Result<()>) {
        if let Err(e) = result {
            tracing::warn!("🟠 {e:#}");
            self.status.set_text(&format!("🔴 {e:#}"));
        }
    }
}

fn grid() -> gtk::Grid {
    gtk::Grid::builder()
        .column_spacing(12)
        .row_spacing(6)
        .margin_bottom(12)
        .build()
}

fn heading(text: &str) -> gtk::Label {
    let label = label(text);
    label.add_css_class("heading");
    label
}

fn label(text: &str) -> gtk::Label {
    gtk::Label::builder().label(text).xalign(0.0).build()
}

/// A labelled slider with a mark at its neutral value.
fn slider(grid: &gtk::Grid, row: i32, text: &str, min: f64, max: f64, neutral: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, 0.01);
    scale.set_digits(2);
    scale.set_draw_value(true);
    scale.set_value_pos(gtk::PositionType::Right);
    scale.set_hexpand(true);
    scale.set_width_request(240);
    scale.add_mark(neutral, gtk::PositionType::Bottom, None);
    scale.set_value(neutral);
    grid.attach(&label(text), 0, row, 1, 1);
    grid.attach(&scale, 1, row, 1, 1);
    scale
}

/// Moves a slider only when the value really differs, so dragging it doesn't fight the refresh.
fn set_value(scale: &gtk::Scale, value: f64) {
    if (scale.value() - value).abs() > 0.005 {
        scale.set_value(value);
    }
}
