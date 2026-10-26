//! Small widgets shared across the UI.

use discord::User;
use gtk::prelude::*;

use crate::images::Images;

/// Avatar/name colour classes in style.css.
pub(super) const PALETTE_SIZE: u64 = 8;

/// A red mention count, or nothing for zero.
pub(super) fn badge(count: u32) -> Option<gtk::Label> {
    (count > 0).then(|| {
        let text = if count > 99 { "99+".to_owned() } else { count.to_string() };
        gtk::Label::builder().label(text).valign(gtk::Align::Center).css_classes(["badge"]).build()
    })
}

pub(super) fn initials(name: &str) -> String {
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(3).collect();
    if letters.is_empty() { "?".into() } else { letters }
}

pub(super) fn palette_index(user: &User) -> u64 {
    user.id.0 % PALETTE_SIZE
}

/// A round avatar Picture of `size` px (with the coloured initial behind it
/// while it loads), or just the initial when the user has no avatar.
pub(super) fn avatar_widget(user: &User, colour: u64, size: i32, images: Option<&Images>) -> gtk::Widget {
    let initial = user.display_name().chars().next().unwrap_or('?').to_uppercase().to_string();
    let fallback = gtk::Label::builder()
        .label(initial)
        .width_request(size)
        .height_request(size)
        .css_classes(["avatar", &format!("avatar-{colour}")])
        .build();
    let (Some(hash), Some(images)) = (user.avatar.as_deref(), images) else {
        return fallback.upcast();
    };
    // Overlay the picture on the coloured initial so there is no blank gap.
    let overlay = gtk::Overlay::builder().child(&fallback).build();
    let picture = gtk::Picture::builder()
        .width_request(size)
        .height_request(size)
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["avatar"])
        .build();
    images.set(&picture, &discord::cdn::avatar_url(user.id.0, hash, 64), discord::cdn::is_animated(hash));
    overlay.add_overlay(&picture);
    overlay.upcast()
}
