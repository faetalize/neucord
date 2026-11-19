//! Stats for Nerds over a playing video: the client's panel
//! (`VideoStatsOverlay`, 410694). What the file says comes from a
//! `GstDiscoverer` run on a worker thread, once, when the panel first shows;
//! what changes while it plays is read from the player every second.
//! The rows and their wording are `discord::video_stats`.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use discord::video_stats::{self, Audio, Enabled, Moment, Probe, Video};
use gst::prelude::*;
use gst_pbutils::prelude::*;
use gtk::glib;
use gtk::prelude::*;

use super::video::Player;

/// How long the discoverer may take before the file is left unread.
const TIMEOUT: u64 = 5;
const REFRESH: Duration = Duration::from_secs(1);
/// How long the copy button says it copied.
const COPIED: Duration = Duration::from_secs(2);

thread_local! {
    static ENABLED: RefCell<Enabled> = RefCell::default();
}

/// Whether the panel is on for the video at `url`.
pub(super) fn is_on(url: &str) -> bool {
    ENABLED.with(|on| on.borrow().is_on(url))
}

pub(super) fn set_on(url: &str, on: bool) {
    ENABLED.with(|enabled| enabled.borrow_mut().set(url, on));
}

/// The panel in a player's corner. It shows and hides, and reads the player
/// only while it shows.
pub(super) struct Panel {
    root: gtk::Box,
    refresh: Rc<Readout>,
}

/// What a panel needs to keep itself current.
struct Readout {
    probe: RefCell<Probe>,
    probed: Cell<bool>,
    path: PathBuf,
    grid: gtk::Grid,
    watch: Box<dyn Fn() -> Option<Moment>>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl Panel {
    /// A hidden panel over `player`, which plays the file at `path`.
    pub(super) fn new(player: &Player, path: &Path) -> Panel {
        let title =
            gtk::Label::builder().label("Stats for Nerds").xalign(0.0).hexpand(true).css_classes(["heading"]).build();
        let copy = gtk::Button::builder()
            .icon_name("edit-copy-symbolic")
            .tooltip_text("Copy to JSON")
            .css_classes(["flat", "circular"])
            .build();
        let close = gtk::Button::builder()
            .icon_name("window-close-symbolic")
            .tooltip_text("Close")
            .css_classes(["flat", "circular"])
            .build();
        let head = gtk::Box::builder().spacing(4).build();
        for widget in [title.upcast_ref::<gtk::Widget>(), copy.upcast_ref(), close.upcast_ref()] {
            head.append(widget);
        }
        let grid = gtk::Grid::builder().column_spacing(16).row_spacing(2).build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(12)
            .margin_start(12)
            .css_classes(["video-stats"])
            .visible(false)
            .build();
        root.append(&head);
        root.append(&grid);
        player.widget().add_overlay(&root);

        let refresh = Rc::new(Readout {
            probe: RefCell::default(),
            probed: Cell::new(false),
            path: path.to_owned(),
            grid,
            watch: Box::new(player.watcher()),
            timer: RefCell::default(),
        });
        let panel = Panel { root, refresh };
        copy.connect_clicked({
            let refresh = Rc::downgrade(&panel.refresh);
            move |button| {
                let Some(json) = refresh.upgrade().and_then(|r| r.json()) else { return };
                button.clipboard().set_text(&json);
                button.set_icon_name("object-select-symbolic");
                let button = button.downgrade();
                glib::timeout_add_local_once(COPIED, move || {
                    if let Some(button) = button.upgrade() {
                        button.set_icon_name("edit-copy-symbolic");
                    }
                });
            }
        });
        close.connect_clicked({
            let (panel, refresh) = (panel.root.downgrade(), Rc::downgrade(&panel.refresh));
            move |_| {
                if let (Some(root), Some(refresh)) = (panel.upgrade(), refresh.upgrade()) {
                    set_on_screen(&root, &refresh, false);
                }
            }
        });
        panel
    }

    pub(super) fn set_visible(&self, on: bool) {
        set_on_screen(&self.root, &self.refresh, on);
    }
}

fn set_on_screen(root: &gtk::Box, refresh: &Rc<Readout>, on: bool) {
    root.set_visible(on);
    if let Some(old) = refresh.timer.borrow_mut().take() {
        old.remove();
    }
    if !on {
        return;
    }
    refresh.show();
    if !refresh.probed.replace(true) {
        let weak = Rc::downgrade(refresh);
        let path = refresh.path.clone();
        glib::spawn_future_local(async move {
            let probe = probe(path).await;
            if let Some(refresh) = weak.upgrade() {
                *refresh.probe.borrow_mut() = probe;
                refresh.show();
            }
        });
    }
    let weak = Rc::downgrade(refresh);
    let id = glib::timeout_add_local(REFRESH, move || match weak.upgrade() {
        Some(refresh) if refresh.show() => glib::ControlFlow::Continue,
        _ => glib::ControlFlow::Break,
    });
    *refresh.timer.borrow_mut() = Some(id);
}

impl Readout {
    /// Write the rows; whether the player is still there.
    fn show(&self) -> bool {
        let Some(now) = (self.watch)() else { return false };
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }
        for (n, (label, value)) in video_stats::rows(&self.probe.borrow(), &now).into_iter().enumerate() {
            let n = n as i32;
            self.grid.attach(
                &gtk::Label::builder().label(label).xalign(0.0).css_classes(["dim-label"]).build(),
                0,
                n,
                1,
                1,
            );
            self.grid.attach(
                &gtk::Label::builder().label(value).xalign(0.0).selectable(true).css_classes(["numeric"]).build(),
                1,
                n,
                1,
                1,
            );
        }
        true
    }

    fn json(&self) -> Option<String> {
        Some(video_stats::json(&self.probe.borrow(), &(self.watch)()?))
    }
}

/// Read what the file at `path` says about itself, off the main loop. A file
/// GStreamer can't read, or that takes too long, has nothing to say.
async fn probe(path: PathBuf) -> Probe {
    gio::spawn_blocking(move || discover(&path)).await.unwrap_or_default()
}

fn discover(path: &Path) -> Probe {
    let mut probe = Probe { bytes: std::fs::metadata(path).ok().map(|m| m.len()), ..Probe::default() };
    if gst::init().is_err() {
        return probe;
    }
    let uri = gio::File::for_path(path).uri();
    let Ok(discoverer) = gst_pbutils::Discoverer::new(gst::ClockTime::from_seconds(TIMEOUT)) else { return probe };
    let Ok(info) = discoverer.discover_uri(&uri) else { return probe };
    probe.duration_secs = info.duration().map(|d| d.mseconds() as f64 / 1e3);

    let structure = |caps: Option<gst::Caps>| caps.and_then(|caps| caps.structure(0).map(ToOwned::to_owned));
    probe.container = info
        .container_streams()
        .first()
        .and_then(|c| structure(c.caps()))
        .and_then(|s| video_stats::container(s.name(), s.get::<&str>("variant").ok()))
        .or_else(|| info.tags().and_then(|tags| tags.get::<gst::tags::ContainerFormat>().map(|f| f.get().to_owned())));
    probe.video = info.video_streams().iter().find(|v| !v.is_image()).map(|v| {
        let caps = structure(v.caps());
        let text = |field: &str| caps.as_ref().and_then(|s| s.get::<&str>(field).ok()).map(str::to_owned);
        let number = |field: &str| caps.as_ref().and_then(|s| s.get::<u32>(field).ok());
        let rate = v.framerate();
        // Encoded caps carry the pixel layout as a chroma format and depths.
        let pixel_format = text("format").or_else(|| {
            let chroma = text("chroma-format")?;
            Some(match number("bit-depth-luma") {
                Some(depth) => format!("{chroma} {depth}-bit"),
                None => chroma,
            })
        });
        let name = caps.as_ref().map(|s| s.name().to_string()).unwrap_or_default();
        Video {
            codec: video_stats::video_codec(&name),
            raw: name,
            bitrate: Some(v.bitrate()).filter(|b| *b > 0).or(Some(v.max_bitrate()).filter(|b| *b > 0)),
            size: Some((v.width(), v.height())).filter(|&(w, h)| w > 0 && h > 0),
            frame_rate: Some((rate.numer(), rate.denom()))
                .filter(|&(n, d)| n > 0 && d > 0)
                .map(|(n, d)| (n as u32, d as u32)),
            profile: text("profile"),
            pixel_format,
        }
    });
    probe.audio = info.audio_streams().first().map(|a| {
        let caps = structure(a.caps());
        let name = caps.as_ref().map(|s| s.name().to_string()).unwrap_or_default();
        let version = caps.as_ref().and_then(|s| s.get::<i32>("mpegversion").ok());
        let profile = caps.as_ref().and_then(|s| s.get::<&str>("profile").ok());
        let known = |n: u32| Some(n).filter(|n| *n > 0);
        Audio {
            codec: video_stats::audio_codec(&name, version, profile),
            raw: name,
            bitrate: known(a.bitrate()).or(known(a.max_bitrate())),
            channels: known(a.channels()),
            sample_rate: known(a.sample_rate()),
        }
    });
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Theora and Vorbis in Ogg, which GStreamer's base plugins
    /// (`gst-plugins-base`) make and read.
    fn clip(path: &Path) {
        gst::init().unwrap();
        let line = format!(
            "videotestsrc num-buffers=30 ! video/x-raw,width=320,height=180,framerate=15/1 ! theoraenc ! queue ! mux. \
             audiotestsrc num-buffers=30 samplesperbuffer=1600 ! audio/x-raw,rate=48000,channels=2 ! vorbisenc ! queue ! mux. \
             oggmux name=mux ! filesink location={}",
            path.display()
        );
        let pipeline = gst::parse::launch(&line).unwrap();
        pipeline.set_state(gst::State::Playing).unwrap();
        let bus = pipeline.bus().unwrap();
        bus.timed_pop_filtered(gst::ClockTime::from_seconds(20), &[gst::MessageType::Eos, gst::MessageType::Error]);
        pipeline.set_state(gst::State::Null).unwrap();
    }

    #[test]
    fn the_discoverer_reads_a_files_container_codecs_size_and_rate() {
        let path = std::env::temp_dir().join(format!("neucord-probe-{}.ogg", std::process::id()));
        clip(&path);
        let probe = discover(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(probe.container.as_deref(), Some("Ogg"));
        let video = probe.video.expect("a video stream");
        assert_eq!((video.codec.as_str(), video.size, video.frame_rate), ("theora", Some((320, 180)), Some((15, 1))));
        let audio = probe.audio.expect("an audio stream");
        assert_eq!((audio.codec.as_str(), audio.channels, audio.sample_rate), ("Vorbis", Some(2), Some(48_000)));
        assert!(probe.bytes.is_some_and(|b| b > 0));
        assert!(probe.duration_secs.is_some_and(|d| d > 1.0 && d < 4.0));
    }

    #[test]
    fn a_file_that_is_not_media_has_only_its_size() {
        let path = std::env::temp_dir().join(format!("neucord-probe-text-{}.txt", std::process::id()));
        std::fs::write(&path, "not a video").unwrap();
        let probe = discover(&path);
        std::fs::remove_file(&path).ok();
        assert_eq!(probe, Probe { bytes: Some(11), ..Probe::default() });
    }
}
