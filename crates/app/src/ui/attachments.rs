//! Attachments and stickers: images, videos, sound, voice messages, and
//! links to other files. Media is fetched once it is asked for, so a long
//! history of videos costs nothing until one plays.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use discord::animation::Subject;
use discord::gifs::Starred;
use discord::settings::MediaSettings;
use discord::voice::VoicePlayer;
use discord::{Attachment, Snowflake, Sticker};
use gtk::prelude::*;
use gtk::{gio, glib};

use super::App;
use super::chat::MessageTarget;
use super::embeds::{behind_spoiler, fitted_picture};
use super::favorites::Stars;
use super::menus::{Entry, Item, Kind};
use super::video::{Options, Player, media_file};
use super::viewer::MediaCx;
use crate::images::{Images, Row};

/// The official client's box for a single image or video attachment.
const MAX_W: f64 = 550.0;
const MAX_H: f64 = 350.0;
/// A voice message's waveform: bars this wide, this far apart, in a strip
/// this tall (the official client's sizes).
pub(super) const BAR: f64 = 2.0;
pub(super) const BAR_PITCH: f64 = 6.0;
pub(super) const WAVE_HEIGHT: i32 = 32;

/// How an attachment shows, when not as a link.
#[derive(Clone, Copy)]
enum Shown {
    Voice,
    Audio,
    Image,
    Video,
}

/// One attachment of a message, `voice` if the message is a voice message.
pub(super) fn attachment(
    attachment: &Attachment,
    voice: bool,
    images: &Images,
    media: &MediaSettings,
    stars: &Stars,
    view: &MediaCx,
) -> gtk::Widget {
    let inline = media.inline_attachment_media;
    let shown = if voice && attachment.is_audio() {
        Shown::Voice
    } else if attachment.is_audio() {
        Shown::Audio
    } else if attachment.is_video() && inline {
        // Before images: a video of no stated type has dimensions too.
        Shown::Video
    } else if attachment.is_image() && inline {
        Shown::Image
    } else {
        return file(attachment);
    };
    let stars = stars.clone();
    let build = move |attachment: &Attachment, images: &Images, view: &MediaCx| match shown {
        Shown::Voice => voice_message(attachment, images),
        Shown::Audio => audio(attachment, images),
        Shown::Image => image(attachment, images, view, &stars),
        Shown::Video => video(attachment, images, view),
    };
    if attachment.is_spoiler() {
        let (attachment, images, view) = (attachment.clone(), images.clone(), view.clone());
        behind_spoiler(move || build(&attachment, &images, &view))
    } else {
        build(attachment, images, view)
    }
}

/// A sticker, or for one we can't draw, its name.
pub(super) fn sticker(sticker: &Sticker, images: &Images) -> gtk::Widget {
    let size = Sticker::SIZE;
    let request = discord::cdn::sticker_size(size, images.scale());
    let Some((url, animated)) = sticker.image(request) else {
        return gtk::Label::builder()
            .label(format!("Sticker: {}", sticker.name))
            .xalign(0.0)
            .css_classes(["dimmed"])
            .name(format!("{STICKER_NAME}{}", sticker.id))
            .build()
            .upcast();
    };
    let fitted = fitted_picture(Some(size), Some(size), size as f64, size as f64);
    // Stickers are cut out: no backdrop behind them, as media has.
    fitted.picture.set_css_classes(&[]);
    fitted.picture.set_tooltip_text(Some(&sticker.name));
    fitted.picture.set_alternative_text(Some(&sticker.name));
    if animated {
        images.set_moving(&fitted.picture, &url, &url, Subject::Sticker, Row::Parent);
    } else {
        images.set(&fitted.picture, &url);
    }
    // So a right-click on it can say which one.
    fitted.frame.set_widget_name(&format!("{STICKER_NAME}{}", sticker.id));
    fitted.frame.upcast()
}

/// What a sticker's widget is named before its id.
const STICKER_NAME: &str = "sticker-";

/// The id of the sticker `widget` shows, if it is one [`sticker`] made.
pub(super) fn sticker_id(widget: &gtk::Widget) -> Option<Snowflake> {
    widget.widget_name().strip_prefix(STICKER_NAME)?.parse().ok().map(Snowflake)
}

/// An inline image, scaled to fit a reasonable box. Clicking it opens the
/// viewer. A GIF has a star.
fn image(attachment: &Attachment, images: &Images, view: &MediaCx, stars: &Stars) -> gtk::Widget {
    let fitted = fitted_picture(attachment.width, attachment.height, MAX_W, MAX_H);
    fitted.picture.set_css_classes(&["attachment-image"]);
    // Through the media proxy when we can, which sends it at the size shown.
    let url = attachment.proxy_url.as_deref().unwrap_or(&attachment.url);
    let original = attachment.width.zip(attachment.height);
    if attachment.is_animated() {
        images.set_sized_moving(&fitted.picture, url, fitted.size, original, Subject::Gif, Row::None);
    } else {
        images.set_sized(&fitted.picture, url, fitted.size, original);
    }
    view.link(&fitted.frame, view.attachment(attachment.id));
    stars.over(&fitted.frame, Starred::attachment(attachment).as_ref())
}

/// A video, its box kept for it, fetched and played once clicked. Among
/// other media of its message it opens the viewer instead, as the client's
/// does, and its fullscreen button does.
fn video(attachment: &Attachment, images: &Images, view: &MediaCx) -> gtk::Widget {
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    let fitted = fitted_picture(attachment.width, attachment.height, MAX_W, MAX_H);
    fitted.picture.set_css_classes(&["attachment-image"]);
    frame.set_child(Some(&fitted.frame));
    let play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(format!("Play {}", attachment.filename))
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["osd", "circular", "embed-play"])
        .build();
    frame.add_overlay(&play);
    let index = view.attachment(attachment.id);
    view.name(&frame, index);
    play.connect_clicked({
        let frame = frame.downgrade();
        let (images, view) = (images.clone(), view.clone());
        // The viewer plays what the proxy serves, so it is this.
        let url = attachment.proxy_url.clone().unwrap_or_else(|| attachment.url.clone());
        move |button| {
            if let Some(index) = index.filter(|&index| view.together(index) > 1) {
                return view.open(index, 0.0);
            }
            button.set_sensitive(false);
            let (frame, button, images, url, view) =
                (frame.clone(), button.downgrade(), images.clone(), url.clone(), view.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let (Some(frame), Some(button)) = (frame.upgrade(), button.upgrade()) else { return };
                let Some(path) = path else { return button.set_sensitive(true) };
                frame.remove_overlay(&button);
                let options =
                    Options { autoplay: true, start: 0.0, fullscreen: index.is_some(), classes: &["attachment-image"] };
                let player = Player::new(&path, &options);
                if let Some(index) = index {
                    let control = player.control();
                    player.fullscreen_button().connect_clicked(move |_| {
                        control.pause();
                        view.open(index, control.position());
                    });
                }
                frame.add_overlay(player.widget());
            });
        }
    });
    frame.upcast()
}

/// A sound file: its name, and a play button that becomes the player.
fn audio(attachment: &Attachment, images: &Images) -> gtk::Widget {
    let row = gtk::Box::builder().spacing(8).halign(gtk::Align::Start).build();
    let play = play_button(&format!("Play {}", attachment.filename));
    row.append(&play);
    row.append(&file(attachment));
    play.connect_clicked({
        let row = row.downgrade();
        let (images, url) = (images.clone(), attachment.url.clone());
        move |button| {
            button.set_sensitive(false);
            let (row, button, images, url) = (row.clone(), button.downgrade(), images.clone(), url.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let (Some(row), Some(button)) = (row.upgrade(), button.upgrade()) else { return };
                let Some(path) = path else { return button.set_sensitive(true) };
                let stream = media_file(&path);
                stream.play();
                let controls = gtk::MediaControls::builder().media_stream(&stream).width_request(300).build();
                row.remove(&button);
                row.append(&controls);
            });
        }
    });
    row.upcast()
}

/// A voice message: play and pause, its waveform, which fills in as it plays
/// and can be pressed or dragged to move through it, and the time left.
fn voice_message(attachment: &Attachment, images: &Images) -> gtk::Widget {
    let duration = attachment.duration_secs.unwrap_or(0.0).max(0.0);
    let bars = discord::model::voice_bars(duration);
    let levels = attachment.waveform_bars(bars);
    let row = gtk::Box::builder().spacing(8).halign(gtk::Align::Start).build();
    let play = play_button("Play voice message");
    let wave = gtk::DrawingArea::builder()
        .content_width((bars as f64 * BAR_PITCH - (BAR_PITCH - BAR)) as i32)
        .content_height(WAVE_HEIGHT)
        .valign(gtk::Align::Center)
        .build();
    let player = VoicePlayer::new(duration);
    let time =
        gtk::Label::builder().label(clock(player.clock_secs() as f64)).css_classes(["caption", "numeric"]).build();
    row.append(&play);
    row.append(&wave);
    row.append(&time);

    // What the handlers share; the widgets own it, and it holds them weakly.
    let voice = Rc::new(VoiceMessage {
        url: attachment.url.clone(),
        images: images.clone(),
        player: RefCell::new(player),
        stream: RefCell::default(),
        loading: Cell::new(false),
        seek_to: Cell::new(None),
        play: play.downgrade(),
        wave: wave.downgrade(),
        time: time.downgrade(),
    });
    wave.set_draw_func({
        let voice = voice.clone();
        move |area, cr, _, height| {
            let progress = {
                let player = voice.player.borrow();
                if player.played() { player.progress() } else { 0.0 }
            };
            let colour = area.color();
            for (n, level) in levels.iter().enumerate() {
                // Played bars full, the rest faint.
                let alpha = if (n as f64 + 0.5) / bars as f64 <= progress { 1.0 } else { 0.4 };
                cr.set_source_rgba(colour.red().into(), colour.green().into(), colour.blue().into(), alpha);
                let bar = 22.0 * *level as f64 + 2.0;
                cr.rectangle(n as f64 * BAR_PITCH, (height as f64 - bar) / 2.0, BAR, bar);
                let _ = cr.fill();
            }
        }
    });
    play.connect_clicked({
        let voice = voice.clone();
        move |_| voice.toggle()
    });
    // Press jumps to the pointer; dragging keeps seeking, off the waveform too.
    let drag = gtk::GestureDrag::new();
    drag.connect_drag_begin({
        let voice = voice.clone();
        move |_, x, _| voice.scrub(x, Scrub::Begin)
    });
    for step in [Scrub::Move, Scrub::End] {
        let voice = voice.clone();
        let follow = move |drag: &gtk::GestureDrag, offset: f64| {
            let from = drag.start_point().map_or(0.0, |(x, _)| x);
            voice.scrub(from + offset, step);
        };
        match step {
            Scrub::Move => drag.connect_drag_update(move |drag, x, _| follow(drag, x)),
            _ => drag.connect_drag_end(move |drag, x, _| follow(drag, x)),
        };
    }
    wave.add_controller(drag);
    row.upcast()
}

/// How a press on a voice message's waveform goes: pressed, dragged, let go.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scrub {
    Begin,
    Move,
    End,
}

/// A voice message being listened to, which the audio is fetched for on the
/// first press or play.
struct VoiceMessage {
    url: String,
    images: Images,
    player: RefCell<VoicePlayer>,
    stream: RefCell<Option<gtk::MediaFile>>,
    loading: Cell<bool>,
    /// A position asked for that the stream, once it exists, goes to.
    seek_to: Cell<Option<f64>>,
    play: glib::WeakRef<gtk::Button>,
    wave: glib::WeakRef<gtk::DrawingArea>,
    time: glib::WeakRef<gtk::Label>,
}

impl VoiceMessage {
    /// The play button.
    fn toggle(self: &Rc<Self>) {
        self.player.borrow_mut().toggle();
        self.settle();
    }

    /// The waveform was pressed or dragged at `x`, or let go.
    fn scrub(self: &Rc<Self>, x: f64, step: Scrub) {
        let width = self.wave.upgrade().map_or(0.0, |wave| f64::from(wave.width()));
        let position = {
            let mut player = self.player.borrow_mut();
            if step == Scrub::Begin {
                player.drag_start();
            }
            if player.is_dragging() && width > 0.0 {
                player.seek(x / width);
            }
            if step == Scrub::End {
                player.drag_end();
            }
            player.position()
        };
        self.seek_to.set(Some(position));
        self.settle();
    }

    /// Make the stream do what the player says: fetch it if it has to, go to
    /// the position asked for, play or pause.
    fn settle(self: &Rc<Self>) {
        self.show();
        let stream = self.stream.borrow().clone();
        let Some(stream) = stream else { return self.load() };
        if let Some(position) = self.seek_to.take() {
            stream.seek((position * 1e6) as i64);
        }
        let audible = self.player.borrow().audible();
        match (audible, stream.is_playing()) {
            (true, false) => stream.play(),
            (false, true) => stream.pause(),
            _ => {}
        }
    }

    /// Fetch the audio, then carry on where [`Self::settle`] was.
    fn load(self: &Rc<Self>) {
        if self.loading.replace(true) {
            return;
        }
        if let Some(button) = self.play.upgrade() {
            button.set_sensitive(false);
        }
        let voice = self.clone();
        glib::spawn_future_local(async move {
            let path = voice.images.file(&voice.url).await;
            voice.loading.set(false);
            if let Some(button) = voice.play.upgrade() {
                button.set_sensitive(true);
            }
            // The row may be gone, and the voice message with it.
            let Some(path) = path.filter(|_| voice.play.upgrade().is_some()) else {
                voice.player.borrow_mut().failed();
                return voice.show();
            };
            let stream = media_file(&path);
            // Only weak references: the widgets own the voice message, which owns the stream.
            stream.connect_timestamp_notify({
                let voice = Rc::downgrade(&voice);
                move |stream| {
                    let Some(voice) = voice.upgrade() else { return };
                    let mut player = voice.player.borrow_mut();
                    if !player.is_dragging() {
                        player.moved(stream.timestamp() as f64 / 1e6);
                    }
                    drop(player);
                    voice.show();
                }
            });
            stream.connect_duration_notify({
                let voice = Rc::downgrade(&voice);
                move |stream| {
                    let Some(voice) = voice.upgrade() else { return };
                    voice.player.borrow_mut().loaded(stream.duration() as f64 / 1e6);
                    voice.show();
                }
            });
            stream.connect_ended_notify({
                let voice = Rc::downgrade(&voice);
                move |stream| {
                    let Some(voice) = voice.upgrade().filter(|_| stream.is_ended()) else { return };
                    voice.player.borrow_mut().ended();
                    stream.seek(0);
                    voice.show();
                }
            });
            *voice.stream.borrow_mut() = Some(stream);
            voice.settle();
        });
    }

    /// What the button, the waveform and the clock show.
    fn show(&self) {
        let player = self.player.borrow();
        if let Some(button) = self.play.upgrade() {
            let (icon, tip) = match player.is_playing() {
                true => ("media-playback-pause-symbolic", "Pause"),
                false => ("media-playback-start-symbolic", "Play voice message"),
            };
            button.set_icon_name(icon);
            button.set_tooltip_text(Some(tip));
        }
        if let Some(wave) = self.wave.upgrade() {
            wave.queue_draw();
        }
        if let Some(time) = self.time.upgrade() {
            time.set_label(&clock(player.clock_secs() as f64));
        }
    }
}

/// Save File, on a voice message (761304, where the client hands the audio's
/// URL to the browser to download). The desktop's file chooser asks where,
/// and the file the player keeps is copied there, fetched first if it wasn't
/// played: the same request playing makes.
pub(super) const SAVE_FILE: Entry<MessageTarget> = Entry::Item(Item {
    name: "save-file",
    label: "Save _File",
    shows: |_, target| target.shown().voice_audio().is_some(),
    kind: Kind::Action(|app, target| {
        if let Some(audio) = target.shown().voice_audio() {
            save_file(app, &audio);
        }
    }),
});

fn save_file(app: &Rc<App>, audio: &Attachment) {
    let dialog = gtk::FileDialog::builder().title("Save File").initial_name(save_name(&audio.filename)).build();
    let (app, url) = (app.clone(), audio.url.clone());
    glib::spawn_future_local(async move {
        // Cancelling is no error.
        let Ok(destination) = dialog.save_future(Some(&app.window)).await else { return };
        let saved = match app.images.file(&url).await {
            Some(path) => {
                let copy = gio::File::for_path(path).copy_future(
                    &destination,
                    gio::FileCopyFlags::OVERWRITE,
                    glib::Priority::DEFAULT,
                );
                copy.0.await.is_ok()
            }
            None => false,
        };
        app.toast(if saved { "File Saved" } else { "Unable to save file" });
    });
}

/// What the chooser offers as the name: the file's, without any folders.
fn save_name(filename: &str) -> &str {
    let name = filename.rsplit(['/', '\\']).next().unwrap_or_default();
    if name.is_empty() { "voice-message.ogg" } else { name }
}

fn play_button(tooltip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text(tooltip)
        .valign(gtk::Align::Center)
        .css_classes(["circular"])
        .build()
}

/// `m:ss`.
pub(super) fn clock(seconds: f64) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A file we don't show: a link to it, and how big it is.
fn file(attachment: &Attachment) -> gtk::Widget {
    let mut markup = format!(
        "<a href=\"{}\">📎 {}</a>",
        glib::markup_escape_text(&attachment.url),
        glib::markup_escape_text(&attachment.filename)
    );
    if let Some(size) = attachment.size.filter(|&s| s > 0) {
        markup += &format!("  <span fgalpha=\"55%\" size=\"small\">{}</span>", glib::format_size(size));
    }
    gtk::Label::builder().use_markup(true).label(markup).xalign(0.0).build().upcast()
}

#[cfg(test)]
mod tests {
    use super::save_name;

    #[test]
    fn the_chooser_offers_a_file_name_and_no_folders() {
        assert_eq!(save_name("voice-message.ogg"), "voice-message.ogg");
        assert_eq!(save_name("../../etc/voice-message.ogg"), "voice-message.ogg");
        assert_eq!(save_name("a\\b.ogg"), "b.ogg");
        assert_eq!(save_name(""), "voice-message.ogg");
        assert_eq!(save_name("dir/"), "voice-message.ogg");
    }
}
