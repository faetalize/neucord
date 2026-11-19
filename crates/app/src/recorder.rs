//! Recording from the default input into Opus packets: PipeWire through
//! GStreamer's encoder, set as the official client's recorder is (mono,
//! 48 kHz, 20 ms frames, voice, around 32 kbps, free to choose its
//! bandwidth). `discord::voice` puts the packets in an Ogg file.
//!
//! The client's recorder takes the voice settings' echo cancellation, noise
//! suppression, noise cancellation and automatic gain control (198620), whose
//! defaults are echo cancellation, noise cancellation and gain control on and
//! the legacy noise suppression off (754712). Here GStreamer's `webrtcdsp`
//! (gst-plugins-bad) stands in for the noise and gain stages; echo
//! cancellation is left out, since `webrtcdsp` only cancels what a
//! `webrtcechoprobe` in the playback pipeline hears, and a recording has no
//! such pipeline: the speakers belong to other programs. A PipeWire
//! echo-cancel source chosen as the default input covers it outside Neucord.

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

/// What is done to the microphone's sound before it is encoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Processing {
    /// Noise cancellation: the client runs Krisp, which isn't free software;
    /// `webrtcdsp`'s suppressor at its high level is the stand-in.
    pub noise_suppression: bool,
    pub gain_control: bool,
}

impl Default for Processing {
    /// The client's default voice settings (754712).
    fn default() -> Processing {
        Processing { noise_suppression: true, gain_control: true }
    }
}

/// The plugin that does the work, in gst-plugins-bad.
const DSP: &str = "webrtcdsp";

impl Processing {
    /// The pipeline's `webrtcdsp` stage, or none if nothing is asked for.
    /// `webrtcdsp` runs on 10 ms frames of 8, 16, 32 or 48 kHz S16LE, which
    /// the recording's caps already are. Echo cancellation is off because
    /// it fails the pipeline without a probe to listen to.
    fn stage(self) -> Option<String> {
        if !self.noise_suppression && !self.gain_control {
            return None;
        }
        let (noise, gain) = (self.noise_suppression, self.gain_control);
        Some(format!(
            "{DSP} echo-cancel=false noise-suppression={noise} noise-suppression-level=high \
            gain-control={gain} gain-control-mode=adaptive-digital ! "
        ))
    }
}

/// `dsp` is whether `webrtcdsp` is installed: without it the sound goes to
/// the encoder as it came.
fn pipeline(processing: Processing, dsp: bool) -> String {
    let (rate, bitrate) = (voice::SAMPLE_RATE, voice::BITRATE);
    let stage = if dsp { processing.stage().unwrap_or_default() } else { String::new() };
    format!(
        "pipewiresrc client-name=Neucord ! audioconvert ! audioresample \
        ! audio/x-raw,format=S16LE,rate={rate},channels=1 ! {stage}opusenc name=encoder audio-type=voice \
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
        let dsp = gst::ElementFactory::find(DSP).is_some();
        if !dsp {
            eprintln!(
                "voice message: GStreamer's {DSP} is missing, recording without noise suppression or gain control"
            );
        }
        let pipeline = gst::parse::launch(&pipeline(Processing::default(), dsp)).map_err(|e| e.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;

    const ON: Processing = Processing { noise_suppression: true, gain_control: true };

    #[test]
    fn the_defaults_are_the_clients() {
        assert_eq!(Processing::default(), ON);
    }

    #[test]
    fn the_stage_sits_between_the_caps_and_the_encoder() {
        let line = pipeline(ON, true);
        let at = |needle: &str| line.find(needle).unwrap_or_else(|| panic!("{needle} missing from {line}"));
        assert!(at("channels=1") < at(DSP) && at(DSP) < at("opusenc"), "{line}");
        assert!(line.contains("noise-suppression=true") && line.contains("gain-control=true"), "{line}");
    }

    #[test]
    fn echo_cancellation_is_never_asked_for() {
        assert!(pipeline(ON, true).contains("echo-cancel=false"));
    }

    #[test]
    fn each_stage_follows_its_setting() {
        let noise_only = pipeline(Processing { gain_control: false, ..ON }, true);
        assert!(noise_only.contains("noise-suppression=true") && noise_only.contains("gain-control=false"));
        let gain_only = pipeline(Processing { noise_suppression: false, ..ON }, true);
        assert!(gain_only.contains("noise-suppression=false") && gain_only.contains("gain-control=true"));
    }

    #[test]
    fn nothing_asked_for_or_no_plugin_leaves_the_sound_alone() {
        let off = Processing { noise_suppression: false, gain_control: false };
        assert!(!pipeline(off, true).contains(DSP));
        assert!(!pipeline(ON, false).contains(DSP));
    }
}
