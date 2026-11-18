//! The cover over media the sensitive filter hides (see
//! [`discord::sensitive`]): the client's "May contain sensitive content" over
//! the media's place, which a click reveals. Like a spoiler's, the media is
//! built only once revealed, so it isn't fetched before. A cover for a
//! flagged item has the client's eye button, which hides it again.

use std::cell::Cell;
use std::rc::Rc;

use discord::sensitive::Cover;
use gtk::prelude::*;

/// What the cover and its button say (the client's strings).
const WARNING: &str = "May contain sensitive content";
const LABEL: &str = "Content Warning: This may contain sensitive content";
const REVEAL: &str = "Reveal sensitive content";

/// `reveal`'s widget behind a cover as big as `size`, if the media's is
/// known.
pub(super) fn covered(
    cover: Cover,
    size: Option<(i32, i32)>,
    reveal: impl Fn() -> gtk::Widget + 'static,
) -> gtk::Widget {
    let stack = gtk::Stack::builder().halign(gtk::Align::Start).hhomogeneous(false).vhomogeneous(false).build();
    let button = gtk::Button::builder().css_classes(["sensitive-cover"]).build();
    button.update_property(&[gtk::accessible::Property::Label(LABEL)]);
    if let Some((width, height)) = size {
        button.set_size_request(width, height);
    }
    // A scan that hasn't looked yet covers without a word.
    if matches!(cover, Cover::Flagged(_)) {
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).halign(gtk::Align::Center);
        let content = content.valign(gtk::Align::Center).build();
        content.append(&gtk::Image::builder().icon_name("dialog-warning-symbolic").pixel_size(32).build());
        content.append(&gtk::Label::builder().label(WARNING).css_classes(["caption"]).wrap(true).build());
        button.set_child(Some(&content));
    }
    stack.add_named(&button, Some("cover"));

    let toggle = matches!(cover, Cover::Flagged(_)).then(|| {
        gtk::Button::builder()
            .icon_name("view-conceal-symbolic")
            .tooltip_text(REVEAL)
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(8)
            .margin_end(8)
            .css_classes(["osd", "circular"])
            .build()
    });
    let shown = Rc::new(Cell::new(false));
    let set = {
        let (stack, toggle, shown) = (stack.downgrade(), toggle.as_ref().map(|t| t.downgrade()), shown.clone());
        Rc::new(move |visible: bool| {
            let Some(stack) = stack.upgrade() else { return };
            let media = stack.child_by_name("media");
            if visible && media.is_none() {
                stack.add_named(&reveal(), Some("media"));
            }
            stack.set_visible_child_name(if visible { "media" } else { "cover" });
            // Hidden again, it stops with whatever it was doing.
            if let Some(media) = media.filter(|_| !visible) {
                stack.remove(&media);
            }
            shown.set(visible);
            if let Some(toggle) = toggle.as_ref().and_then(|t| t.upgrade()) {
                toggle.set_icon_name(if visible { "view-reveal-symbolic" } else { "view-conceal-symbolic" });
            }
        })
    };
    button.connect_clicked({
        let set = set.clone();
        move |_| set(true)
    });
    let Some(toggle) = toggle else { return stack.upcast() };
    toggle.connect_clicked(move |_| set(!shown.get()));
    let overlay = gtk::Overlay::builder().halign(gtk::Align::Start).child(&stack).build();
    overlay.add_overlay(&toggle);
    overlay.upcast()
}
