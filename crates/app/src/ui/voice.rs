//! Recording a voice message: the attach menu's Record Voice Message and
//! Alt+R start it, a bar takes the message box's place with what has been
//! heard, and Send uploads the recording and posts it. What the file and the
//! message hold is `discord::voice`; the recording itself is [`crate::recorder`].
//!
//! One recording exists at a time, in one channel, as in the client: it
//! stays there as a draft after it stops, until it is sent or deleted, and
//! can be played in the bar before then (`e7` in 989161).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::rest::Reply;
use discord::uploads::{self, Data, Sent, Voice as Meta};
use discord::voice::{self, Capture, Recording, VoicePlayer};
use discord::{Rest, Snowflake};
use gtk::glib;

use super::App;
use super::attachments::{BAR, BAR_PITCH, WAVE_HEIGHT, clock};
use super::composer::ReplyCtx;
use super::menus::{Entry, Item, Kind};
use super::state::now_ms;
use super::video::media_bytes;
use crate::recorder::{self, Event, Recorder};

/// Record Voice Message, in the attach menu: greyed while a recording is
/// being finished, as in the client (`disabled` in 316576, from `wX`).
pub(super) const RECORD_VOICE: Entry<()> = Entry::Item(Item {
    name: "record-voice-message",
    label: "_Record Voice Message",
    shows: |app, _| app.voice_channel().is_some(),
    kind: Kind::Gated { enabled: |app, _| !finishing(app.voice_phase()), run: |app, _| app.record_voice_message() },
});

/// How long the recorder has to start, and to finish, before it is given up
/// on (198620's `d`).
const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(120);

/// Whether a recording is being finished, while which the client starts
/// nothing: it is stopping, or being sent (`wX` in 709372).
fn finishing(phase: Option<Phase>) -> bool {
    matches!(phase, Some(Phase::Finishing | Phase::Sending))
}

/// How a press on the waveform goes: pressed, dragged, let go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scrub {
    Begin,
    Move,
    End,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Asked to start; the recorder hasn't said it has.
    Requesting,
    Recording,
    /// Stopped, with the last packets still on their way.
    Finishing,
    Recorded,
    /// Uploading, then posting.
    Sending,
}

struct Draft {
    id: u64,
    channel: Snowflake,
    phase: Phase,
    recorder: Option<Recorder>,
    capture: Option<Capture>,
    /// When recording began, in `glib::monotonic_time` microseconds.
    started: i64,
    /// How long the finished recording is.
    millis: u64,
    /// The finished recording, unless it was too short to send.
    recording: Option<Recording>,
    /// Post the recording as soon as it is finished.
    send_when_done: bool,
    /// How many times uploading it has been tried, across sends.
    attempts: Rc<Cell<u32>>,
}

/// The finished draft being listened to.
struct Playback {
    id: u64,
    player: VoicePlayer,
    stream: gtk::MediaFile,
}

pub(super) struct Voice {
    draft: RefCell<Option<Draft>>,
    playback: RefCell<Option<Playback>>,
    ids: Cell<u64>,
    /// How many recordings this run has made, which names the next one.
    count: Cell<u32>,
    /// How loud each 20 ms of the draft was.
    levels: Rc<RefCell<Vec<u8>>>,
    bar: gtk::Box,
    stop: gtk::Button,
    play: gtk::Button,
    wave: gtk::DrawingArea,
    time: gtk::Label,
    delete: gtk::Button,
    send: gtk::Button,
}

impl Voice {
    pub(super) fn new() -> Self {
        let button = |icon: &str, tip: &str| {
            gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Center).build()
        };
        Voice {
            draft: RefCell::default(),
            playback: RefCell::default(),
            ids: Cell::default(),
            count: Cell::default(),
            levels: Rc::default(),
            bar: gtk::Box::builder().spacing(8).hexpand(true).valign(gtk::Align::Center).visible(false).build(),
            stop: button("media-playback-stop-symbolic", "Stop Voice Message Recording"),
            play: button("media-playback-start-symbolic", "Play"),
            wave: gtk::DrawingArea::builder()
                .hexpand(true)
                .content_height(WAVE_HEIGHT)
                .valign(gtk::Align::Center)
                .build(),
            time: gtk::Label::builder().css_classes(["numeric"]).build(),
            delete: button("user-trash-symbolic", "Delete Voice Message"),
            send: button("mail-send-symbolic", "Send Voice Message"),
        }
    }
}

impl App {
    /// The open channel, if Record Voice Message is offered there now: the
    /// experiment, Send Voice Messages, no forum, nothing waiting to be sent
    /// with it (files, a sticker or a draft recording), not while editing
    /// (87207, and `j` in 426819), and a machine that can record.
    pub(super) fn voice_channel(&self) -> Option<Snowflake> {
        let channel = self.account.state.borrow().channel?;
        let waiting = discord::voice::Waiting {
            editing: self.editing().is_some(),
            files: !self.drafts.of(channel).is_empty(),
            sticker: self.waiting.of(channel).is_some(),
            recording: self.has_voice_draft(channel),
        };
        let offered = self.composer_box.is_visible()
            && !waiting.blocks()
            && self.account.state.borrow().can_record_voice(channel)
            && recorder::available();
        offered.then_some(channel)
    }

    /// Whether a recording waits in `channel`.
    pub(super) fn has_voice_draft(&self, channel: Snowflake) -> bool {
        self.voice.draft.borrow().as_ref().is_some_and(|d| d.channel == channel)
    }

    pub(super) fn build_voice(self: &Rc<Self>) {
        let voice = &self.voice;
        for widget in [
            voice.stop.upcast_ref::<gtk::Widget>(),
            voice.play.upcast_ref(),
            voice.wave.upcast_ref(),
            voice.time.upcast_ref(),
            voice.delete.upcast_ref(),
            voice.send.upcast_ref(),
        ] {
            voice.bar.append(widget);
        }
        self.composer_box.append(&voice.bar);
        voice.wave.set_draw_func({
            let (levels, this) = (voice.levels.clone(), Rc::downgrade(self));
            move |area, cr, width, height| {
                let recording = this.upgrade().is_some_and(|this| this.voice_phase() == Some(Phase::Recording));
                let bars = (width as f64 / BAR_PITCH).floor() as usize;
                let shown = bars_of(&levels.borrow(), bars, recording);
                let colour = area.color();
                let progress = this.upgrade().and_then(|this| this.playback_progress());
                for (n, level) in shown.iter().enumerate() {
                    // Once played, the bars not reached yet are faint.
                    let alpha = match progress {
                        Some(progress) if (n as f64 + 0.5) / shown.len() as f64 > progress => 0.4,
                        _ => 1.0,
                    };
                    cr.set_source_rgba(colour.red().into(), colour.green().into(), colour.blue().into(), alpha);
                    let bar = 22.0 * f64::from(*level) / 255.0 + 2.0;
                    cr.rectangle(n as f64 * BAR_PITCH, (f64::from(height) - bar) / 2.0, BAR, bar);
                    let _ = cr.fill();
                }
            }
        });
        voice.stop.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.stop_voice();
                }
            }
        });
        voice.play.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.toggle_playback();
                }
            }
        });
        // Dragging through the waveform, or clicking on it, moves the playback.
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin({
            let this = Rc::downgrade(self);
            move |_, x, _| {
                if let Some(this) = this.upgrade() {
                    this.scrub(x, Scrub::Begin);
                }
            }
        });
        for step in [Scrub::Move, Scrub::End] {
            let this = Rc::downgrade(self);
            let follow = move |drag: &gtk::GestureDrag, offset: f64| {
                let from = drag.start_point().map_or(0.0, |(x, _)| x);
                if let Some(this) = this.upgrade() {
                    this.scrub(from + offset, step);
                }
            };
            match step {
                Scrub::Move => drag.connect_drag_update(move |drag, x, _| follow(drag, x)),
                _ => drag.connect_drag_end(move |drag, x, _| follow(drag, x)),
            };
        }
        voice.wave.add_controller(drag);
        voice.delete.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.discard_voice();
                }
            }
        });
        voice.send.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.send_voice();
                }
            }
        });
        let record = gtk::gio::ActionEntry::builder("record-voice-message")
            .activate({
                let this = Rc::downgrade(self);
                move |_: &adw::ApplicationWindow, _, _| {
                    if let Some(this) = this.upgrade() {
                        this.record_voice_message();
                    }
                }
            })
            .build();
        self.window.add_action_entries([record]);
        if let Some(app) = self.window.application() {
            // The client's binding where it isn't macOS (426819).
            app.set_accels_for_action("win.record-voice-message", &["<Alt>r"]);
        }
    }

    fn voice_phase(&self) -> Option<Phase> {
        self.voice.draft.borrow().as_ref().map(|d| d.phase)
    }

    /// Show the bar in the message box's place while the open channel has a
    /// recording, as the client does, and what it holds for the phase.
    pub(super) fn refresh_voice(&self) {
        let voice = &self.voice;
        let channel = self.account.state.borrow().channel;
        let shown = {
            let draft = voice.draft.borrow();
            draft.as_ref().filter(|d| Some(d.channel) == channel).map(|d| (d.phase, d.started, d.millis))
        };
        voice.bar.set_visible(shown.is_some());
        self.composer.set_visible(shown.is_none());
        self.refresh_chat_bar();
        let Some((phase, started, millis)) = shown else { return self.stop_playback() };
        voice.stop.set_visible(phase == Phase::Recording);
        voice.delete.set_sensitive(matches!(phase, Phase::Requesting | Phase::Recording | Phase::Recorded));
        voice.send.set_sensitive(matches!(phase, Phase::Recording | Phase::Recorded));
        let seconds = match phase {
            Phase::Requesting => 0.0,
            Phase::Recording => (glib::monotonic_time() - started) as f64 / 1e6,
            _ => millis as f64 / 1e3,
        };
        // The client counts whole seconds up while recording.
        voice.time.set_label(&clock(if phase == Phase::Recording { seconds.floor() } else { seconds }));
        voice.wave.queue_draw();
        self.refresh_playback();
    }

    /// What the draft's player shows: its button, and the clock counting down
    /// once it has played.
    fn refresh_playback(&self) {
        let voice = &self.voice;
        let phase = self.voice_phase();
        let playback = voice.playback.borrow();
        let Some(playback) = playback.as_ref().filter(|_| matches!(phase, Some(Phase::Recorded | Phase::Sending)))
        else {
            return voice.play.set_visible(false);
        };
        voice.play.set_visible(true);
        let (icon, tip, label) = match playback.player.is_playing() {
            true => ("media-playback-pause-symbolic", "Pause", "Pause voice message"),
            false => ("media-playback-start-symbolic", "Play", "Play voice message"),
        };
        voice.play.set_icon_name(icon);
        voice.play.set_tooltip_text(Some(tip));
        voice.play.update_property(&[gtk::accessible::Property::Label(label)]);
        voice.time.set_label(&clock(playback.player.clock_secs() as f64));
        voice.wave.queue_draw();
    }

    /// How far the draft has played, once it has.
    fn playback_progress(&self) -> Option<f64> {
        let playback = self.voice.playback.borrow();
        playback.as_ref().filter(|p| p.player.played()).map(|p| p.player.progress())
    }

    /// Give a finished recording its player, which hears the file as it will
    /// be sent.
    fn start_playback(self: &Rc<Self>, id: u64, recording: &Recording) {
        let Data::Memory(bytes) = &recording.file.data else { return };
        let stream = media_bytes(bytes);
        let this = Rc::downgrade(self);
        // Only weak references: the player owns the stream.
        stream.connect_timestamp_notify({
            let this = this.clone();
            move |stream| {
                let Some(this) = this.upgrade() else { return };
                if let Some(playback) = this.voice.playback.borrow_mut().as_mut().filter(|p| p.id == id)
                    && !playback.player.is_dragging()
                {
                    playback.player.moved(stream.timestamp() as f64 / 1e6);
                }
                this.refresh_playback();
            }
        });
        stream.connect_duration_notify({
            let this = this.clone();
            move |stream| {
                let Some(this) = this.upgrade() else { return };
                if let Some(playback) = this.voice.playback.borrow_mut().as_mut().filter(|p| p.id == id) {
                    playback.player.loaded(stream.duration() as f64 / 1e6);
                }
                this.refresh_playback();
            }
        });
        stream.connect_ended_notify({
            let this = this.clone();
            move |stream| {
                if let Some(this) = this.upgrade().filter(|_| stream.is_ended()) {
                    this.playback_ended(id);
                }
            }
        });
        stream.connect_error_notify(move |stream| {
            let Some(this) = this.upgrade() else { return };
            if let Some(error) = stream.error() {
                eprintln!("voice message: can't play the draft: {error}");
                if let Some(playback) = this.voice.playback.borrow_mut().as_mut().filter(|p| p.id == id) {
                    playback.player.failed();
                }
                this.refresh_playback();
            }
        });
        self.voice.playback.replace(Some(Playback { id, player: VoicePlayer::new(recording.duration_secs), stream }));
    }

    /// The play button.
    fn toggle_playback(&self) {
        if let Some(playback) = self.voice.playback.borrow_mut().as_mut() {
            playback.player.toggle();
        }
        self.sound_playback();
        self.refresh_playback();
    }

    /// Make the stream play or pause as the player says.
    fn sound_playback(&self) {
        let Some((stream, audible)) =
            self.voice.playback.borrow().as_ref().map(|p| (p.stream.clone(), p.player.audible()))
        else {
            return;
        };
        match (audible, stream.is_playing()) {
            (true, false) => stream.play(),
            (false, true) => stream.pause(),
            _ => {}
        }
    }

    /// The waveform was pressed or dragged at `x`, or let go.
    fn scrub(&self, x: f64, step: Scrub) {
        let width = f64::from(self.voice.wave.width());
        let (stream, position) = {
            let mut slot = self.voice.playback.borrow_mut();
            let Some(playback) = slot.as_mut().filter(|_| self.voice_phase() == Some(Phase::Recorded)) else { return };
            if step == Scrub::Begin {
                playback.player.drag_start();
            }
            if playback.player.is_dragging() && width > 0.0 {
                playback.player.seek(x / width);
            }
            if step == Scrub::End {
                playback.player.drag_end();
            }
            (playback.stream.clone(), playback.player.position())
        };
        stream.seek((position * 1e6) as i64);
        self.sound_playback();
        self.refresh_playback();
    }

    /// The media played to its end.
    fn playback_ended(&self, id: u64) {
        let stream = {
            let mut slot = self.voice.playback.borrow_mut();
            let Some(playback) = slot.as_mut().filter(|p| p.id == id) else { return };
            playback.player.ended();
            playback.stream.clone()
        };
        stream.seek(0);
        self.refresh_playback();
    }

    /// Stop listening and go back to the start, as the client's player does
    /// when it leaves the screen.
    fn stop_playback(&self) {
        let stream = {
            let mut slot = self.voice.playback.borrow_mut();
            let Some(playback) = slot.as_mut().filter(|p| p.player.played()) else { return };
            playback.player.ended();
            playback.stream.clone()
        };
        stream.pause();
        stream.seek(0);
    }

    /// Start a recording in the open channel: Record Voice Message and Alt+R.
    pub(super) fn record_voice_message(self: &Rc<Self>) {
        let Some(channel) = self.voice_channel() else { return };
        // The client starts nothing while another recording is being finished.
        if finishing(self.voice_phase()) {
            return;
        }
        let other = self.voice.draft.borrow().as_ref().map(|d| d.channel);
        if let Some(other) = other {
            return self.ask_to_discard_voice(other);
        }
        let recorder = match Recorder::start() {
            Ok(recorder) => recorder,
            Err(e) => {
                eprintln!("voice message: {e}");
                return self.show_composer_error("Failed to start recording");
            }
        };
        let id = self.voice.ids.get() + 1;
        self.voice.ids.set(id);
        self.voice.levels.borrow_mut().clear();
        let events = recorder.events();
        self.voice.draft.replace(Some(Draft {
            id,
            channel,
            phase: Phase::Requesting,
            recorder: Some(recorder),
            capture: None,
            started: 0,
            millis: 0,
            recording: None,
            send_when_done: false,
            attempts: Rc::default(),
        }));
        self.refresh_attach();
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(START_TIMEOUT, {
            let this = this.clone();
            move || {
                if let Some(this) = this.upgrade()
                    && this.voice_is(id, Phase::Requesting)
                {
                    this.voice_failed(id, "Failed to start recording");
                }
            }
        });
        glib::spawn_future_local(async move {
            while let Ok(event) = events.recv().await {
                let Some(this) = this.upgrade() else { return };
                if !this.voice_event(id, event) {
                    return;
                }
            }
        });
    }

    /// The client's question when a recording waits in another channel.
    fn ask_to_discard_voice(self: &Rc<Self>, other: Snowflake) {
        let what = {
            let state = self.account.state.borrow();
            state.find_channel(other).map(|(_, c)| match c.kind {
                discord::ChannelKind::GroupDm => "your Group DM".to_owned(),
                discord::ChannelKind::Dm => format!("your conversation with {}", c.title()),
                _ => c.title(),
            })
        };
        let body =
            format!("Starting a new recording will discard your unsent voice message in {}.", what.unwrap_or_default());
        let dialog = adw::AlertDialog::new(Some("You have an unsent voice message"), Some(&body));
        dialog.add_response("return", "_Return to Unsent Voice Message");
        dialog.add_response("discard", "_Discard Unsent Voice Message");
        dialog.set_response_appearance("discard", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("return"));
        dialog.set_close_response("return");
        let this = self.clone();
        glib::spawn_future_local(async move {
            match dialog.choose_future(Some(&this.window)).await.as_str() {
                "discard" => {
                    this.discard_voice();
                    this.record_voice_message();
                }
                _ => this.open_channel(other),
            }
        });
    }

    fn voice_is(&self, id: u64, phase: Phase) -> bool {
        self.voice.draft.borrow().as_ref().is_some_and(|d| d.id == id && d.phase == phase)
    }

    /// Take what the recorder says. Whether to go on listening.
    fn voice_event(self: &Rc<Self>, id: u64, event: Event) -> bool {
        let mut slot = self.voice.draft.borrow_mut();
        let Some(draft) = slot.as_mut().filter(|d| d.id == id) else { return false };
        match event {
            Event::Started if draft.phase == Phase::Requesting => {
                let n = self.voice.count.get();
                self.voice.count.set(n + 1);
                draft.capture = Some(Capture::new(n));
                draft.phase = Phase::Recording;
                draft.started = glib::monotonic_time();
                drop(slot);
                self.voice_started(id);
            }
            Event::Started => {}
            Event::Samples(pcm) => {
                if let Some(capture) = &mut draft.capture {
                    let levels = capture.samples(&pcm);
                    self.voice.levels.borrow_mut().extend(levels);
                }
            }
            Event::Packet(packet) => {
                if let Some(capture) = &mut draft.capture {
                    capture.packet(&packet);
                }
            }
            Event::Ended => {
                drop(slot);
                self.voice_ended(id);
                return false;
            }
            Event::Failed(why) => {
                drop(slot);
                eprintln!("voice message: {why}");
                let words = if self.voice_is(id, Phase::Requesting) {
                    "Failed to start recording"
                } else {
                    "Couldn't send your voice message. Please try again."
                };
                self.voice_failed(id, words);
                return false;
            }
        }
        true
    }

    /// Recording has begun: the clock runs, and it stops by itself at the maximum.
    fn voice_started(self: &Rc<Self>, id: u64) {
        self.refresh_voice();
        let this = Rc::downgrade(self);
        glib::timeout_add_local(Duration::from_millis(250), {
            let this = this.clone();
            move || match this.upgrade() {
                Some(this) if this.voice_is(id, Phase::Recording) => {
                    this.refresh_voice();
                    glib::ControlFlow::Continue
                }
                _ => glib::ControlFlow::Break,
            }
        });
        glib::timeout_add_local_once(Duration::from_millis(voice::MAX_MS), move || {
            if let Some(this) = this.upgrade()
                && this.voice_is(id, Phase::Recording)
            {
                this.stop_voice();
            }
        });
    }

    /// Stop recording; the recording is a draft once the last packets are in.
    fn stop_voice(self: &Rc<Self>) {
        let id = {
            let mut slot = self.voice.draft.borrow_mut();
            let Some(draft) = slot.as_mut().filter(|d| d.phase == Phase::Recording) else { return };
            draft.phase = Phase::Finishing;
            if let Some(recorder) = &draft.recorder {
                recorder.stop();
            }
            draft.id
        };
        self.refresh_voice();
        glib::timeout_add_local_once(STOP_TIMEOUT, {
            let this = Rc::downgrade(self);
            move || {
                if let Some(this) = this.upgrade()
                    && this.voice_is(id, Phase::Finishing)
                {
                    this.voice_failed(id, "Couldn't send your voice message. Please try again.");
                }
            }
        });
    }

    fn voice_ended(self: &Rc<Self>, id: u64) {
        let send = {
            let mut slot = self.voice.draft.borrow_mut();
            let Some(draft) = slot.as_mut().filter(|d| d.id == id) else { return };
            draft.recorder = None;
            if let Some(capture) = draft.capture.take() {
                draft.millis = capture.millis();
                draft.recording = capture.finish();
            }
            draft.phase = Phase::Recorded;
            (draft.send_when_done, draft.recording.clone())
        };
        let (send, recording) = send;
        if let Some(recording) = recording {
            self.start_playback(id, &recording);
        }
        self.refresh_voice();
        if send {
            self.send_voice();
        }
    }

    fn voice_failed(self: &Rc<Self>, id: u64, words: &str) {
        if self.voice.draft.borrow().as_ref().is_some_and(|d| d.id == id) {
            self.discard_voice();
            self.show_composer_error(words);
        }
    }

    /// Throw the draft away: the recording stops and nothing is kept.
    pub(super) fn discard_voice(self: &Rc<Self>) {
        self.drop_voice();
        self.refresh_attach();
    }

    /// [`Self::discard_voice`] without redrawing, for logging out.
    pub(super) fn drop_voice(&self) {
        self.voice.draft.take();
        if let Some(playback) = self.voice.playback.take() {
            playback.stream.pause();
        }
        self.voice.levels.borrow_mut().clear();
    }

    /// The open channel was left. A recording that is still going stops and
    /// stays a draft; one that hadn't started is dropped (`yx` and `eT`).
    pub(super) fn voice_left(self: &Rc<Self>, left: Snowflake) {
        match self.voice.draft.borrow().as_ref().filter(|d| d.channel == left).map(|d| d.phase) {
            Some(Phase::Requesting) => self.discard_voice(),
            Some(Phase::Recording) => self.stop_voice(),
            _ => {}
        }
    }

    /// Send the recording. One still going is stopped first and goes when it
    /// is done; one under half a second is dropped without a word (`ea`, `ed`).
    pub(super) fn send_voice(self: &Rc<Self>) {
        let (channel, recording, phase) = {
            let mut slot = self.voice.draft.borrow_mut();
            let Some(draft) = slot.as_mut() else { return };
            (draft.channel, draft.recording.clone(), draft.phase)
        };
        match phase {
            Phase::Recording => {
                if let Some(draft) = self.voice.draft.borrow_mut().as_mut() {
                    draft.send_when_done = true;
                }
                self.stop_voice();
            }
            Phase::Recorded => match recording {
                Some(recording) => self.post_voice(channel, recording),
                None => self.discard_voice(),
            },
            _ => {}
        }
    }

    /// Upload and post a recording, keeping the draft if anything stops it.
    fn post_voice(self: &Rc<Self>, channel: Snowflake, recording: Recording) {
        let rest = self.account.state.borrow().rest.clone();
        let Some(rest) = rest else { return };
        // The box follows our permissions, but they may have changed since.
        if !self.account.state.borrow().can_record_voice(channel) {
            return self.update_notice_bar();
        }
        let limits = {
            let state = self.account.state.borrow();
            state.limits().uploads(state.find_channel(channel).and_then(|(guild, _)| guild))
        };
        if let Err(refusal) = uploads::check_send(&limits, &[recording.file.size]) {
            return self.show_refusal(&refusal);
        }
        // Slowmode would refuse it; the notice bar says how long is left.
        let wait = {
            let state = self.account.state.borrow();
            state.find_channel(channel).map_or(0, |(_, c)| state.slowmode.remaining_ms(c, now_ms()))
        };
        if wait > 0 {
            return self.update_notice_bar();
        }
        let (reply, replying) = self.voice_reply(channel);
        let attempts = {
            let mut slot = self.voice.draft.borrow_mut();
            let Some(draft) = slot.as_mut() else { return };
            draft.phase = Phase::Sending;
            draft.attempts.clone()
        };
        self.stop_playback();
        self.refresh_voice();
        self.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = this.upload_voice(&rest, channel, &recording, &attempts, reply).await;
            match result {
                Ok(_) => {
                    this.record_slowmode(channel, |slowmode, c, bypass| slowmode.sent(c, bypass, now_ms()));
                    this.discard_voice();
                    this.set_reply(None);
                    if this.account.timelines.borrow().get(channel).is_some_and(|t| t.detached) {
                        this.load_newest(channel);
                    }
                }
                Err(stopped) => {
                    match &stopped {
                        super::uploads::Stopped::Send(e) if e.slowmode_wait().is_some() => {
                            let wait = e.slowmode_wait().unwrap_or_default();
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.refused(c, wait, now_ms()));
                            this.show_composer_error("Not sent: slowmode is enabled in this channel.");
                        }
                        super::uploads::Stopped::Send(e) => {
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                            this.show_composer_error(&format!("Not sent: {e}"));
                        }
                        super::uploads::Stopped::Upload(why) => {
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                            this.show_composer_error(&format!("Not sent: {why}"));
                        }
                        super::uploads::Stopped::Canceled => {
                            this.record_slowmode(channel, |slowmode, c, _| slowmode.failed(c));
                        }
                    }
                    if let Some(draft) = this.voice.draft.borrow_mut().as_mut() {
                        draft.phase = Phase::Recorded;
                    }
                    // The reply stays with the draft for another try.
                    if this.account.state.borrow().channel == Some(channel) && this.reply.borrow().is_none() {
                        this.set_reply(replying);
                    }
                    this.refresh_voice();
                }
            }
        });
    }

    /// The reply the open channel's box is making, if it may be one, for the
    /// message (`getSendMessageOptionsForReply`) and to put back.
    fn voice_reply(&self, channel: Snowflake) -> (Option<Reply>, Option<ReplyCtx>) {
        let can_reply = self.account.state.borrow().can_reply(channel);
        let replying = self.reply.borrow().clone().filter(|r| r.channel == channel && can_reply);
        let reply = replying.as_ref().map(|r| Reply {
            to: r.to,
            guild: r.guild,
            ping: r.fixed_ping.unwrap_or(self.reply_ping.is_active()),
        });
        (reply, replying)
    }

    async fn upload_voice(
        &self,
        rest: &Rest,
        channel: Snowflake,
        recording: &Recording,
        attempts: &Cell<u32>,
        reply: Option<Reply>,
    ) -> Result<discord::Message, super::uploads::Stopped> {
        use super::uploads::Stopped;
        let uploaded = uploads::upload(rest, channel, &recording.file, attempts, Rc::new(|_| {}))
            .await
            .map_err(|failure| Stopped::Upload(failure.words()))?;
        let sent = Sent {
            filename: &recording.file.filename,
            uploaded_filename: &uploaded,
            description: None,
            spoiler: false,
            content_type: &recording.file.content_type,
            voice: Some(Meta { duration_secs: recording.duration_secs, waveform: &recording.waveform }),
        };
        rest.send_voice_message(channel, reply, sent).await.map_err(Stopped::Send)
    }
}

/// The bars to draw for `levels` in `count` places: the newest while
/// recording, scrolling past the left edge; the whole of it once recorded,
/// averaged down to fit.
fn bars_of(levels: &[u8], count: usize, recording: bool) -> Vec<u8> {
    if count == 0 || levels.is_empty() {
        return Vec::new();
    }
    if recording || levels.len() <= count {
        return levels[levels.len().saturating_sub(count)..].to_vec();
    }
    (0..count)
        .map(|n| {
            let (from, to) =
                (n * levels.len() / count, ((n + 1) * levels.len() / count).max(n * levels.len() / count + 1));
            let part = &levels[from..to.min(levels.len())];
            (part.iter().map(|&l| u32::from(l)).sum::<u32>() / part.len() as u32) as u8
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::bars_of;

    #[test]
    fn nothing_starts_while_a_recording_is_being_finished() {
        use super::{Phase, finishing};
        assert!(finishing(Some(Phase::Finishing)) && finishing(Some(Phase::Sending)));
        assert!(!finishing(None));
        for phase in [Phase::Requesting, Phase::Recording, Phase::Recorded] {
            assert!(!finishing(Some(phase)), "a draft that can be returned to or discarded asks first");
        }
    }

    #[test]
    fn recording_scrolls_and_a_finished_one_fits() {
        let levels: Vec<u8> = (0..10).collect();
        assert_eq!(bars_of(&levels, 4, true), [6, 7, 8, 9], "the newest, as they come");
        assert_eq!(bars_of(&levels, 20, true), levels);
        assert_eq!(bars_of(&levels, 5, false), [0, 2, 4, 6, 8], "averaged in twos, rounding down");
        assert_eq!(bars_of(&levels, 20, false), levels, "a short one isn't stretched");
        assert!(bars_of(&levels, 0, false).is_empty() && bars_of(&[], 4, true).is_empty());
    }
}
