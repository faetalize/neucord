//! Small widgets shared across the UI.

use discord::User;
use gtk::prelude::*;

use crate::images::Images;

/// A mention count, or nothing for zero.
pub(super) fn badge(count: u32) -> Option<gtk::Label> {
    (count > 0).then(|| {
        let label = gtk::Label::builder().valign(gtk::Align::Center).css_classes(["badge"]).build();
        set_badge(&label, count);
        label
    })
}

/// Show `count` on a badge, hiding it at zero.
pub(super) fn set_badge(label: &gtk::Label, count: u32) {
    label.set_label(&if count > 99 { "99+".to_owned() } else { count.to_string() });
    label.set_visible(count > 0);
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

/// A header bar title held to the start, with an optional second line,
/// where libadwaita's own centres both.
pub(super) struct Title {
    pub(super) root: gtk::Box,
    title: gtk::Label,
    subtitle: gtk::Label,
}

impl Title {
    pub(super) fn new(title: &str) -> Self {
        let label = |classes: &[&str]| {
            gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .single_line_mode(true)
                .css_classes(classes)
                .build()
        };
        let this = Title {
            root: gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .hexpand(true)
                .css_classes(["title"])
                .build(),
            title: label(&["heading"]),
            subtitle: label(&["caption", "dimmed"]),
        };
        this.root.append(&this.title);
        this.root.append(&this.subtitle);
        this.set(title, None);
        this
    }

    pub(super) fn set(&self, title: &str, subtitle: Option<&str>) {
        self.title.set_label(title);
        self.subtitle.set_label(subtitle.unwrap_or_default());
        self.subtitle.set_visible(subtitle.is_some_and(|s| !s.is_empty()));
    }

    /// A header bar with `before` and then this title at its start.
    pub(super) fn header_bar(&self, before: &[&gtk::Widget]) -> adw::HeaderBar {
        let header = adw::HeaderBar::builder().show_title(false).build();
        for widget in before {
            header.pack_start(*widget);
        }
        header.pack_start(&self.root);
        header
    }
}
