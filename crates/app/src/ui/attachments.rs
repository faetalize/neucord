//! Attachments and stickers: images, videos, sound, voice messages, and
//! links to other files. Media is fetched once it is asked for, so a long
//! history of videos costs nothing until one plays.

use std::cell::RefCell;
use std::rc::Rc;

use discord::settings::MediaSettings;
use discord::{Attachment, Sticker};
use gtk::glib;
use gtk::prelude::*;

use super::embeds::{behind_spoiler, fitted_picture};
use crate::images::Images;

/// The official client's box for a single image or video attachment.
const MAX_W: f64 = 550.0;
const MAX_H: f64 = 350.0;
/// A voice message's waveform: bars this wide, this far apart, in a strip
/// this tall (the official client's sizes).
const BAR: f64 = 2.0;
const BAR_PITCH: f64 = 6.0;
const WAVE_HEIGHT: i32 = 32;

/// How an attachment shows, when not as a link.
#[derive(Clone, Copy)]
enum Shown {
    Voice,
    Audio,
    Image { autoplay: bool },
    Video,
}

/// One attachment of a message, `voice` if the message is a voice message.
pub(super) fn attachment(attachment: &Attachment, voice: bool, images: &Images, media: &MediaSettings) -> gtk::Widget {
    let inline = media.inline_attachment_media;
    let shown = if voice && attachment.is_audio() {
        Shown::Voice
    } else if attachment.is_audio() {
        Shown::Audio
    } else if attachment.is_video() && inline {
        // Before images: a video of no stated type has dimensions too.
        Shown::Video
    } else if attachment.is_image() && inline {
        Shown::Image { autoplay: media.gif_auto_play }
    } else {
        return file(attachment);
    };
    let build = move |attachment: &Attachment, images: &Images| match shown {
        Shown::Voice => voice_message(attachment, images),
        Shown::Audio => audio(attachment, images),
        Shown::Image { autoplay } => image(attachment, images, autoplay),
        Shown::Video => video(attachment, images),
    };
    if attachment.is_spoiler() {
        let (attachment, images) = (attachment.clone(), images.clone());
        behind_spoiler(move || build(&attachment, &images))
    } else {
        build(attachment, images)
    }
}

/// A sticker, or for one we can't draw, its name.
pub(super) fn sticker(sticker: &Sticker, images: &Images, autoplay: bool) -> gtk::Widget {
    let size = Sticker::SIZE;
    let request = crate::images::request_size((size as i32, size as i32), images.scale(), None).0;
    let Some((url, animated)) = sticker.image(request) else {
        return gtk::Label::builder()
            .label(format!("Sticker: {}", sticker.name))
            .xalign(0.0)
            .css_classes(["dimmed"])
            .build()
            .upcast();
    };
    let fitted = fitted_picture(Some(size), Some(size), size as f64, size as f64);
    // Stickers are cut out: no backdrop behind them, as media has.
    fitted.picture.set_css_classes(&[]);
    fitted.picture.set_tooltip_text(Some(&sticker.name));
    fitted.picture.set_alternative_text(Some(&sticker.name));
    images.set(&fitted.picture, &url, animated && autoplay);
    fitted.frame.upcast()
}

/// An inline image, scaled to fit a reasonable box.
fn image(attachment: &Attachment, images: &Images, autoplay: bool) -> gtk::Widget {
    let fitted = fitted_picture(attachment.width, attachment.height, MAX_W, MAX_H);
    fitted.picture.set_css_classes(&["attachment-image"]);
    // Through the media proxy when we can, which sends it at the size shown.
    let url = attachment.proxy_url.as_deref().unwrap_or(&attachment.url);
    let original = attachment.width.zip(attachment.height);
    images.set_sized(&fitted.picture, url, fitted.size, original, autoplay && attachment.is_animated());
    fitted.frame.upcast()
}

/// A video, its box kept for it, fetched and played once clicked.
fn video(attachment: &Attachment, images: &Images) -> gtk::Widget {
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
    play.connect_clicked({
        let frame = frame.downgrade();
        let (images, url) = (images.clone(), attachment.url.clone());
        move |button| {
            button.set_sensitive(false);
            let (frame, button, images, url) = (frame.clone(), button.downgrade(), images.clone(), url.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let (Some(frame), Some(button)) = (frame.upgrade(), button.upgrade()) else { return };
                let Some(path) = path else { return button.set_sensitive(true) };
                frame.remove_overlay(&button);
                let player = gtk::Video::builder().autoplay(true).css_classes(["attachment-image"]).build();
                player.set_filename(Some(&path));
                frame.add_overlay(&player);
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
                let stream = gtk::MediaFile::for_filename(&path);
                stream.play();
                let controls = gtk::MediaControls::builder().media_stream(&stream).width_request(300).build();
                row.remove(&button);
                row.append(&controls);
            });
        }
    });
    row.upcast()
}

/// A voice message: play and pause, its waveform filling in as it plays,
/// and its length.
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
    let time = gtk::Label::builder().label(clock(duration)).css_classes(["caption", "numeric"]).build();
    row.append(&play);
    row.append(&wave);
    row.append(&time);

    let stream: Rc<RefCell<Option<gtk::MediaFile>>> = Rc::default();
    wave.set_draw_func({
        let stream = stream.clone();
        move |area, cr, _, height| {
            let played = stream.borrow().as_ref().map_or(0.0, |s| s.timestamp() as f64 / 1e6);
            let progress = if duration > 0.0 { played / duration } else { 0.0 };
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
        let (wave, time) = (wave.downgrade(), time.downgrade());
        let (images, url) = (images.clone(), attachment.url.clone());
        move |button| {
            if let Some(stream) = stream.borrow().as_ref() {
                return if stream.is_playing() { stream.pause() } else { stream.play() };
            }
            button.set_sensitive(false);
            let (stream, button, wave, time) = (stream.clone(), button.downgrade(), wave.clone(), time.clone());
            let (images, url) = (images.clone(), url.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let Some(button) = button.upgrade() else { return };
                button.set_sensitive(true);
                let Some(path) = path else { return };
                let file = gtk::MediaFile::for_filename(&path);
                // Only weak references here: the widgets own the stream.
                file.connect_timestamp_notify(move |file| {
                    let (Some(wave), Some(time)) = (wave.upgrade(), time.upgrade()) else { return };
                    wave.queue_draw();
                    let left = duration - file.timestamp() as f64 / 1e6;
                    time.set_label(&clock(if file.is_ended() { duration } else { left }));
                });
                file.connect_playing_notify({
                    let button = button.downgrade();
                    move |file| {
                        if let Some(button) = button.upgrade() {
                            let (icon, tip) = match file.is_playing() {
                                true => ("media-playback-pause-symbolic", "Pause"),
                                false => ("media-playback-start-symbolic", "Play voice message"),
                            };
                            button.set_icon_name(icon);
                            button.set_tooltip_text(Some(tip));
                        }
                    }
                });
                file.play();
                *stream.borrow_mut() = Some(file);
            });
        }
    });
    row.upcast()
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
fn clock(seconds: f64) -> String {
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
