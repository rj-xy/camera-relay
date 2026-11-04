//! The GStreamer relay: any source element to any sink element, with live picture adjustments.
//! Knows nothing about config files: the caller describes the elements in a [`RelaySpec`].
//!
//! ```text
//! source → capture caps → tee → videocrop → videoflip → videoconvertscale → videorate
//!                          └→ source preview (optional)
//!        → filters (plugins, each videoconvert → … → videoconvert)
//!        → videobalance → gamma → output caps (one fixed format) → tee → queue → sink
//!                                                                    └→ output preview (optional)
//! output preview: queue (leaky) → valve → videoconvertscale → RGBA caps → appsink
//! source preview: queue (leaky) → valve → videoconvertscale → RGBA caps → videoflip → appsink
//! ```
//!
//! The preview branches drop every frame at their valve until [`Relay::set_preview`] asks for
//! them, so they cost nothing while no one watches. The source preview shows the whole picture,
//! turned like the output, for choosing a crop.

use std::sync::{Arc, Mutex, PoisonError};

use anyhow::{Context, Result, anyhow, bail};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;

use crate::model::{Adjustments, Crop, Orientation, VideoSize};

/// An element made from its factory name and `gst-launch-1.0` style property strings.
#[derive(Clone, Debug, PartialEq)]
pub struct ElementSpec {
    pub factory: String,
    pub properties: Vec<(String, String)>,
}

impl ElementSpec {
    pub fn new(factory: &str) -> Self {
        Self {
            factory: factory.to_owned(),
            properties: Vec::new(),
        }
    }

    pub fn property(mut self, name: &str, value: impl Into<String>) -> Self {
        self.properties.push((name.to_owned(), value.into()));
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OutputFormat {
    pub size: VideoSize,
    pub framerate: u32,
    /// GStreamer raw video format name, such as `YUY2`.
    pub format: String,
}

/// A chain of elements in series, as `gst-launch-1.0` takes it, run as one bin called `name`.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterSpec {
    pub name: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RelaySpec {
    pub source: ElementSpec,
    /// Size asked of the source; `None` lets it choose.
    pub capture: Option<VideoSize>,
    pub output: OutputFormat,
    pub sink: ElementSpec,
    /// Effects between the frame rate and the colour adjustments, in order.
    pub filters: Vec<FilterSpec>,
    /// Size of the frames [`Relay::set_preview`] delivers; `None` builds no preview branch.
    pub preview: Option<VideoSize>,
}

/// One RGBA picture from the preview branch, 4 bytes a pixel.
#[derive(Clone, Debug)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    /// Bytes from the start of one row to the next.
    pub stride: usize,
    pub data: Vec<u8>,
}

/// Receives preview frames on a GStreamer streaming thread: hand them on, don't block.
pub type PreviewSink = Arc<dyn Fn(PreviewFrame) + Send + Sync>;

/// Where the preview taps the pipeline.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewTap {
    /// What the virtual camera sends.
    #[default]
    Output,
    /// The whole camera picture before the crop and the colour changes, turned like the output.
    Source,
}

/// The valves and the current receiver of the preview branches.
struct Preview {
    output_valve: gst::Element,
    source_valve: gst::Element,
    /// Follows the main flip, so the source preview is turned like the output.
    source_flip: gst::Element,
    sink: Arc<Mutex<Option<PreviewSink>>>,
}

/// What the pipeline reports while it runs. Delivered on the default GLib main context.
#[derive(Clone, Debug, PartialEq)]
pub enum RelayEvent {
    /// The whole pipeline plays.
    Playing,
    Error(String),
    EndOfStream,
}

/// A built pipeline. Dropping it stops it and frees the camera.
pub struct Relay {
    pipeline: gst::Pipeline,
    crop: gst::Element,
    flip: gst::Element,
    balance: gst::Element,
    gamma: gst::Element,
    filters: Vec<gst::Bin>,
    preview: Option<Preview>,
    _bus_watch: gst::bus::BusWatchGuard,
}

impl Relay {
    pub fn new(
        spec: &RelaySpec,
        adjustments: &Adjustments,
        on_event: impl Fn(RelayEvent) + Send + 'static,
    ) -> Result<Self> {
        let pipeline = gst::Pipeline::with_name("camera-relay");
        let source = make_from_spec(&spec.source)?;
        let mut capture_caps = gst::Caps::builder("video/x-raw");
        if let Some(size) = spec.capture {
            capture_caps = capture_caps
                .field("width", to_i32(size.width)?)
                .field("height", to_i32(size.height)?);
        }
        let capture_filter = make("capsfilter")?;
        capture_filter.set_property("caps", capture_caps.build());
        let crop = make("videocrop")?;
        let flip = make("videoflip")?;
        let scale = make("videoconvertscale")?;
        scale.set_property("n-threads", 4u32);
        scale.set_property("add-borders", true);
        let rate = make("videorate")?;
        let filters = spec
            .filters
            .iter()
            .map(make_filter)
            .collect::<Result<Vec<_>>>()?;
        let balance = make("videobalance")?;
        let gamma = make("gamma")?;
        let output_filter = make("capsfilter")?;
        output_filter.set_property("caps", output_caps(&spec.output)?);
        let tee = make("tee")?;
        let sink_queue = make("queue")?;
        let sink = make_from_spec(&spec.sink)?;

        // The source tee is only there for the preview, so a relay without one stays as lean
        // as the legacy one.
        let source_tee = spec.preview.map(|_| make("tee")).transpose()?;
        let chain: Vec<&gst::Element> = [&source, &capture_filter]
            .into_iter()
            .chain(&source_tee)
            .chain([&crop, &flip, &scale, &rate])
            .chain(filters.iter().map(|f| f.upcast_ref()))
            .chain([&balance, &gamma, &output_filter, &tee, &sink_queue, &sink])
            .collect();
        pipeline.add_many(&chain)?;
        gst::Element::link_many(&chain).context("can't link the relay pipeline")?;
        let preview = match (spec.preview, &source_tee) {
            (Some(size), Some(source_tee)) => Some(add_preview(&pipeline, &tee, source_tee, size)?),
            _ => None,
        };

        let bus = pipeline.bus().context("pipeline has no bus")?;
        let pipeline_name = pipeline.upcast_ref::<gst::Object>().clone();
        let bus_watch = bus.add_watch(move |_, message| {
            if let Some(event) = event_from_message(message, &pipeline_name) {
                on_event(event);
            }
            gst::glib::ControlFlow::Continue
        })?;

        let relay = Self {
            pipeline,
            crop,
            flip,
            balance,
            gamma,
            filters,
            preview,
            _bus_watch: bus_watch,
        };
        relay.apply(adjustments);
        Ok(relay)
    }

    /// Starts the stream. Errors after this point arrive as [`RelayEvent::Error`].
    pub fn play(&self) -> Result<()> {
        self.pipeline
            .set_state(gst::State::Playing)
            .map(|_| ())
            .map_err(|_| anyhow!("the pipeline didn't start (run with GST_DEBUG=3 for details)"))
    }

    /// Changes the picture on the running pipeline, without restarting it.
    pub fn apply(&self, adjustments: &Adjustments) {
        let a = adjustments.clamped();
        self.balance.set_property("brightness", a.brightness);
        self.balance.set_property("contrast", a.contrast);
        self.balance.set_property("hue", a.hue);
        self.balance.set_property("saturation", a.saturation);
        self.gamma.set_property("gamma", a.gamma);
        for flip in std::iter::once(&self.flip).chain(self.preview.as_ref().map(|p| &p.source_flip))
        {
            flip.set_property_from_str("video-direction", video_direction(a.orientation));
        }
        let crop = fit_crop(a.crop, self.input_size());
        self.crop.set_property("left", crop.left as i32);
        self.crop.set_property("right", crop.right as i32);
        self.crop.set_property("top", crop.top as i32);
        self.crop.set_property("bottom", crop.bottom as i32);
    }

    /// Sets a number property on an element inside a filter, live. Converts to the property's
    /// type: whole-number properties get the value rounded.
    pub fn set_filter_property(
        &self,
        filter: &str,
        element: &str,
        property: &str,
        value: f64,
    ) -> Result<()> {
        let bin = self
            .filters
            .iter()
            .find(|f| f.name() == filter)
            .with_context(|| format!("no filter {filter}"))?;
        let element = bin
            .by_name(element)
            .with_context(|| format!("{filter} has no element {element}"))?;
        let pspec = element
            .find_property(property)
            .with_context(|| format!("{filter}: no property {property}"))?;
        let ty = pspec.value_type();
        let number = if ty == f64::static_type() || ty == f32::static_type() {
            value.to_value()
        } else {
            (value.round() as i64).to_value()
        };
        let value = number
            .transform_with_type(ty)
            .map_err(|_| anyhow!("{filter}: {property} doesn't take a number"))?;
        element.set_property_from_value(property, &value);
        Ok(())
    }

    /// Sends preview frames from `tap` to `sink`, or stops them with `None`. Does nothing when
    /// the spec had no preview.
    pub fn set_preview(&self, sink: Option<PreviewSink>, tap: PreviewTap) {
        let Some(preview) = &self.preview else { return };
        let on = sink.is_some();
        *preview.sink.lock().unwrap_or_else(PoisonError::into_inner) = sink;
        preview
            .output_valve
            .set_property("drop", !(on && tap == PreviewTap::Output));
        preview
            .source_valve
            .set_property("drop", !(on && tap == PreviewTap::Source));
    }

    /// The frame size coming into the crop, once the source has negotiated one.
    pub fn input_size(&self) -> Option<VideoSize> {
        let caps = self.crop.static_pad("sink")?.current_caps()?;
        let s = caps.structure(0)?;
        let width = s.get::<i32>("width").ok()?;
        let height = s.get::<i32>("height").ok()?;
        Some(VideoSize {
            width: width.try_into().ok()?,
            height: height.try_into().ok()?,
        })
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if self.pipeline.set_state(gst::State::Null).is_err() {
            tracing::warn!("🟠 The pipeline didn't stop cleanly");
        }
    }
}

/// Adds both preview branches, valves closed. Output frames are `size`; source frames are as
/// wide, with the height that keeps the picture's shape.
fn add_preview(
    pipeline: &gst::Pipeline,
    output_tee: &gst::Element,
    source_tee: &gst::Element,
    size: VideoSize,
) -> Result<Preview> {
    let sink: Arc<Mutex<Option<PreviewSink>>> = Arc::default();
    let output_caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", to_i32(size.width)?)
        .field("height", to_i32(size.height)?)
        .build();
    let output_valve = add_preview_branch(pipeline, output_tee, &output_caps, None, &sink)?;
    let source_caps = gst::Caps::builder("video/x-raw")
        .field("format", "RGBA")
        .field("width", to_i32(size.width)?)
        .field("pixel-aspect-ratio", gst::Fraction::new(1, 1))
        .build();
    let source_flip = make("videoflip")?;
    let source_valve = add_preview_branch(
        pipeline,
        source_tee,
        &source_caps,
        Some(&source_flip),
        &sink,
    )?;
    Ok(Preview {
        output_valve,
        source_valve,
        source_flip,
        sink,
    })
}

/// Adds `tee → queue → valve → videoconvertscale → caps → [flip] → appsink` and returns the
/// valve, closed. Frames go to whatever `sink` holds.
fn add_preview_branch(
    pipeline: &gst::Pipeline,
    tee: &gst::Element,
    caps: &gst::Caps,
    flip: Option<&gst::Element>,
    sink: &Arc<Mutex<Option<PreviewSink>>>,
) -> Result<gst::Element> {
    let queue = make("queue")?;
    queue.set_property_from_str("leaky", "downstream");
    queue.set_property("max-size-buffers", 1u32);
    let valve = make("valve")?;
    valve.set_property("drop", true);
    let scale = make("videoconvertscale")?;
    scale.set_property("add-borders", true);
    let filter = make("capsfilter")?;
    filter.set_property("caps", caps);
    let appsink = gst_app::AppSink::builder()
        .caps(
            &gst::Caps::builder("video/x-raw")
                .field("format", "RGBA")
                .build(),
        )
        .max_buffers(1)
        .drop(true)
        .sync(false)
        .async_(false)
        .enable_last_sample(false)
        .build();

    let receiver = sink.clone();
    appsink.set_callbacks(
        gst_app::AppSinkCallbacks::builder()
            .new_sample(move |appsink| {
                let sample = appsink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                let receiver = receiver
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();
                if let Some(receiver) = receiver
                    && let Some(frame) = preview_frame(&sample)
                {
                    receiver(frame);
                }
                Ok(gst::FlowSuccess::Ok)
            })
            .build(),
    );

    let branch: Vec<&gst::Element> = [&queue, &valve, &scale, &filter]
        .into_iter()
        .chain(flip)
        .chain([appsink.upcast_ref()])
        .collect();
    pipeline.add_many(&branch)?;
    gst::Element::link_many(&branch).context("can't link a preview branch")?;
    tee.link(&queue).context("can't link a preview branch")?;
    Ok(valve)
}

fn preview_frame(sample: &gst::Sample) -> Option<PreviewFrame> {
    let s = sample.caps()?.structure(0)?;
    let width: u32 = s.get::<i32>("width").ok()?.try_into().ok()?;
    let height: u32 = s.get::<i32>("height").ok()?.try_into().ok()?;
    let map = sample.buffer()?.map_readable().ok()?;
    // RGBA rows are 4-byte aligned already, so GStreamer adds no padding.
    let stride = width as usize * 4;
    (map.len() >= stride * height as usize).then(|| PreviewFrame {
        width,
        height,
        stride,
        data: map.as_slice().to_vec(),
    })
}

/// Checks that GStreamer and every element the relay needs are there.
pub fn check_elements(spec: &RelaySpec) -> Result<()> {
    gst::init()?;
    let fixed = [
        "capsfilter",
        "videocrop",
        "videoflip",
        "videoconvertscale",
        "videoconvert",
        "videorate",
        "videobalance",
        "gamma",
        "tee",
        "queue",
        "valve",
        "appsink",
    ];
    for factory in fixed
        .iter()
        .copied()
        .chain([spec.source.factory.as_str(), spec.sink.factory.as_str()])
    {
        gst::ElementFactory::find(factory).ok_or_else(|| missing(factory))?;
    }
    Ok(())
}

/// A filter's elements in one bin, with converters either side so its elements can take
/// whatever format they like.
fn make_filter(spec: &FilterSpec) -> Result<gst::Bin> {
    let description = format!("videoconvert ! {} ! videoconvert", spec.description);
    let bin = gst::parse::bin_from_description(&description, true)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("can't build {}", spec.name))?;
    bin.set_property("name", &spec.name);
    Ok(bin)
}

fn make(factory: &str) -> Result<gst::Element> {
    gst::ElementFactory::make(factory)
        .build()
        .map_err(|_| missing(factory))
}

fn make_from_spec(spec: &ElementSpec) -> Result<gst::Element> {
    let element = make(&spec.factory)?;
    for (name, value) in &spec.properties {
        set_property_str(&element, name, value)
            .with_context(|| format!("{}: can't set {name}={value}", spec.factory))?;
    }
    Ok(element)
}

/// Like `set_property_from_str`, which panics on a bad name or value, but returns an error instead.
/// (`ElementFactory::make().property_from_str()` panics too.)
fn set_property_str(element: &gst::Element, name: &str, value: &str) -> Result<()> {
    let pspec = element
        .find_property(name)
        .ok_or_else(|| anyhow!("no such property"))?;
    if !pspec.flags().contains(gst::glib::ParamFlags::WRITABLE) {
        bail!("the property is read-only");
    }
    let value = gst::glib::Value::deserialize(value, pspec.value_type())
        .map_err(|_| anyhow!("not a valid {}", pspec.value_type()))?;
    element.set_property_from_value(name, &value);
    Ok(())
}

fn missing(factory: &str) -> anyhow::Error {
    anyhow!("GStreamer element {factory} is missing (check with: gst-inspect-1.0 {factory})")
}

fn to_i32(n: u32) -> Result<i32> {
    i32::try_from(n).with_context(|| format!("{n} is too large for a video size"))
}

fn output_caps(output: &OutputFormat) -> Result<gst::Caps> {
    Ok(gst::Caps::builder("video/x-raw")
        .field("format", output.format.as_str())
        .field("width", to_i32(output.size.width)?)
        .field("height", to_i32(output.size.height)?)
        .field(
            "framerate",
            gst::Fraction::new(to_i32(output.framerate)?, 1),
        )
        .build())
}

fn video_direction(orientation: Orientation) -> &'static str {
    match orientation {
        Orientation::None => "identity",
        Orientation::Rotate90 => "90r",
        Orientation::Rotate180 => "180",
        Orientation::Rotate270 => "90l",
        Orientation::FlipHorizontal => "horiz",
        Orientation::FlipVertical => "vert",
    }
}

/// Smallest picture a crop may leave, so a typo can't crop the frame away and stop the stream.
const MIN_CROPPED: u32 = 64;

/// Shrinks `crop` so at least [`MIN_CROPPED`] pixels stay in each direction. Unchanged when the
/// frame size isn't known yet.
fn fit_crop(crop: Crop, frame: Option<VideoSize>) -> Crop {
    let Some(frame) = frame else { return crop };
    let fit = |a: u32, b: u32, total: u32| {
        let max = total.saturating_sub(MIN_CROPPED);
        if a.saturating_add(b) <= max {
            (a, b)
        } else {
            let a = a.min(max);
            (a, b.min(max - a))
        }
    };
    let (left, right) = fit(crop.left, crop.right, frame.width);
    let (top, bottom) = fit(crop.top, crop.bottom, frame.height);
    Crop {
        left,
        right,
        top,
        bottom,
    }
}

fn event_from_message(message: &gst::Message, pipeline: &gst::Object) -> Option<RelayEvent> {
    use gst::MessageView;
    match message.view() {
        MessageView::Error(e) => {
            let source = e
                .src()
                .map(|s| s.path_string().to_string())
                .unwrap_or_default();
            let debug = e.debug().map(|d| format!(" ({d})")).unwrap_or_default();
            Some(RelayEvent::Error(format!("{source}: {}{debug}", e.error())))
        }
        MessageView::Warning(w) => {
            let source = w
                .src()
                .map(|s| s.path_string().to_string())
                .unwrap_or_default();
            tracing::warn!("🟠 {source}: {}", w.error());
            None
        }
        MessageView::Eos(_) => Some(RelayEvent::EndOfStream),
        MessageView::StateChanged(s)
            if s.src() == Some(pipeline) && s.current() == gst::State::Playing =>
        {
            Some(RelayEvent::Playing)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn crop_keeps_some_picture() {
        let frame = Some(VideoSize {
            width: 1000,
            height: 500,
        });
        let fine = Crop {
            left: 100,
            right: 100,
            top: 50,
            bottom: 0,
        };
        assert_eq!(fit_crop(fine, frame), fine);
        let too_much = Crop {
            left: 900,
            right: 900,
            top: 0,
            bottom: 1000,
        };
        let fitted = fit_crop(too_much, frame);
        assert_eq!(fitted.left + fitted.right, 1000 - MIN_CROPPED);
        assert_eq!(fitted.top + fitted.bottom, 500 - MIN_CROPPED);
        assert_eq!(fit_crop(too_much, None), too_much);
    }

    fn test_spec() -> RelaySpec {
        RelaySpec {
            source: ElementSpec::new("videotestsrc").property("is-live", "true"),
            capture: Some(VideoSize {
                width: 640,
                height: 480,
            }),
            output: OutputFormat {
                size: VideoSize {
                    width: 320,
                    height: 240,
                },
                framerate: 30,
                format: "YUY2".into(),
            },
            sink: ElementSpec::new("fakesink").property("sync", "true"),
            filters: Vec::new(),
            preview: Some(VideoSize {
                width: 160,
                height: 120,
            }),
        }
    }

    #[test]
    fn test_pattern_plays_and_takes_live_changes() {
        gst::init().unwrap();
        let relay = Relay::new(&test_spec(), &Adjustments::default(), |_| {}).unwrap();
        relay.play().unwrap();
        let (result, state, _) = relay.pipeline.state(gst::ClockTime::from_seconds(5));
        result.unwrap();
        assert_eq!(state, gst::State::Playing);

        let mut a = Adjustments::default();
        a.set("brightness", "0.3").unwrap();
        a.set("orientation", "flip-horizontal").unwrap();
        a.set("crop.left", "5000").unwrap();
        relay.apply(&a);
        std::thread::sleep(Duration::from_millis(200));

        assert_eq!(relay.balance.property::<f64>("brightness"), 0.3);
        assert_eq!(relay.crop.property::<i32>("left"), 640 - MIN_CROPPED as i32);
        let (_, state, _) = relay.pipeline.state(gst::ClockTime::from_seconds(1));
        assert_eq!(
            state,
            gst::State::Playing,
            "live changes must not stop the pipeline"
        );
    }

    #[test]
    fn filters_run_in_series_and_take_live_changes() {
        gst::init().unwrap();
        crate::plugins::register_elements();
        let mut spec = test_spec();
        spec.filters = vec![
            FilterSpec {
                name: "plugin-cleanup".into(),
                description: "camerarelaydenoise name=denoise".into(),
            },
            // Any element works, in any format: the converters around it take care of that.
            FilterSpec {
                name: "plugin-median".into(),
                description: "videomedian name=median".into(),
            },
        ];
        let relay = Relay::new(&spec, &Adjustments::default(), |_| {}).unwrap();
        relay.play().unwrap();
        let (result, state, _) = relay.pipeline.state(gst::ClockTime::from_seconds(5));
        result.unwrap();
        assert_eq!(state, gst::State::Playing);

        relay
            .set_filter_property("plugin-cleanup", "denoise", "strength", 0.8)
            .unwrap();
        // A whole-number property takes the rounded value.
        relay
            .set_filter_property("plugin-median", "median", "filtersize", 9.2)
            .unwrap();
        let bin = |name: &str| relay.filters.iter().find(|f| f.name() == name).unwrap();
        let denoise = bin("plugin-cleanup").by_name("denoise").unwrap();
        assert_eq!(denoise.property::<f64>("strength"), 0.8);
        assert!(
            relay
                .set_filter_property("plugin-cleanup", "nothing", "strength", 1.0)
                .is_err()
        );
    }

    #[test]
    fn preview_frames_arrive_only_when_asked() {
        gst::init().unwrap();
        let relay = Relay::new(&test_spec(), &Adjustments::default(), |_| {}).unwrap();
        relay.play().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        std::thread::sleep(Duration::from_millis(300));
        relay.set_preview(
            Some(Arc::new(move |frame| {
                let _ = tx.try_send(frame);
            })),
            PreviewTap::Output,
        );
        let frame = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((frame.width, frame.height), (160, 120));
        assert_eq!(frame.data.len(), 160 * 120 * 4);

        relay.set_preview(None, PreviewTap::Output);
        std::thread::sleep(Duration::from_millis(200));
        while rx.try_recv().is_ok() {}
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "the sender is gone once the preview stops"
        );
    }

    #[test]
    fn source_preview_shows_the_whole_picture_turned() {
        gst::init().unwrap();
        let mut a = Adjustments::default();
        a.set("crop.left", "320").unwrap();
        a.set("orientation", "rotate90").unwrap();
        let relay = Relay::new(&test_spec(), &a, |_| {}).unwrap();
        relay.play().unwrap();
        let (tx, rx) = std::sync::mpsc::sync_channel(8);
        relay.set_preview(
            Some(Arc::new(move |frame| {
                let _ = tx.try_send(frame);
            })),
            PreviewTap::Source,
        );
        // 640×480 scaled to 160 wide, then turned on its side; the crop doesn't show.
        let frame = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((frame.width, frame.height), (120, 160));
    }

    #[test]
    fn bad_source_property_is_an_error_not_a_panic() {
        gst::init().unwrap();
        let mut spec = test_spec();
        spec.source = spec.source.property("no-such-property", "1");
        assert!(Relay::new(&spec, &Adjustments::default(), |_| {}).is_err());
        let mut spec = test_spec();
        spec.source = spec.source.property("pattern", "no-such-pattern");
        assert!(Relay::new(&spec, &Adjustments::default(), |_| {}).is_err());
        let mut spec = test_spec();
        spec.source = spec.source.property("pattern", "ball");
        assert!(Relay::new(&spec, &Adjustments::default(), |_| {}).is_ok());
        assert!(
            check_elements(&RelaySpec {
                sink: ElementSpec::new("no-such-sink"),
                ..test_spec()
            })
            .is_err()
        );
    }
}
