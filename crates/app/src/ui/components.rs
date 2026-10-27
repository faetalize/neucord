//! Message components, shown but not yet usable: buttons that would act
//! and menus are insensitive. Link buttons open their link.

use discord::component::{Button, ButtonStyle, Component, Media};
use gtk::prelude::*;
use gtk::{gio, pango};

use super::embeds::{behind_spoiler, colour_bar, fitted_picture};
use super::polls::emoji_widget;
use crate::images::Images;
use crate::render::{self, Context};

/// Media in a gallery, or alone, and thumbnails beside text (the official
/// client's sizes for embeds, which these follow).
const MAX_W: f64 = 400.0;
const MAX_H: f64 = 300.0;
const THUMBNAIL: f64 = 80.0;
/// What a button that would do something says it can't.
const NOT_YET: &str = "Neucord can't use this yet";

/// The components of a message, one under another.
pub(super) fn components(components: &[Component], cx: &Context) -> gtk::Widget {
    let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).build();
    for component in components {
        if let Some(widget) = component_widget(component, cx) {
            column.append(&widget);
        }
    }
    column.upcast()
}

fn component_widget(component: &Component, cx: &Context) -> Option<gtk::Widget> {
    let images = cx.images;
    Some(match component {
        Component::Row(children) => {
            let row = gtk::Box::builder().spacing(8).halign(gtk::Align::Start).build();
            for child in children {
                if let Some(widget) = component_widget(child, cx) {
                    row.append(&widget);
                }
            }
            row.upcast()
        }
        Component::Button(button) => button_widget(button, images),
        Component::Select { placeholder, options, .. } => {
            // What it would show: the options picked by default, or what to do.
            let picked: Vec<&str> = options.iter().filter(|o| o.default).map(|o| o.label.as_str()).collect();
            let label = match (picked.is_empty(), placeholder) {
                (false, _) => picked.join(", "),
                (true, Some(placeholder)) => placeholder.clone(),
                (true, None) => "Make a selection".to_owned(),
            };
            gtk::MenuButton::builder().label(label).sensitive(false).tooltip_text(NOT_YET).build().upcast()
        }
        Component::Section { components: children, accessory } => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
            let text = components(children, cx);
            text.set_hexpand(true);
            row.append(&text);
            if let Some(widget) = accessory.as_deref().and_then(|a| component_widget(a, cx)) {
                widget.set_valign(gtk::Align::Start);
                row.append(&widget);
            }
            row.upcast()
        }
        Component::Text(content) => render::render(&discord::markup::parse(content), cx, false),
        Component::Thumbnail(media) => picture(media, images?, THUMBNAIL, THUMBNAIL),
        Component::Gallery(items) => {
            // One item large; more, two to a row.
            let grid = gtk::Grid::builder().column_spacing(4).row_spacing(4).halign(gtk::Align::Start).build();
            let (w, h) = if items.len() == 1 { (MAX_W, MAX_H) } else { (MAX_W / 2.0 - 2.0, MAX_H / 2.0 - 2.0) };
            for (n, media) in items.iter().enumerate() {
                grid.attach(&picture(media, images?, w, h), n as i32 % 2, n as i32 / 2, 1, 1);
            }
            grid.upcast()
        }
        Component::File { media, name, size } => {
            let name = name.as_deref().unwrap_or_else(|| media.url.rsplit('/').next().unwrap_or_default());
            let mut markup = format!(
                "<a href=\"{}\">📎 {}</a>",
                glib::markup_escape_text(&media.url),
                glib::markup_escape_text(name)
            );
            if let Some(size) = size.filter(|&s| s > 0) {
                markup += &format!("  <span fgalpha=\"55%\" size=\"small\">{}</span>", glib::format_size(size));
            }
            gtk::Label::builder().use_markup(true).label(markup).xalign(0.0).build().upcast()
        }
        Component::Separator { divider, large } => {
            let margin = if *large { 8 } else { 2 };
            let separator = gtk::Separator::builder().margin_top(margin).margin_bottom(margin).build();
            separator.set_opacity(if *divider { 1.0 } else { 0.0 });
            separator.upcast()
        }
        Component::Container { accent, spoiler, components: children } => {
            // Drawn as an embed is, which it stands in for.
            let card = gtk::Box::builder()
                .halign(gtk::Align::Start)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["embed"])
                .build();
            if let Some(accent) = accent {
                card.append(&colour_bar(*accent));
            }
            let body = components(children, cx);
            body.add_css_class("embed-body");
            card.append(&body);
            if *spoiler {
                let card = card.upcast::<gtk::Widget>();
                return Some(behind_spoiler(move || card.clone()));
            }
            card.upcast()
        }
        Component::Unknown => return None,
    })
}

fn button_widget(button: &Button, images: Option<&Images>) -> gtk::Widget {
    let content = gtk::Box::builder().spacing(6).halign(gtk::Align::Center).build();
    if let (Some(emoji), Some(images)) = (&button.emoji, images) {
        content.append(&emoji_widget(emoji, images));
    }
    if let Some(label) = &button.label {
        content.append(&gtk::Label::builder().label(label).ellipsize(pango::EllipsizeMode::End).build());
    }
    let widget = gtk::Button::builder().child(&content).build();
    // libadwaita's own kinds of button, where it has one.
    match button.style {
        ButtonStyle::Primary => widget.add_css_class("suggested-action"),
        ButtonStyle::Destructive => widget.add_css_class("destructive-action"),
        _ => {}
    }
    match button.url.as_deref().filter(|url| button.style == ButtonStyle::Link && url.starts_with("https://")) {
        Some(url) if !button.disabled => {
            widget.set_tooltip_text(Some(url));
            let url = url.to_owned();
            widget.connect_clicked(move |widget| {
                let window = widget.root().and_downcast::<gtk::Window>();
                gtk::UriLauncher::new(&url).launch(window.as_ref(), gio::Cancellable::NONE, |_| {});
            });
        }
        _ => {
            widget.set_sensitive(false);
            if !button.disabled {
                widget.set_tooltip_text(Some(NOT_YET));
            }
        }
    }
    widget.upcast()
}

/// An image a component shows, fitted into `max_w` × `max_h`, behind a
/// cover if it is a spoiler.
fn picture(media: &Media, images: &Images, max_w: f64, max_h: f64) -> gtk::Widget {
    let build = {
        let (media, images) = (media.clone(), images.clone());
        move || {
            let fitted = fitted_picture(media.width, media.height, max_w, max_h);
            if let Some(description) = &media.description {
                fitted.picture.set_alternative_text(Some(description));
                fitted.picture.set_tooltip_text(Some(description));
            }
            let original = media.width.zip(media.height);
            images.set_sized(&fitted.picture, &media.url, fitted.size, original, false);
            fitted.frame.upcast()
        }
    };
    if media.spoiler { behind_spoiler(build) } else { build() }
}
