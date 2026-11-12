//! Embed widgets: cards, bare images, galleries, GIF links and videos. What
//! to show is decided in [`discord::embed`]; this only builds it.

use std::cell::Cell;
use std::rc::Rc;

use discord::animation::Subject;
use discord::embed::{Display, Embed, Large, Media, field_rows};
use discord::settings::MediaSettings;
use gtk::prelude::*;
use gtk::{glib, pango};

use super::favorites::Stars;
use super::media_frame::MediaFrame;
use super::video::{Options, Player, media_file};
use super::viewer::MediaCx;
use crate::images::{Images, Row, Update};
use crate::render::{self, Context, Names};

/// Largest box embed media is scaled into, and its thumbnails (the official
/// client's sizes).
const MAX_W: f64 = 400.0;
const MAX_H: f64 = 300.0;
const THUMBNAIL: f64 = 80.0;
/// Fields sit on a 12-column grid, split evenly between a row's fields.
const FIELD_COLUMNS: i32 = 12;
/// The bar of embeds that set no colour.
const DEFAULT_COLOUR: u32 = 0x1E1F22;

/// The embed at place `n` among the message's. `images` is `None` where
/// nothing may load. With the viewer's `inline_embed_media` off, author and
/// footer icons still show, as in the official client.
pub(super) fn embed_widget(
    embed: &Embed,
    n: usize,
    names: &Names,
    images: Option<&Images>,
    settings: &MediaSettings,
    stars: &Stars,
    view: &MediaCx,
) -> gtk::Widget {
    let show_media = settings.inline_embed_media && images.is_some();
    match (embed.display(show_media), images) {
        (Display::Bare(media), Some(images)) => large(&media, embed, n, images, stars, view),
        (Display::Card { media, thumbnail }, images) => card(embed, n, names, images, media, thumbnail, stars, view),
        // `display` only offers media when told it may show it.
        (Display::Bare(_), None) => card(embed, n, names, None, None, None, stars, view),
    }
}

/// Large media, bare or inside a card, of the embed at place `n`. A GIF that
/// animates has a star.
fn large(media: &Large, embed: &Embed, n: usize, images: &Images, stars: &Stars, view: &MediaCx) -> gtk::Widget {
    match media {
        Large::Image(media) => {
            let frame = image(media, images, MAX_W, MAX_H);
            view.link(&frame, view.embedded(n, 0));
            stars.over(&frame, media.star.as_ref())
        }
        Large::Gallery(media) => gallery(media, n, images, stars, view),
        Large::Gifv { video, still } => stars.over(&gifv(video, still, images), video.star.as_ref()),
        Large::Video { video: Some(video), still } => self::video(video, still, images, view.embedded(n, 0), view),
        Large::Video { video: None, still } => page_video(still, embed.url.as_deref(), images),
    }
}

/// Images gathered from embeds sharing a URL, two to a row.
fn gallery(media: &[Media], n: usize, images: &Images, stars: &Stars, view: &MediaCx) -> gtk::Widget {
    let grid = gtk::Grid::builder().column_spacing(4).row_spacing(4).halign(gtk::Align::Start).build();
    for (i, media) in media.iter().enumerate() {
        let picture = image(media, images, MAX_W / 2.0 - 2.0, MAX_H / 2.0 - 2.0);
        view.link(&picture, view.embedded(n, i));
        grid.attach(&stars.over(&picture, media.star.as_ref()), i as i32 % 2, i as i32 / 2, 1, 1);
    }
    grid.upcast()
}

/// A picture at most `size`, the media fitted into a box, that shrinks with
/// the room it gets and keeps its aspect ratio.
pub(super) struct Fitted {
    /// What to put in the layout.
    pub(super) frame: MediaFrame,
    pub(super) picture: gtk::Picture,
    pub(super) size: (i32, i32),
}

pub(super) fn fitted_picture(width: Option<u32>, height: Option<u32>, max_w: f64, max_h: f64) -> Fitted {
    let (mut w, mut h) = match (width, height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (max_w, max_h),
    };
    let scale = (max_w / w).min(max_h / h).min(1.0);
    w *= scale;
    h *= scale;
    let size = ((w as i32).max(1), (h as i32).max(1));
    let picture = shrinking_picture();
    Fitted { frame: MediaFrame::new(size, &picture), picture, size }
}

/// A picture that fills what it is given.
fn shrinking_picture() -> gtk::Picture {
    gtk::Picture::builder()
        .can_shrink(true)
        .content_fit(gtk::ContentFit::Contain)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["embed-media"])
        .build()
}

fn image(media: &Media, images: &Images, max_w: f64, max_h: f64) -> MediaFrame {
    let fitted = fitted_picture(Some(media.width), Some(media.height), max_w, max_h);
    let original = Some((media.width, media.height));
    if media.animated {
        images.set_sized_moving(&fitted.picture, &media.url, fitted.size, original, Subject::Gif, Row::None);
    } else {
        images.set_sized(&fitted.picture, &media.url, fitted.size, original);
    }
    fitted.frame
}

/// A still that opens its video page in the browser (YouTube and the like,
/// whose players Neucord doesn't embed).
fn page_video(still: &Media, page: Option<&str>, images: &Images) -> gtk::Widget {
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    frame.set_child(Some(&image(still, images, MAX_W, MAX_H)));
    if let Some(page) = page.filter(|u| u.starts_with("https://")) {
        let play = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text(page)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .css_classes(["osd", "circular", "embed-play"])
            .build();
        let page = page.to_owned();
        play.connect_clicked(move |button| {
            let window = button.root().and_downcast::<gtk::Window>();
            gtk::UriLauncher::new(&page).launch(window.as_ref(), gtk::gio::Cancellable::NONE, |_| {});
        });
        frame.add_overlay(&play);
    }
    frame.upcast()
}

/// A GIF link: a muted looping video over its still. It isn't downloaded
/// until the rules for GIFs let it move, and plays only while they say so.
fn gifv(video: &Media, still: &Media, images: &Images) -> gtk::Widget {
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    frame.set_child(Some(&image(still, images, MAX_W, MAX_H)));
    let player = shrinking_picture();
    frame.add_overlay(&player);

    let (latest, loading) = (Rc::new(Cell::new(None::<Update>)), Rc::new(Cell::new(false)));
    images.watch(&frame, Subject::Gif, Row::None, {
        let (player, images, url) = (player.downgrade(), images.clone(), video.url.clone());
        move |update| {
            latest.set(Some(update));
            let Some(player) = player.upgrade() else { return };
            match player.paintable().and_downcast::<gtk::MediaStream>() {
                Some(stream) => settle(&stream, update),
                None if update.moving && !loading.replace(true) => {
                    let (player, images, url) = (player.downgrade(), images.clone(), url.clone());
                    let (latest, loading) = (latest.clone(), loading.clone());
                    glib::spawn_future_local(async move {
                        let path = images.file(&url).await;
                        loading.set(false);
                        let (Some(player), Some(path)) = (player.upgrade(), path) else { return };
                        let stream = media_file(&path);
                        stream.set_loop(true);
                        stream.set_muted(true);
                        player.set_paintable(Some(&stream));
                        if let Some(update) = latest.get() {
                            settle(stream.upcast_ref(), update);
                        }
                    });
                }
                None => {}
            }
        }
    });
    frame.upcast()
}

/// Play `stream` if it should now, else keep it paused, back at its first
/// frame when it isn't to move at all.
fn settle(stream: &gtk::MediaStream, update: Update) {
    if update.plays {
        stream.play();
    } else {
        stream.pause();
        if !update.moving {
            stream.seek(0);
        }
    }
}

/// A video with sound: its still and a play button, fetched only on click.
/// It plays where it is, and its fullscreen button opens the viewer on it.
fn video(video: &Media, still: &Media, images: &Images, index: Option<usize>, view: &MediaCx) -> gtk::Widget {
    let frame = gtk::Overlay::builder().halign(gtk::Align::Start).build();
    let poster = fitted_picture(Some(video.width), Some(video.height), MAX_W, MAX_H);
    images.set_sized(&poster.picture, &still.url, poster.size, Some((still.width, still.height)));
    frame.set_child(Some(&poster.frame));
    let play = gtk::Button::builder()
        .icon_name("media-playback-start-symbolic")
        .tooltip_text("Play")
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .css_classes(["osd", "circular", "embed-play"])
        .build();
    frame.add_overlay(&play);
    view.name(&frame, index);
    play.connect_clicked({
        let frame = frame.downgrade();
        let (images, url, view) = (images.clone(), video.url.clone(), view.clone());
        move |button| {
            button.set_sensitive(false);
            let (frame, button, images, url, view) =
                (frame.clone(), button.downgrade(), images.clone(), url.clone(), view.clone());
            glib::spawn_future_local(async move {
                let path = images.file(&url).await;
                let (Some(frame), Some(button)) = (frame.upgrade(), button.upgrade()) else { return };
                let Some(path) = path else { return button.set_sensitive(true) };
                // Over the poster, which keeps the frame's size.
                frame.remove_overlay(&button);
                let options =
                    Options { autoplay: true, start: 0.0, fullscreen: index.is_some(), classes: &["embed-media"] };
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
#[expect(clippy::too_many_arguments, reason = "what the media of the embed's place needs, the star's and the viewer's")]
fn card(
    embed: &Embed,
    n: usize,
    names: &Names,
    images: Option<&Images>,
    media: Option<Large>,
    thumbnail: Option<Media>,
    stars: &Stars,
    view: &MediaCx,
) -> gtk::Widget {
    let card =
        gtk::Box::builder().halign(gtk::Align::Start).overflow(gtk::Overflow::Hidden).css_classes(["embed"]).build();
    card.append(&colour_bar(embed.bar_colour().unwrap_or(DEFAULT_COLOUR)));
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
    // Only bot cards use markup; a website's text shows as it is.
    if let Some(description) = embed.description.as_deref().filter(|d| !d.is_empty() && embed.shows_description()) {
        match embed.is_rich() {
            true => {
                text.append(&render::render(&discord::markup::parse(description), &Context::new(names, images), false))
            }
            false => text.append(&plain_label(description, "embed-description")),
        }
    }
    if embed.is_rich() && !embed.fields.is_empty() {
        text.append(&fields(embed, names, images));
    }
    top.append(&text);
    if let (Some(images), Some(thumbnail)) = (images, thumbnail) {
        top.append(&image(&thumbnail, images, THUMBNAIL, THUMBNAIL));
    }
    body.append(&top);

    if let (Some(media), Some(images)) = (media, images) {
        body.append(&large(&media, embed, n, images, stars, view));
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

/// Up to three inline fields share a row, evenly; the others take a row.
fn fields(embed: &Embed, names: &Names, images: Option<&Images>) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_spacing(16).row_spacing(8).column_homogeneous(true).build();
    for (row, fields) in field_rows(&embed.fields).into_iter().enumerate() {
        let span = FIELD_COLUMNS / fields.len() as i32;
        for (column, field) in fields.iter().enumerate() {
            let cell = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).build();
            cell.append(&plain_label(&field.name, "embed-field-name"));
            cell.append(&render::render(&discord::markup::parse(&field.value), &Context::new(names, images), false));
            grid.attach(&cell, column as i32 * span, row as i32, span, 1);
        }
    }
    grid
}

pub(super) fn colour_bar(colour: u32) -> gtk::DrawingArea {
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
    images.set_sized(&picture, url, (size, size), None);
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
