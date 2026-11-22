//! Icons Adwaita doesn't have, drawn for Neucord. `gtk::Svg` draws them in
//! the text color as it does themed symbolic icons, and needs neither a
//! compiled resource nor an installed theme.

/// A sticker: a rounded square with its corner peeling off.
pub fn sticker() -> gtk::Svg {
    gtk::Svg::from_bytes(&glib::Bytes::from_static(include_bytes!("icons/sticker-symbolic.svg")))
}
