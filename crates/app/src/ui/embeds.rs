//! Embed widgets: cards, bare images, GIF links and videos. What to show is
//! decided in [`discord::embed`]; this only builds it.

use discord::embed::{CardMedia, Display, Embed, Media, field_rows};
use discord::settings::MediaSettings;
use gtk::prelude::*;
use gtk::{glib, pango};

use crate::images::Images;
use crate::render::{self, Names};

/// Largest box an inline image or video is scaled into.
pub(super) const MAX_W: f64 = 400.0;
pub(super) const MAX_H: f64 = 300.0;
const THUMBNAIL: f64 = 80.0;
/// The bar of embeds that set no colour.
const DEFAULT_COLOUR: u32 = 0x1E1F22;

/// One embed. `images` is `None` where media must not load.
pub(super) fn embed_widget(
    embed: &Embed,
    names: &Names,
    images: Option<&Images>,
    settings: &MediaSettings,
) -> gtk::Widget {
    let images = images.filter(|_| settings.inline_embed_media);
    let autoplay = settings.gif_auto_play;
    match (embed.display(images.is_some()), images) {
        (Display::Image(media), Some(images)) => image(&media, images, autoplay, MAX_W, MAX_H).upcast(),
        (Display::Gifv { video, still }, Some(images)) => gifv(&video, still.as_ref(), images, autoplay),
        (Display::Card { media, thumbnail }, images) => card(embed, names, images, autoplay, media, thumbnail),
        // `display` only offers media when told it may show it.
        (_, None) => card(embed, names, None, autoplay, None, None),
    }
}

/// A picture sized to fit `max_w`×`max_h`, keeping the aspect ratio.
pub(super) fn fitted_picture(width: Option<u32>, height: Option<u32>, max_w: f64, max_h: f64) -> gtk::Picture {
    let (mut w, mut h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (max_w, max_h),
    };
    let scale = (max_w / w).min(max_h / h).min(1.0);
    w *= scale;
    h *= scale;
    gtk::Picture::builder()
        .width_request(w as i32)
        .height_request(h as i32)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .content_fit(gtk::ContentFit::Contain)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["embed-media"])
        .build()
}

fn image(media: &Media, images: &Images, autoplay: bool, max_w: f64, max_h: f64) -> gtk::Picture {
    let picture = fitted_picture(media.width, media.height, max_w, max_h);
    images.set_sized(&picture, &media.url, media.width.zip(media.height), autoplay && media.animated);
    picture
}

/// A GIF link: a muted looping video over its still. Without autoplay it
/// plays while hovered, and isn't downloaded until then.
fn gifv(video: &Media, still: Option<&Media>, images: &Images, autoplay: bool) -> gtk::Widget {
    let size = |m: &Media| (m.width, m.height);
    let (width, height) = size(video);
    let (width, height) = if width.is_some() { (width, height) } else { still.map_or((None, None), size) };
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    let poster = fitted_picture(width, height, MAX_W, MAX_H);
    if let Some(still) = still {
        images.set_sized(&poster, &still.url, still.width.zip(still.height), false);
    }
    frame.set_child(Some(&poster));
    let player = fitted_picture(width, height, MAX_W, MAX_H);
    frame.add_overlay(&player);

    if autoplay {
        load_gifv(&player, images, &video.url, None);
    } else {
        let hover = gtk::EventControllerMotion::new();
        hover.connect_enter({
            let (player, images, url) = (player.clone(), images.clone(), video.url.clone());
            move |hover, _, _| match player.paintable().and_downcast::<gtk::MediaStream>() {
                Some(stream) => stream.play(),
                None => load_gifv(&player, &images, &url, Some(hover.clone())),
            }
        });
        hover.connect_leave({
            let player = player.clone();
            move |_| {
                if let Some(stream) = player.paintable().and_downcast::<gtk::MediaStream>() {
                    stream.pause();
                }
            }
        });
        frame.add_controller(hover);
    }
    frame.upcast()
}

/// Load a GIF link's video into `player` and play it, or with `hover`, only
/// if the pointer is still there once it has loaded.
fn load_gifv(player: &gtk::Picture, images: &Images, url: &str, hover: Option<gtk::EventControllerMotion>) {
    let player = player.downgrade();
    let (images, url) = (images.clone(), url.to_owned());
    glib::spawn_future_local(async move {
        let Some(path) = images.file(&url).await else { return };
        let Some(player) = player.upgrade() else { return };
        let stream = gtk::MediaFile::for_filename(&path);
        stream.set_loop(true);
        stream.set_muted(true);
        if hover.is_none_or(|hover| hover.contains_pointer()) {
            stream.play();
        }
        player.set_paintable(Some(&stream));
    });
}

/// A video with sound: its still and a play button, fetched only on click.
fn video(video: &Media, still: Option<&Media>, images: &Images) -> gtk::Widget {
    let (width, height) = match still {
        Some(still) if video.width.is_none() => (still.width, still.height),
        _ => (video.width, video.height),
    };
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    let poster = fitted_picture(width, height, MAX_W, MAX_H);
    if let Some(still) = still {
        images.set_sized(&poster, &still.url, still.width.zip(still.height), false);
    }
    frame.set_child(Some(&poster));
    let play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Play")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["osd", "circular", "embed-play"])
        .build();
    frame.add_overlay(&play);
    play.connect_clicked({
        let frame = frame.downgrade();
        let (images, url) = (images.clone(), video.url.clone());
        let (w, h) = (poster.width_request(), poster.height_request());
        move |button| {
            button.set_sensitive(false);
            let (frame, button, images, url) = (frame.clone(), button.downgrade(), images.clone(), url.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let (Some(frame), Some(button)) = (frame.upgrade(), button.upgrade()) else { return };
                let Some(path) = path else { return button.set_sensitive(true) };
                frame.remove_overlay(&button);
                let player = gtk::Video::builder().autoplay(true).width_request(w).height_request(h).build();
                player.add_css_class("embed-media");
                player.set_filename(Some(&path));
                frame.set_child(Some(&player));
            });
        }
    });
    frame.upcast()
}

/// A cover that shows what `reveal` builds only once clicked, so spoilered
/// media isn't even fetched before then.
pub(super) fn behind_spoiler(reveal: impl Fn() -> gtk::Widget + 'static) -> gtk::Widget {
    let holder = gtk::Box::builder().halign(gtk::Align::Start).build();
    let cover =
        gtk::Button::builder().label("Spoiler").tooltip_text("Show spoiler").css_classes(["spoiler-cover"]).build();
    cover.connect_clicked({
        let holder = holder.downgrade();
        move |cover| {
            if let Some(holder) = holder.upgrade() {
                holder.remove(cover);
                holder.append(&reveal());
            }
        }
    });
    holder.append(&cover);
    holder.upcast()
}

/// A card: colour bar, then provider, author, title, description, fields,
/// media and footer, with the thumbnail to the right of the text.
fn card(
    embed: &Embed,
    names: &Names,
    images: Option<&Images>,
    autoplay: bool,
    media: Option<CardMedia>,
    thumbnail: Option<Media>,
) -> gtk::Widget {
    let card =
        gtk::Box::builder().halign(gtk::Align::Start).overflow(gtk::Overflow::Hidden).css_classes(["embed"]).build();
    card.append(&colour_bar(embed.color.unwrap_or(DEFAULT_COLOUR)));
    let body =
        gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).css_classes(["embed-body"]).build();
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();

    if let Some(provider) = embed.provider.as_ref().and_then(|p| Some((p.name.as_deref()?, p.url.as_deref()))) {
        text.append(&linked_label(provider.0, provider.1, "embed-provider"));
    }
    if let Some(author) = embed.author.as_ref().filter(|a| !a.name.is_empty()) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let (Some(images), Some(icon)) = (images, author.proxy_icon_url.as_deref()) {
            row.append(&icon_picture(images, icon, 24, "embed-author-icon"));
        }
        row.append(&linked_label(&author.name, author.url.as_deref(), "embed-author"));
        text.append(&row);
    }
    if let Some(title) = embed.title.as_deref().filter(|t| !t.is_empty()) {
        text.append(&linked_label(title, embed.url.as_deref(), "embed-title"));
    }
    if let Some(description) = embed.description.as_deref().filter(|d| !d.is_empty()) {
        text.append(&render::render(&discord::markup::parse(description), names, false));
    }
    if !embed.fields.is_empty() {
        text.append(&fields(embed, names));
    }
    top.append(&text);
    if let (Some(images), Some(thumbnail)) = (images, thumbnail) {
        top.append(&image(&thumbnail, images, autoplay, THUMBNAIL, THUMBNAIL));
    }
    body.append(&top);

    match (media, images) {
        (Some(CardMedia::Image(media)), Some(images)) => body.append(&image(&media, images, autoplay, MAX_W, MAX_H)),
        (Some(CardMedia::Video { video: media, still }), Some(images)) => {
            body.append(&video(&media, still.as_ref(), images))
        }
        _ => {}
    }

    let footer_text = embed.footer.as_ref().map(|f| f.text.as_str()).filter(|t| !t.is_empty());
    let time = embed.timestamp.as_deref().and_then(super::chat::parse_time).map(|t| super::chat::format_time(&t));
    if footer_text.is_some() || time.is_some() {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let (Some(images), Some(icon)) = (images, embed.footer.as_ref().and_then(|f| f.proxy_icon_url.as_deref())) {
            row.append(&icon_picture(images, icon, 20, "embed-footer-icon"));
        }
        let line: Vec<&str> = footer_text.into_iter().chain(time.as_deref()).collect();
        row.append(&plain_label(&line.join(" • "), "embed-footer"));
        body.append(&row);
    }
    card.append(&body);
    card.upcast()
}

/// Inline fields share a row three to a row; the others span it.
fn fields(embed: &Embed, names: &Names) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_spacing(16).row_spacing(8).column_homogeneous(true).build();
    for (row, fields) in field_rows(&embed.fields).into_iter().enumerate() {
        let span = if fields.len() == 1 && !fields[0].inline { 3 } else { 1 };
        for (column, field) in fields.iter().enumerate() {
            let cell = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
            cell.append(&plain_label(&field.name, "embed-field-name"));
            cell.append(&render::render(&discord::markup::parse(&field.value), names, false));
            grid.attach(&cell, column as i32, row as i32, span, 1);
        }
    }
    grid
}

fn colour_bar(colour: u32) -> gtk::DrawingArea {
    let bar = gtk::DrawingArea::builder().width_request(4).vexpand(true).build();
    let channel = |shift: u32| ((colour >> shift) & 0xFF) as f64 / 255.0;
    let (r, g, b) = (channel(16), channel(8), channel(0));
    bar.set_draw_func(move |_, cr, _, _| {
        cr.set_source_rgb(r, g, b);
        let _ = cr.paint();
    });
    bar
}

fn icon_picture(images: &Images, url: &str, size: i32, class: &str) -> gtk::Picture {
    let picture = gtk::Picture::builder()
        .width_request(size)
        .height_request(size)
        .valign(gtk::Align::Center)
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes([class])
        .build();
    images.set_sized(&picture, url, None, false);
    picture
}

fn plain_label(text: &str, class: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .selectable(true)
        .css_classes([class])
        .build()
}

/// Text that links to `url` when there is one.
fn linked_label(text: &str, url: Option<&str>, class: &str) -> gtk::Label {
    let label = plain_label(text, class);
    if let Some(url) = url.filter(|u| u.starts_with("https://") || u.starts_with("http://")) {
        let escape = |s: &str| glib::markup_escape_text(s).to_string();
        label.set_markup(&format!("<a href=\"{}\">{}</a>", escape(url), escape(text)));
    }
    label
}
