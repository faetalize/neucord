//! Recording from the default input into Opus packets: PipeWire through
//! GStreamer's encoder, set as the official client's recorder is (mono,
//! 48 kHz, 20 ms frames, voice, around 32 kbps, free to choose its
//! bandwidth). `discord::voice` puts the packets in an Ogg file.
//!
//! Unlike the client's recorder, nothing is done to the sound: no echo
//! cancellation, noise suppression or gain control.

use std::sync::atomic::{AtomicBool, Ordering};

use discord::voice;
use gst::prelude::*;
use gst_app::AppSinkCallbacks;

/// What the recording has made, in order.
pub enum Event {
    /// Capturing has begun.
    Started,
    /// Raw mono samples at 48 kHz, as they enter the encoder.
    Samples(Vec<i16>),
    /// One Opus packet of 20 ms.
    Packet(Vec<u8>),
    /// Stopped, and every packet has come.
    Ended,
    Failed(String),
}

fn pipeline() -> String {
    let (rate, bitrate) = (voice::SAMPLE_RATE, voice::BITRATE);
    format!(
        "pipewiresrc client-name=Neucord ! audioconvert ! audioresample \
        ! audio/x-raw,format=S16LE,rate={rate},channels=1 ! opusenc name=encoder audio-type=voice \
        bitrate={bitrate} bitrate-type=vbr frame-size=20 bandwidth=auto ! appsink name=packets sync=false"
    )
}

/// Whether this machine can record: GStreamer starts, with PipeWire's source
/// and the Opus encoder.
pub fn available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        gst::init().is_ok()
            && ["pipewiresrc", "opusenc", "audioconvert", "audioresample", "appsink"]
                .iter()
                .all(|name| gst::ElementFactory::find(name).is_some())
    })
}

pub struct Recorder {
    pipeline: gst::Pipeline,
    events: async_channel::Receiver<Event>,
    _bus: gst::bus::BusWatchGuard,
}

impl Recorder {
    /// Start recording. What goes wrong afterwards arrives as [`Event::Failed`].
    pub fn start() -> Result<Recorder, String> {
        if !available() {
            return Err("GStreamer's PipeWire source or Opus encoder is missing".to_owned());
        }
        let pipeline = gst::parse::launch(&pipeline()).map_err(|e| e.to_string())?;
        let pipeline = pipeline.downcast::<gst::Pipeline>().map_err(|_| "no pipeline".to_owned())?;
        let (send, events) = async_channel::unbounded();

        let sink = pipeline
            .by_name("packets")
            .and_then(|e| e.downcast::<gst_app::AppSink>().ok())
            .ok_or("no sink in the pipeline")?;
        sink.set_callbacks(
            AppSinkCallbacks::builder()
                .new_sample({
                    let send = send.clone();
                    move |sink| {
                        let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                        if let Some(buffer) = sample.buffer()
                            && !buffer.flags().contains(gst::BufferFlags::HEADER)
                            && let Ok(map) = buffer.map_readable()
                        {
                            let _ = send.send_blocking(Event::Packet(map.as_slice().to_vec()));
                        }
                        Ok(gst::FlowSuccess::Ok)
                    }
                })
                .build(),
        );

        // The samples are read on their way into the encoder.
        let encoder = pipeline.by_name("encoder").ok_or("no encoder in the pipeline")?;
        let pad = encoder.static_pad("sink").ok_or("no encoder input")?;
        let samples = send.clone();
        let started = AtomicBool::new(false);
        pad.add_probe(gst::PadProbeType::BUFFER, move |_, info| {
            if let Some(buffer) = info.buffer()
                && let Ok(map) = buffer.map_readable()
            {
                // The pipeline never reports PLAYING while it waits on a live source,
                // so capturing has begun when sound first arrives.
                if !started.swap(true, Ordering::Relaxed) {
                    let _ = samples.send_blocking(Event::Started);
                }
                let pcm = map.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect();
                let _ = samples.send_blocking(Event::Samples(pcm));
            }
            gst::PadProbeReturn::Ok
        });

        let bus = pipeline.bus().ok_or("no bus")?;
        let watch = bus
            .add_watch_local({
                move |_, message| {
                    use gst::MessageView::*;
                    let event = match message.view() {
                        Eos(_) => Some(Event::Ended),
                        Error(error) => Some(Event::Failed(error.error().to_string())),
                        _ => None,
                    };
                    if let Some(event) = event {
                        let _ = send.try_send(event);
                    }
                    glib::ControlFlow::Continue
                }
            })
            .map_err(|e| e.to_string())?;

        pipeline.set_state(gst::State::Playing).map_err(|e| e.to_string())?;
        Ok(Recorder { pipeline, events, _bus: watch })
    }

    pub fn events(&self) -> async_channel::Receiver<Event> {
        self.events.clone()
    }

    /// Finish what the encoder holds; [`Event::Ended`] follows the last packet.
    pub fn stop(&self) {
        self.pipeline.send_event(gst::event::Eos::new());
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}
