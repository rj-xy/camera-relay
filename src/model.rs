//! Plain data types shared by the config files and the video pipeline.
//! No GStreamer and no file I/O here, so both sides can use them without depending on each other.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Width and height in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VideoSize {
    pub width: u32,
    pub height: u32,
}

impl fmt::Display for VideoSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}×{}", self.width, self.height)
    }
}

/// Pixels to cut from each edge of the captured frame, before scaling (so zoom keeps detail).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Crop {
    pub left: u32,
    pub right: u32,
    pub top: u32,
    pub bottom: u32,
}

impl Crop {
    /// The part of a `frame`-sized picture this crop keeps.
    pub fn to_rect(self, frame: VideoSize) -> Rect {
        let keep = |total: u32, first: u32, rest: u32| {
            let first = first.min(total);
            (
                f64::from(first),
                f64::from(total - rest.min(total - first) - first),
            )
        };
        let (x, width) = keep(frame.width, self.left, self.right);
        let (y, height) = keep(frame.height, self.top, self.bottom);
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    /// The crop that keeps `rect` of a `frame`-sized picture. Edges are even, which YUV formats
    /// need.
    pub fn from_rect(rect: Rect, frame: VideoSize) -> Crop {
        let r = rect.inside(f64::from(frame.width), f64::from(frame.height));
        let even = |n: f64| ((n / 2.0).round() * 2.0).max(0.0) as u32;
        Crop {
            left: even(r.x),
            right: even(f64::from(frame.width) - r.right()),
            top: even(r.y),
            bottom: even(f64::from(frame.height) - r.bottom()),
        }
    }

    /// This crop changed to `aspect` as the output sees it (after `orientation`): same centre and
    /// about the same area, inside the frame. The default crop becomes the largest one that fits.
    pub fn refit(self, frame: VideoSize, orientation: Orientation, aspect: Aspect) -> Crop {
        let rect = orientation.rect_to_output(frame, self.to_rect(frame));
        let rect = aspect.snap(rect, orientation.output_size(frame));
        Crop::from_rect(orientation.rect_to_source(frame, rect), frame)
    }
}

/// A rectangle in pixels. Fractional, so a drag on the preview stays smooth until it becomes a
/// [`Crop`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn right(self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn contains(self, x: f64, y: f64) -> bool {
        (self.x..=self.right()).contains(&x) && (self.y..=self.bottom()).contains(&y)
    }

    /// The four corners, clockwise from the top left.
    pub fn corners(self) -> [(f64, f64); 4] {
        [
            (self.x, self.y),
            (self.right(), self.y),
            (self.right(), self.bottom()),
            (self.x, self.bottom()),
        ]
    }

    /// The rectangle with aspect `ratio` (width / height) that has a corner at `anchor` and
    /// reaches out to `to`. It stays inside `width` × `height` and keeps at least `min_width`,
    /// moving away from the anchor when it has to.
    pub fn from_corner(
        anchor: (f64, f64),
        to: (f64, f64),
        ratio: f64,
        (width, height): (f64, f64),
        min_width: f64,
    ) -> Rect {
        let (ax, ay) = (anchor.0.clamp(0.0, width), anchor.1.clamp(0.0, height));
        let (right, down) = (to.0 >= ax, to.1 >= ay);
        let room_x = if right { width - ax } else { ax };
        let room_y = if down { height - ay } else { ay };
        let w = (to.0 - ax)
            .abs()
            .max((to.1 - ay).abs() * ratio)
            .min(room_x)
            .min(room_y * ratio)
            .max(min_width)
            .min(width)
            .min(height * ratio);
        let h = w / ratio;
        Rect {
            x: if right { ax } else { ax - w },
            y: if down { ay } else { ay - h },
            width: w,
            height: h,
        }
        .inside(width, height)
    }

    /// Moved by `dx`, `dy`, but no further than the edges of `width` × `height`.
    pub fn moved(self, dx: f64, dy: f64, width: f64, height: f64) -> Rect {
        Rect {
            x: self.x + dx,
            y: self.y + dy,
            ..self
        }
        .inside(width, height)
    }

    /// Shrunk to fit `width` × `height` if it's bigger, then moved inside.
    fn inside(self, width: f64, height: f64) -> Rect {
        let w = self.width.clamp(0.0, width);
        let h = self.height.clamp(0.0, height);
        Rect {
            x: self.x.clamp(0.0, width - w),
            y: self.y.clamp(0.0, height - h),
            width: w,
            height: h,
        }
    }
}

/// Shape of the virtual camera's picture. It sets the output size: the largest size of this
/// shape inside the configured one (1280×720 gives 1280×720, 960×720 and 720×720).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aspect {
    #[default]
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "4:3")]
    Standard,
    #[serde(rename = "1:1")]
    Square,
}

impl Aspect {
    pub const ALL: [Aspect; 3] = [Self::Wide, Self::Standard, Self::Square];

    pub fn name(self) -> &'static str {
        match self {
            Self::Wide => "16:9",
            Self::Standard => "4:3",
            Self::Square => "1:1",
        }
    }

    /// Width and height in their smallest whole numbers.
    fn parts(self) -> (u32, u32) {
        match self {
            Self::Wide => (16, 9),
            Self::Standard => (4, 3),
            Self::Square => (1, 1),
        }
    }

    /// Width divided by height.
    pub fn ratio(self) -> f64 {
        let (w, h) = self.parts();
        f64::from(w) / f64::from(h)
    }

    /// The largest size of this shape inside `bounds`, in whole steps so both sides are even
    /// and the shape is exact.
    pub fn fit(self, bounds: VideoSize) -> VideoSize {
        let (w, h) = self.parts();
        let steps = (bounds.width / (2 * w)).min(bounds.height / (2 * h)).max(1);
        VideoSize {
            width: steps * 2 * w,
            height: steps * 2 * h,
        }
    }

    /// The rectangle of this exact shape closest to `rect`: same centre and about the same area,
    /// inside `bounds`, with even corners. An exact shape scales to the output with no black bars.
    pub fn snap(self, rect: Rect, bounds: VideoSize) -> Rect {
        let (w, h) = self.parts();
        let (step_w, step_h) = (f64::from(2 * w), f64::from(2 * h));
        let max_steps = f64::from(self.fit(bounds).width) / step_w;
        let steps = (rect.width * rect.height / (step_w * step_h))
            .sqrt()
            .round()
            .clamp(1.0, max_steps);
        let (width, height) = (steps * step_w, steps * step_h);
        let even = |n: f64| (n / 2.0).round() * 2.0;
        Rect {
            x: even(rect.x + (rect.width - width) / 2.0),
            y: even(rect.y + (rect.height - height) / 2.0),
            width,
            height,
        }
        .inside(f64::from(bounds.width), f64::from(bounds.height))
    }
}

impl fmt::Display for Aspect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

impl FromStr for Aspect {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.name() == s)
            .with_context(|| {
                let names: Vec<_> = Self::ALL.iter().map(|a| a.name()).collect();
                format!(
                    "unknown aspect ratio {s:?}, expected one of {}",
                    names.join(", ")
                )
            })
    }
}

/// Rotation or mirroring of the picture.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Orientation {
    #[default]
    None,
    Rotate90,
    Rotate180,
    Rotate270,
    FlipHorizontal,
    FlipVertical,
}

impl Orientation {
    pub const ALL: [Orientation; 6] = [
        Self::None,
        Self::Rotate90,
        Self::Rotate180,
        Self::Rotate270,
        Self::FlipHorizontal,
        Self::FlipVertical,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Rotate90 => "rotate90",
            Self::Rotate180 => "rotate180",
            Self::Rotate270 => "rotate270",
            Self::FlipHorizontal => "flip-horizontal",
            Self::FlipVertical => "flip-vertical",
        }
    }

    /// Size of a `frame`-sized picture after this orientation.
    pub fn output_size(self, frame: VideoSize) -> VideoSize {
        match self {
            Self::Rotate90 | Self::Rotate270 => VideoSize {
                width: frame.height,
                height: frame.width,
            },
            _ => frame,
        }
    }

    /// Where point `(x, y)` of a `frame`-sized picture lands after this orientation.
    fn point_to_output(self, frame: VideoSize, (x, y): (f64, f64)) -> (f64, f64) {
        let (w, h) = (f64::from(frame.width), f64::from(frame.height));
        match self {
            Self::None => (x, y),
            Self::Rotate90 => (h - y, x),
            Self::Rotate180 => (w - x, h - y),
            Self::Rotate270 => (y, w - x),
            Self::FlipHorizontal => (w - x, y),
            Self::FlipVertical => (x, h - y),
        }
    }

    /// The opposite of [`Orientation::point_to_output`]; `frame` is still the unturned size.
    fn point_to_source(self, frame: VideoSize, (x, y): (f64, f64)) -> (f64, f64) {
        let (w, h) = (f64::from(frame.width), f64::from(frame.height));
        match self {
            Self::Rotate90 => (y, h - x),
            Self::Rotate270 => (w - y, x),
            // The rest undo themselves.
            other => other.point_to_output(frame, (x, y)),
        }
    }

    /// Where `rect` of a `frame`-sized picture lands after this orientation.
    pub fn rect_to_output(self, frame: VideoSize, rect: Rect) -> Rect {
        map_rect(rect, |p| self.point_to_output(frame, p))
    }

    /// The part of a `frame`-sized picture that becomes `rect` after this orientation.
    pub fn rect_to_source(self, frame: VideoSize, rect: Rect) -> Rect {
        map_rect(rect, |p| self.point_to_source(frame, p))
    }
}

/// The rectangle between two opposite corners of `rect` after `map`.
fn map_rect(rect: Rect, map: impl Fn((f64, f64)) -> (f64, f64)) -> Rect {
    let (x1, y1) = map((rect.x, rect.y));
    let (x2, y2) = map((rect.right(), rect.bottom()));
    Rect {
        x: x1.min(x2),
        y: y1.min(y2),
        width: (x2 - x1).abs(),
        height: (y2 - y1).abs(),
    }
}

impl FromStr for Orientation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::ALL
            .into_iter()
            .find(|o| o.name() == s)
            .with_context(|| {
                let names: Vec<_> = Self::ALL.iter().map(|o| o.name()).collect();
                format!(
                    "unknown orientation {s:?}, expected one of {}",
                    names.join(", ")
                )
            })
    }
}

/// Picture settings that change live on the running pipeline. One profile file holds one of these.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Adjustments {
    /// -1.0 to 1.0, 0 leaves the picture as is.
    pub brightness: f64,
    /// 0.0 to 2.0, 1 leaves the picture as is.
    pub contrast: f64,
    /// -1.0 to 1.0, 0 leaves the picture as is.
    pub hue: f64,
    /// 0.0 to 2.0, 1 leaves the picture as is.
    pub saturation: f64,
    /// 0.01 to 10.0, 1 leaves the picture as is.
    pub gamma: f64,
    pub orientation: Orientation,
    /// Shape of the output picture. Changing it restarts the relay at the new size.
    pub aspect: Aspect,
    /// Plugins by id, in the order they run. Changing the list restarts the relay.
    pub plugins: Vec<String>,
    pub crop: Crop,
    /// Plugin settings by plugin id, then setting key. Kept while a plugin is off.
    pub plugin_settings: BTreeMap<String, BTreeMap<String, f64>>,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            hue: 0.0,
            saturation: 1.0,
            gamma: 1.0,
            orientation: Orientation::None,
            aspect: Aspect::Wide,
            plugins: Vec::new(),
            crop: Crop::default(),
            plugin_settings: BTreeMap::new(),
        }
    }
}

impl Adjustments {
    /// Names accepted by [`Adjustments::set`].
    pub const KEYS: [&'static str; 13] = [
        "brightness",
        "contrast",
        "hue",
        "saturation",
        "gamma",
        "orientation",
        "aspect",
        "crop.left",
        "crop.right",
        "crop.top",
        "crop.bottom",
        "plugins",
        "<plugin>.<setting>",
    ];

    /// The same settings with every number inside the range its element accepts.
    pub fn clamped(&self) -> Self {
        Self {
            brightness: self.brightness.clamp(-1.0, 1.0),
            contrast: self.contrast.clamp(0.0, 2.0),
            hue: self.hue.clamp(-1.0, 1.0),
            saturation: self.saturation.clamp(0.0, 2.0),
            gamma: self.gamma.clamp(0.01, 10.0),
            ..self.clone()
        }
    }

    /// Sets one value by name, as typed on the command line (`brightness 0.1`, `crop.left 200`,
    /// `plugins cleanup,soften`, `cleanup.strength 0.5`). Plugin names and settings aren't
    /// checked here: the plugin catalog does that.
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        let number = || -> Result<f64> {
            let n: f64 = value
                .parse()
                .with_context(|| format!("{key}: {value:?} isn't a number"))?;
            if !n.is_finite() {
                bail!("{key}: {value:?} isn't a finite number");
            }
            Ok(n)
        };
        let pixels = || -> Result<u32> {
            value
                .parse()
                .with_context(|| format!("{key}: {value:?} isn't a whole number of pixels"))
        };
        match key {
            "brightness" => self.brightness = number()?,
            "contrast" => self.contrast = number()?,
            "hue" => self.hue = number()?,
            "saturation" => self.saturation = number()?,
            "gamma" => self.gamma = number()?,
            "orientation" => self.orientation = value.parse()?,
            "aspect" => self.aspect = value.parse()?,
            "crop.left" => self.crop.left = pixels()?,
            "crop.right" => self.crop.right = pixels()?,
            "crop.top" => self.crop.top = pixels()?,
            "crop.bottom" => self.crop.bottom = pixels()?,
            "plugins" => {
                self.plugins.clear();
                for id in value.split([',', ' ']).filter(|id| !id.is_empty()) {
                    if !self.plugins.iter().any(|p| p == id) {
                        self.plugins.push(id.to_owned());
                    }
                }
            }
            _ if key.contains('.') => {
                let (plugin, setting) = key.split_once('.').unwrap_or_default();
                self.plugin_settings
                    .entry(plugin.to_owned())
                    .or_default()
                    .insert(setting.to_owned(), number()?);
            }
            _ => bail!(
                "unknown setting {key:?}, expected one of {}",
                Self::KEYS.join(", ")
            ),
        }
        *self = self.clamped();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_parses_and_clamps() {
        let mut a = Adjustments::default();
        a.set("brightness", "0.25").unwrap();
        a.set("contrast", "5").unwrap();
        a.set("orientation", "rotate180").unwrap();
        a.set("crop.left", "200").unwrap();
        assert_eq!(a.brightness, 0.25);
        assert_eq!(a.contrast, 2.0);
        assert_eq!(a.orientation, Orientation::Rotate180);
        assert_eq!(a.crop.left, 200);
    }

    #[test]
    fn set_rejects_bad_input() {
        let mut a = Adjustments::default();
        assert!(a.set("sharpness", "1").is_err());
        assert!(a.set("gamma", "abc").is_err());
        assert!(a.set("gamma", "NaN").is_err());
        assert!(a.set("crop.top", "-3").is_err());
        assert!(a.set("orientation", "sideways").is_err());
        assert_eq!(a, Adjustments::default());
    }

    const FRAME: VideoSize = VideoSize {
        width: 2560,
        height: 1440,
    };

    #[test]
    fn aspect_sets_the_output_size() {
        let box_ = VideoSize {
            width: 1280,
            height: 720,
        };
        let sizes: Vec<_> = Aspect::ALL.map(|a| a.fit(box_).to_string()).into();
        assert_eq!(sizes, ["1280×720", "960×720", "720×720"]);
        let mut a = Adjustments::default();
        a.set("aspect", "4:3").unwrap();
        assert_eq!(a.aspect, Aspect::Standard);
        assert!(a.set("aspect", "21:9").is_err());
        let text = toml::to_string(&a).unwrap();
        assert!(text.contains("aspect = \"4:3\""), "{text}");
        assert_eq!(toml::from_str::<Adjustments>(&text).unwrap(), a);
    }

    #[test]
    fn refit_takes_the_largest_centred_area_of_the_new_shape() {
        let full = Crop::default();
        assert_eq!(full.refit(FRAME, Orientation::None, Aspect::Wide), full);
        let square = full.refit(FRAME, Orientation::None, Aspect::Square);
        assert_eq!(
            square,
            Crop {
                left: 560,
                right: 560,
                top: 0,
                bottom: 0
            }
        );
        // Turned on its side, a 16:9 output takes a band across the middle of the picture.
        let turned = full.refit(FRAME, Orientation::Rotate90, Aspect::Wide);
        let rect = turned.to_rect(FRAME);
        assert_eq!((rect.width, rect.height), (810.0, 1440.0));
        assert!((rect.x - 875.0).abs() <= 1.0, "{turned:?}");
    }

    #[test]
    fn snapped_rects_have_the_exact_shape() {
        let rough = Rect {
            x: 101.3,
            y: 57.9,
            width: 1001.7,
            height: 560.2,
        };
        for aspect in Aspect::ALL {
            let r = aspect.snap(rough, FRAME);
            assert_eq!(r.width / r.height, aspect.ratio(), "{aspect}: {r:?}");
            assert_eq!((r.x % 2.0, r.y % 2.0), (0.0, 0.0), "{aspect}: {r:?}");
            assert!(
                r.right() <= 2560.0 && r.bottom() <= 1440.0,
                "{aspect}: {r:?}"
            );
        }
        let crop = Crop::from_rect(Aspect::Wide.snap(rough, FRAME), FRAME);
        let kept = crop.to_rect(FRAME);
        assert_eq!(kept.width * 9.0, kept.height * 16.0, "{crop:?}");
    }

    #[test]
    fn orientation_maps_rects_both_ways() {
        let rect = Rect {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 100.0,
        };
        // Turned right, the top-left corner ends up top right.
        let turned = Orientation::Rotate90.rect_to_output(FRAME, rect);
        assert_eq!(
            turned,
            Rect {
                x: 1340.0,
                y: 0.0,
                width: 100.0,
                height: 200.0
            }
        );
        for o in Orientation::ALL {
            let there = o.rect_to_output(FRAME, rect);
            let size = o.output_size(FRAME);
            assert!(there.right() <= f64::from(size.width), "{o:?}");
            assert!(there.bottom() <= f64::from(size.height), "{o:?}");
            assert_eq!(o.rect_to_source(FRAME, there), rect, "{o:?}");
        }
    }

    #[test]
    fn corner_drags_keep_the_ratio_and_stay_inside() {
        let bounds = (2560.0, 1440.0);
        let ratio = 16.0 / 9.0;
        let down_right = Rect::from_corner((100.0, 100.0), (900.0, 300.0), ratio, bounds, 64.0);
        assert_eq!(
            (down_right.x, down_right.y, down_right.width),
            (100.0, 100.0, 800.0)
        );
        assert_eq!(down_right.height, 450.0);
        let up_left = Rect::from_corner((1000.0, 1000.0), (-500.0, 990.0), ratio, bounds, 64.0);
        assert_eq!(up_left.x, 0.0, "{up_left:?}");
        assert_eq!(up_left.right(), 1000.0, "{up_left:?}");
        assert_eq!(up_left.width / up_left.height, ratio);
        let tiny = Rect::from_corner((2550.0, 10.0), (2551.0, 11.0), ratio, bounds, 64.0);
        assert_eq!(tiny.width, 64.0, "{tiny:?}");
        assert!(tiny.right() <= 2560.0, "{tiny:?}");
        let moved = down_right.moved(5000.0, -5000.0, bounds.0, bounds.1);
        assert_eq!((moved.right(), moved.y), (2560.0, 0.0));
    }

    #[test]
    fn orientation_names_round_trip_through_toml() {
        for o in Orientation::ALL {
            let a = Adjustments {
                orientation: o,
                ..Default::default()
            };
            let text = toml::to_string(&a).unwrap();
            assert!(
                text.contains(&format!("orientation = \"{}\"", o.name())),
                "{text}"
            );
            assert_eq!(toml::from_str::<Adjustments>(&text).unwrap(), a);
        }
    }
}
