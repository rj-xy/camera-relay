//! `camerarelaysoften`: a gentle blur of the brightness, blended over the picture. Smooths skin
//! and leftover grain; colour stays as sharp as it was.

use std::sync::{LazyLock, Mutex};

use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use gstreamer::subclass::prelude::*;
use gstreamer_base as gst_base;
use gstreamer_video as gst_video;
use gstreamer_video::prelude::*;
use gstreamer_video::subclass::prelude::*;

use super::{lock, yuy2_templates};

glib::wrapper! {
    pub struct Soften(ObjectSubclass<imp::Soften>)
        @extends gst_video::VideoFilter, gst_base::BaseTransform, gst::Element, gst::Object;
}

/// Buffers reused from frame to frame.
#[derive(Default)]
pub struct Scratch {
    luma: Vec<u8>,
    rows: Vec<u8>,
    blurred: Vec<u8>,
}

/// Softens the brightness of a YUY2 frame (Y U Y V: brightness in every even byte).
/// `amount` 0 leaves it as is; 1 blurs 4 pixels wide and shows only the blur.
pub fn soften(
    frame: &mut [u8],
    width: usize,
    height: usize,
    stride: usize,
    amount: f64,
    scratch: &mut Scratch,
) {
    if amount <= 0.0 || width == 0 || height == 0 {
        return;
    }
    let amount = amount.min(1.0);
    let radius = 1 + (amount * 3.0).round() as usize;
    let pixels = width * height;
    let Scratch {
        luma,
        rows,
        blurred,
    } = scratch;
    luma.resize(pixels, 0);
    rows.resize(pixels, 0);
    blurred.resize(pixels, 0);
    for y in 0..height {
        for x in 0..width {
            luma[y * width + x] = frame[y * stride + 2 * x];
        }
    }
    box_blur(luma, rows, width, 1, height, width, radius);
    box_blur(rows, blurred, height, width, width, 1, radius);

    let mix = (amount * 256.0) as i32;
    for y in 0..height {
        for x in 0..width {
            let sample = &mut frame[y * stride + 2 * x];
            let sharp = i32::from(*sample);
            let soft = i32::from(blurred[y * width + x]);
            *sample = (sharp + (soft - sharp) * mix / 256) as u8;
        }
    }
}

/// Averages each sample with `radius` neighbours either side, along `lines` lines of `len`
/// samples. Samples are `step` apart in a line, lines `line_step` apart. Edges repeat.
fn box_blur(
    src: &[u8],
    dst: &mut [u8],
    len: usize,
    step: usize,
    lines: usize,
    line_step: usize,
    radius: usize,
) {
    let window = 2 * radius as u32 + 1;
    for line in 0..lines {
        let start = line * line_step;
        let at = |i: usize| u32::from(src[start + i.min(len - 1) * step]);
        let mut sum = at(0) * radius as u32 + (0..=radius).map(at).sum::<u32>();
        for i in 0..len {
            dst[start + i * step] = ((sum + window / 2) / window) as u8;
            sum = sum + at(i + radius + 1) - at(i.saturating_sub(radius));
        }
    }
}

mod imp {
    use super::*;

    pub struct Soften {
        amount: Mutex<f64>,
        scratch: Mutex<Scratch>,
    }

    impl Default for Soften {
        fn default() -> Self {
            Self {
                amount: Mutex::new(0.3),
                scratch: Mutex::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Soften {
        const NAME: &'static str = "CameraRelaySoften";
        type Type = super::Soften;
        type ParentType = gst_video::VideoFilter;
    }

    impl ObjectImpl for Soften {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
                vec![
                    glib::ParamSpecDouble::builder("amount")
                        .nick("Amount")
                        .blurb("0 leaves the picture as is, 1 is the softest")
                        .minimum(0.0)
                        .maximum(1.0)
                        .default_value(0.3)
                        .mutable_playing()
                        .build(),
                ]
            });
            &PROPERTIES
        }

        fn set_property(&self, _id: usize, value: &glib::Value, _pspec: &glib::ParamSpec) {
            *lock(&self.amount) = value.get().unwrap_or(0.3);
        }

        fn property(&self, _id: usize, _pspec: &glib::ParamSpec) -> glib::Value {
            lock(&self.amount).to_value()
        }
    }

    impl GstObjectImpl for Soften {}

    impl ElementImpl for Soften {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
                gst::subclass::ElementMetadata::new(
                    "Camera relay soften",
                    "Filter/Effect/Video",
                    "Blurs the brightness gently and blends it over the picture",
                    "camera-relay",
                )
            });
            Some(&METADATA)
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            yuy2_templates()
        }
    }

    impl BaseTransformImpl for Soften {
        const MODE: gst_base::subclass::BaseTransformMode =
            gst_base::subclass::BaseTransformMode::AlwaysInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;
    }

    impl VideoFilterImpl for Soften {
        fn transform_frame_ip(
            &self,
            frame: &mut gst_video::VideoFrameRef<&mut gst::BufferRef>,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let amount = *lock(&self.amount);
            let (width, height) = (frame.width() as usize, frame.height() as usize);
            let stride = frame.plane_stride()[0] as usize;
            let data = frame.plane_data_mut(0).map_err(|_| gst::FlowError::Error)?;
            soften(
                data,
                width,
                height,
                stride,
                amount,
                &mut lock(&self.scratch),
            );
            Ok(gst::FlowSuccess::Ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soften_smooths_brightness_and_keeps_colour() {
        // 4×1 YUY2: a hard edge in brightness, colour bytes all 128.
        let mut frame = [0, 128, 0, 128, 255, 128, 255, 128];
        soften(&mut frame, 4, 1, 8, 1.0, &mut Scratch::default());
        assert!(frame[2] > 0 && frame[4] < 255, "edge is softer: {frame:?}");
        assert!(frame.iter().skip(1).step_by(2).all(|&c| c == 128));

        let mut same = [0, 128, 0, 128, 255, 128, 255, 128];
        soften(&mut same, 4, 1, 8, 0.0, &mut Scratch::default());
        assert_eq!(same, [0, 128, 0, 128, 255, 128, 255, 128]);
    }
}
