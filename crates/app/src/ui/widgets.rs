//! Small widgets shared across the UI.

use discord::User;
use gtk::prelude::*;

use crate::images::Images;

/// A mention count, or nothing for zero.
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

/// A round avatar of `size` px: the user's picture over their initials, which
/// show while it loads, or the initials alone when they have no picture.
pub(super) fn avatar_widget(user: &User, size: i32, images: Option<&Images>) -> gtk::Widget {
    let initials = adw::Avatar::new(size, Some(user.display_name()), true);
    let (Some(hash), Some(images)) = (user.avatar.as_deref(), images) else {
        return initials.upcast();
    };
    let overlay = gtk::Overlay::builder().child(&initials).build();
    let picture = round_picture(size);
    images.set(&picture, &discord::cdn::avatar_url(user.id.0, hash, 64), discord::cdn::is_animated(hash));
    overlay.add_overlay(&picture);
    overlay.upcast()
}

/// An empty round picture of `size` px, for an avatar or icon to load into.
pub(super) fn round_picture(size: i32) -> gtk::Picture {
    gtk::Picture::builder()
        .width_request(size)
        .height_request(size)
        .content_fit(gtk::ContentFit::Cover)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["avatar"])
        .build()
}
