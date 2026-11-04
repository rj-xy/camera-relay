//! `camerarelaydenoise`: takes the grain out of a dark picture. Each sample is blended with the
//! same sample in the frames before it, which averages the noise away where the picture holds
//! still. A big change counts as movement and shows at once, so moving things don't smear.

use std::sync::{LazyLock, Mutex};

use gstreamer as gst;
use gstreamer::glib;
use gstreamer::prelude::*;
use gstreamer::subclass::prelude::*;
use gstreamer_base as gst_base;
use gstreamer_video as gst_video;
use gstreamer_video::subclass::prelude::*;

use super::{lock, yuy2_templates};

glib::wrapper! {
    pub struct Denoise(ObjectSubclass<imp::Denoise>)
        @extends gst_video::VideoFilter, gst_base::BaseTransform, gst::Element, gst::Object;
}

/// Blends `frame` into `history` (8.8 fixed point, one per byte) and writes the result back.
/// Works on any 8-bit format: every byte is treated as a sample of its own.
pub fn denoise(frame: &mut [u8], history: &mut Vec<u16>, strength: f64) {
    if history.len() != frame.len() || strength <= 0.0 {
        history.clear();
        history.extend(frame.iter().map(|&b| u16::from(b) << 8));
        return;
    }
    let strength = strength.min(1.0);
    // How much of the past each frame keeps, in 1/256: up to 7/8 at full strength.
    let keep = (strength * 224.0) as i32;
    // A change bigger than this is movement, not noise.
    let movement = (4.0 + strength * 20.0) as i32 * 256;
    for (sample, past) in frame.iter_mut().zip(history.iter_mut()) {
        let now = i32::from(*sample) << 8;
        let past_value = i32::from(*past);
        let change = now - past_value;
        let next = if change.abs() > movement {
            now
        } else {
            past_value + change * (256 - keep) / 256
        };
        *past = next as u16;
        *sample = ((next + 128) >> 8) as u8;
    }
}

mod imp {
    use super::*;

    pub struct Denoise {
        strength: Mutex<f64>,
        history: Mutex<Vec<u16>>,
    }

    impl Default for Denoise {
        fn default() -> Self {
            Self {
                strength: Mutex::new(0.5),
                history: Mutex::default(),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Denoise {
        const NAME: &'static str = "CameraRelayDenoise";
        type Type = super::Denoise;
        type ParentType = gst_video::VideoFilter;
    }

    impl ObjectImpl for Denoise {
        fn properties() -> &'static [glib::ParamSpec] {
            static PROPERTIES: LazyLock<Vec<glib::ParamSpec>> = LazyLock::new(|| {
                vec![
                    glib::ParamSpecDouble::builder("strength")
                        .nick("Strength")
                        .blurb("0 leaves the picture as is, 1 averages the most frames")
                        .minimum(0.0)
                        .maximum(1.0)
                        .default_value(0.5)
                        .mutable_playing()
                        .build(),
                ]
            });
            &PROPERTIES
        }

        fn set_property(&self, _id: usize, value: &glib::Value, _pspec: &glib::ParamSpec) {
            *lock(&self.strength) = value.get().unwrap_or(0.5);
        }

        fn property(&self, _id: usize, _pspec: &glib::ParamSpec) -> glib::Value {
            lock(&self.strength).to_value()
        }
    }

    impl GstObjectImpl for Denoise {}

    impl ElementImpl for Denoise {
        fn metadata() -> Option<&'static gst::subclass::ElementMetadata> {
            static METADATA: LazyLock<gst::subclass::ElementMetadata> = LazyLock::new(|| {
                gst::subclass::ElementMetadata::new(
                    "Camera relay denoise",
                    "Filter/Effect/Video",
                    "Takes the grain out of a dark picture by averaging still areas over frames",
                    "camera-relay",
                )
            });
            Some(&METADATA)
        }

        fn pad_templates() -> &'static [gst::PadTemplate] {
            yuy2_templates()
        }
    }

    impl BaseTransformImpl for Denoise {
        const MODE: gst_base::subclass::BaseTransformMode =
            gst_base::subclass::BaseTransformMode::AlwaysInPlace;
        const PASSTHROUGH_ON_SAME_CAPS: bool = false;
        const TRANSFORM_IP_ON_PASSTHROUGH: bool = false;

        fn stop(&self) -> Result<(), gst::ErrorMessage> {
            lock(&self.history).clear();
            Ok(())
        }
    }

    impl VideoFilterImpl for Denoise {
        fn transform_frame_ip(
            &self,
            frame: &mut gst_video::VideoFrameRef<&mut gst::BufferRef>,
        ) -> Result<gst::FlowSuccess, gst::FlowError> {
            let strength = *lock(&self.strength);
            let data = frame.plane_data_mut(0).map_err(|_| gst::FlowError::Error)?;
            denoise(data, &mut lock(&self.history), strength);
            Ok(gst::FlowSuccess::Ok)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noise_settles_and_movement_shows_at_once() {
        let mut history = Vec::new();
        let mut frame = [100u8; 4];
        denoise(&mut frame, &mut history, 1.0);
        // Small flicker is mostly averaged away.
        let mut frame = [106u8; 4];
        denoise(&mut frame, &mut history, 1.0);
        assert!(frame[0] <= 101, "{frame:?}");
        // A big change is movement: shown as is.
        let mut frame = [200u8; 4];
        denoise(&mut frame, &mut history, 1.0);
        assert_eq!(frame, [200; 4]);
        // Strength 0 leaves frames alone.
        let mut frame = [50u8; 4];
        denoise(&mut frame, &mut history, 0.0);
        assert_eq!(frame, [50; 4]);
    }
}
