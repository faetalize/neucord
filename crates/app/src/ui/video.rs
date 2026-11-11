//! A video player: the picture with libadwaita's OSD controls over it, in
//! place of `GtkVideo`'s own. They fade away while it plays and the pointer
//! rests, and stay while it is paused.
//!
//! Volume and mute are the client's `MediaPlayerVolume` (587481): every
//! player starts with what the last one left, and a click on a video that
//! began muted unmutes it before it pauses (`shouldUnmuteOnFirstInteraction`
//! in 153440).

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use discord::media::{Playback, clock};
use gtk::glib;
use gtk::prelude::*;

/// How long the controls stay once the pointer rests.
const IDLE: Duration = Duration::from_millis(2500);

thread_local! {
    /// What the next player starts with; kept while the app runs.
    static PLAYBACK: Cell<Playback> = Cell::new(Playback::default());
}

fn playback() -> Playback {
    PLAYBACK.with(Cell::get)
}

fn remember(playback: Playback) {
    PLAYBACK.with(|p| p.set(playback));
}

pub(super) struct Options {
    /// Start playing, as the viewer does; inline videos play on a click that
    /// already asked for them.
    pub autoplay: bool,
    /// Where to start, in seconds.
    pub start: f64,
    /// Show the fullscreen button.
    pub fullscreen: bool,
    /// The classes of the picture, which the caller's layout styles.
    pub classes: &'static [&'static str],
}

/// What a player does, without its widgets: what their handlers hold, since
/// a handler holding the widgets would keep them alive.
#[derive(Clone)]
pub(super) struct Control {
    stream: gtk::MediaFile,
    clicked: Rc<Cell<bool>>,
    autoplay: bool,
}

impl Control {
    /// Where it is, in seconds.
    pub(super) fn position(&self) -> f64 {
        self.stream.timestamp() as f64 / 1e6
    }

    pub(super) fn pause(&self) {
        self.stream.pause();
    }

    /// Play, or pause; from the start if it has ended.
    pub(super) fn set_playing(&self, playing: bool) {
        if playing {
            if self.stream.is_ended() {
                self.stream.seek(0);
            }
            self.clicked.set(true);
            self.stream.play();
        } else {
            self.stream.pause();
        }
    }

    pub(super) fn toggle(&self) {
        self.set_playing(!self.stream.is_playing());
    }

    /// To a tenth of its length: the number keys.
    pub(super) fn seek_tenth(&self, tenth: u32) {
        let end = self.stream.duration();
        if end > 0 {
            self.stream.seek(end * i64::from(tenth.min(9)) / 10);
            self.set_playing(true);
        }
    }

    fn set_muted(&self, muted: bool) {
        remember(Playback { muted, ..playback() });
        self.stream.set_muted(muted);
    }

    /// A click on the picture. A video that began muted, playing before
    /// anyone asked, is unmuted by the first one instead of paused.
    fn clicked_on(&self) {
        if self.autoplay && !self.clicked.get() && self.stream.is_playing() && self.stream.is_muted() {
            self.clicked.set(true);
            self.set_muted(false);
        } else {
            self.toggle();
        }
    }
}

pub(super) struct Player {
    root: gtk::Overlay,
    fullscreen: gtk::Button,
    control: Control,
}

impl Player {
    pub(super) fn new(file: &Path, options: &Options) -> Player {
        let stream = gtk::MediaFile::for_filename(file);
        let start = playback();
        stream.set_volume(start.volume);
        stream.set_muted(start.muted);
        let picture = gtk::Picture::builder()
            .paintable(&stream)
            .can_shrink(true)
            .content_fit(gtk::ContentFit::Contain)
            .overflow(gtk::Overflow::Hidden)
            .build();
        picture.set_css_classes(options.classes);
        let root = gtk::Overlay::builder().child(&picture).focusable(true).build();

        let play = flat_button("media-playback-start-symbolic", "Play");
        let time = gtk::Label::builder().label("0:00 / 0:00").css_classes(["caption", "numeric"]).build();
        let seek = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.1);
        seek.set_hexpand(true);
        seek.set_draw_value(false);
        seek.set_width_request(24);
        let mute = flat_button("audio-volume-high-symbolic", "Mute");
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 1.0, 0.05);
        volume.set_draw_value(false);
        volume.set_width_request(80);
        let volume_reveal =
            gtk::Revealer::builder().transition_type(gtk::RevealerTransitionType::SlideRight).child(&volume).build();
        let fullscreen = flat_button("view-fullscreen-symbolic", "Fullscreen");
        fullscreen.set_visible(options.fullscreen);

        let volume_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        volume_group.append(&mute);
        volume_group.append(&volume_reveal);
        let bar = gtk::Box::builder().spacing(6).css_classes(["osd", "toolbar"]).build();
        bar.append(&play);
        bar.append(&time);
        bar.append(&seek);
        bar.append(&volume_group);
        bar.append(&fullscreen);
        let controls = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .valign(gtk::Align::End)
            .child(&bar)
            .reveal_child(true)
            .build();
        root.add_overlay(&controls);

        let control =
            Control { stream: stream.clone(), clicked: Rc::new(Cell::new(false)), autoplay: options.autoplay };

        play.connect_clicked({
            let control = control.clone();
            move |_| control.toggle()
        });
        seek.connect_change_value({
            let stream = stream.clone();
            move |_, _, value| {
                stream.seek((value.max(0.0) * 1e6) as i64);
                glib::Propagation::Proceed
            }
        });
        // The handlers below get the stream as an argument: one that held it
        // would keep it, and its sound, alive.
        let refresh = {
            let (seek, time) = (seek.downgrade(), time.downgrade());
            move |stream: &gtk::MediaFile| {
                let (Some(seek), Some(time)) = (seek.upgrade(), time.upgrade()) else { return };
                let (position, length) = (stream.timestamp() as f64 / 1e6, stream.duration() as f64 / 1e6);
                if length > 0.0 {
                    seek.set_range(0.0, length);
                }
                seek.set_value(position);
                time.set_label(&format!("{} / {}", clock(position), clock(length)));
            }
        };
        stream.connect_timestamp_notify({
            let refresh = refresh.clone();
            move |stream| refresh(stream)
        });
        stream.connect_duration_notify({
            let refresh = refresh.clone();
            move |stream| refresh(stream)
        });
        stream.connect_prepared_notify(move |stream| refresh(stream));
        stream.connect_playing_notify({
            let (play, controls) = (play.downgrade(), controls.downgrade());
            move |stream| {
                let (Some(play), Some(controls)) = (play.upgrade(), controls.upgrade()) else { return };
                let (icon, tip) = match stream.is_playing() {
                    true => ("media-playback-pause-symbolic", "Pause"),
                    false => ("media-playback-start-symbolic", "Play"),
                };
                play.set_icon_name(icon);
                play.set_tooltip_text(Some(tip));
                // Paused, the controls stay.
                if !stream.is_playing() {
                    controls.set_reveal_child(true);
                }
            }
        });
        let sound = {
            let (mute, volume) = (mute.downgrade(), volume.downgrade());
            move |stream: &gtk::MediaFile| {
                let (Some(mute), Some(volume)) = (mute.upgrade(), volume.upgrade()) else { return };
                let silent = stream.is_muted() || stream.volume() == 0.0;
                let icon = match (silent, stream.volume()) {
                    (true, _) => "audio-volume-muted-symbolic",
                    (_, v) if v < 0.34 => "audio-volume-low-symbolic",
                    (_, v) if v < 0.67 => "audio-volume-medium-symbolic",
                    _ => "audio-volume-high-symbolic",
                };
                mute.set_icon_name(icon);
                mute.set_tooltip_text(Some(if silent { "Unmute" } else { "Mute" }));
                volume.set_value(if stream.is_muted() { 0.0 } else { stream.volume() });
            }
        };
        sound(&stream);
        mute.connect_clicked({
            let control = control.clone();
            move |_| control.set_muted(!control.stream.is_muted())
        });
        // Only the pointer's moves: `sound` sets the slider from the stream.
        volume.connect_change_value({
            let stream = stream.clone();
            move |_, _, value| {
                let next = playback().dragged_to(value);
                remember(next);
                stream.set_volume(next.volume);
                stream.set_muted(next.muted);
                glib::Propagation::Proceed
            }
        });
        stream.connect_volume_notify({
            let sound = sound.clone();
            move |stream| sound(stream)
        });
        stream.connect_muted_notify(move |stream| sound(stream));

        // The volume slider slides out under the pointer.
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter({
            let reveal = volume_reveal.downgrade();
            move |_, _, _| {
                if let Some(reveal) = reveal.upgrade() {
                    reveal.set_reveal_child(true);
                }
            }
        });
        hover.connect_leave({
            let reveal = volume_reveal.downgrade();
            move |_| {
                if let Some(reveal) = reveal.upgrade() {
                    reveal.set_reveal_child(false);
                }
            }
        });
        volume_group.add_controller(hover);

        hide_when_idle(&root, &controls, &bar, &stream);

        let click = gtk::GestureClick::builder().button(gtk::gdk::BUTTON_PRIMARY).build();
        click.connect_released({
            let control = control.clone();
            move |_, _, _, _| control.clicked_on()
        });
        picture.add_controller(click);

        if options.start > 0.0 {
            let start = options.start;
            stream.connect_prepared_notify(move |stream| stream.seek((start * 1e6) as i64));
        }
        if options.autoplay {
            stream.play();
        }
        Player { root, fullscreen, control }
    }

    pub(super) fn widget(&self) -> &gtk::Overlay {
        &self.root
    }

    pub(super) fn fullscreen_button(&self) -> &gtk::Button {
        &self.fullscreen
    }

    pub(super) fn control(&self) -> Control {
        self.control.clone()
    }
}

/// The controls leave a moment after the pointer last moved over a playing
/// video, and with it.
fn hide_when_idle(root: &gtk::Overlay, controls: &gtk::Revealer, bar: &gtk::Box, stream: &gtk::MediaFile) {
    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let over_bar = Rc::new(Cell::new(false));
    let wake = {
        let (controls, stream, timer, over_bar) =
            (controls.downgrade(), stream.downgrade(), timer.clone(), over_bar.clone());
        move || {
            let Some(revealer) = controls.upgrade() else { return };
            revealer.set_reveal_child(true);
            if let Some(old) = timer.borrow_mut().take() {
                old.remove();
            }
            let (controls, stream, over_bar, again) =
                (controls.clone(), stream.clone(), over_bar.clone(), timer.clone());
            let id = glib::timeout_add_local_once(IDLE, move || {
                again.borrow_mut().take();
                if let (Some(controls), Some(stream)) = (controls.upgrade(), stream.upgrade())
                    && stream.is_playing()
                    && !over_bar.get()
                {
                    controls.set_reveal_child(false);
                }
            });
            *timer.borrow_mut() = Some(id);
        }
    };
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let wake = wake.clone();
        move |_, _, _| wake()
    });
    motion.connect_enter({
        let wake = wake.clone();
        move |_, _, _| wake()
    });
    motion.connect_leave({
        let (controls, stream, timer) = (controls.downgrade(), stream.downgrade(), timer.clone());
        move |_| {
            if let Some(old) = timer.borrow_mut().take() {
                old.remove();
            }
            if let (Some(controls), Some(stream)) = (controls.upgrade(), stream.upgrade())
                && stream.is_playing()
            {
                controls.set_reveal_child(false);
            }
        }
    });
    root.add_controller(motion);
    // Over the bar, it stays.
    let bar_motion = gtk::EventControllerMotion::new();
    bar_motion.connect_enter({
        let over_bar = over_bar.clone();
        move |_, _, _| over_bar.set(true)
    });
    bar_motion.connect_leave(move |_| over_bar.set(false));
    bar.add_controller(bar_motion);
    // Playing starts the clock, so the controls go if nobody moves.
    stream.connect_playing_notify(move |_| wake());
}

fn flat_button(icon: &str, tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .build()
}
