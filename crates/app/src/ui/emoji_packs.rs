//! The emoji packs page of the picker: every pack Discord lists for the
//! account (`EmojiPackStore`, 956396), each with its emoji and a button to
//! pin it to the picker or take it off. Pinning is the account's
//! `PinnedEmojiPacks` setting; what the picker lists of a pack, and what a
//! pick writes, is `discord::emoji`.

use std::cell::RefCell;
use std::rc::Rc;

use adw::prelude::*;
use discord::emoji::{Item, Library};
use discord::emoji_packs::{Fetch, Pack};

use super::App;
use super::emoji::glyph;
use super::picker;

/// What hears an emoji picked, in the picker or on this page.
pub(super) type Picked = Rc<dyn Fn(&Rc<App>, &Item)>;

impl App {
    /// The picker's button to the packs page. It shows once the packs are
    /// fetched and the account has any, so [`Self::show_packs_button`] is
    /// for when they come.
    pub(super) fn packs_button(
        self: &Rc<Self>,
        library: &Rc<RefCell<Library>>,
        picked: &Picked,
        handle: &Rc<RefCell<Option<picker::Handle>>>,
    ) -> gtk::Button {
        let button = gtk::Button::builder()
            .icon_name("view-grid-symbolic")
            .tooltip_text("Emoji Packs")
            .css_classes(["flat"])
            .visible(false)
            .build();
        button.connect_clicked({
            let (this, library, picked, handle) =
                (Rc::downgrade(self), library.clone(), picked.clone(), handle.clone());
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if let Some(handle) = handle.borrow().as_ref() {
                    handle.close();
                }
                this.open_emoji_packs(&library, &picked, &handle);
            }
        });
        self.show_packs_button(&button);
        button
    }

    /// Show the button if there are packs to look at.
    pub(super) fn show_packs_button(&self, button: &gtk::Button) {
        let state = self.account.state.borrow();
        button.set_visible(state.emoji_packs.fetch() == Fetch::Fetched && !state.emoji_packs.packs().is_empty());
    }

    pub(super) fn open_emoji_packs(
        self: &Rc<Self>,
        library: &Rc<RefCell<Library>>,
        picked: &Picked,
        handle: &Rc<RefCell<Option<picker::Handle>>>,
    ) {
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .margin_top(12)
            .margin_bottom(12)
            .margin_start(12)
            .margin_end(12)
            .build();
        let scroller =
            gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.set_content(Some(&scroller));
        let dialog =
            adw::Dialog::builder().title("Emoji Packs").content_width(480).content_height(560).child(&toolbar).build();
        let packs: Vec<Pack> = self.account.state.borrow().emoji_packs.packs().to_vec();
        for pack in &packs {
            list.append(&self.pack_section(pack, library, picked, handle, &dialog));
        }
        dialog.present(Some(&self.window));
    }

    /// A pack: its name, how many emoji it has, the button to pin it, and
    /// its emoji, which pick as in the picker. An unavailable pack's are dimmed.
    fn pack_section(
        self: &Rc<Self>,
        pack: &Pack,
        library: &Rc<RefCell<Library>>,
        picked: &Picked,
        handle: &Rc<RefCell<Option<picker::Handle>>>,
        dialog: &adw::Dialog,
    ) -> gtk::Widget {
        let section = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).build();
        let header = gtk::Box::builder().spacing(8).build();
        let title = gtk::Label::builder().label(&pack.name).xalign(0.0).css_classes(["heading"]).build();
        let count = gtk::Label::builder()
            .label(format!("{}", pack.emojis.len()))
            .css_classes(["dim-label"])
            .hexpand(true)
            .xalign(0.0)
            .build();
        let pin = gtk::Button::builder().css_classes(["flat"]).build();
        let update = {
            let (this, pin, id) = (Rc::downgrade(self), pin.downgrade(), pack.id);
            move || {
                let (Some(this), Some(pin)) = (this.upgrade(), pin.upgrade()) else { return };
                let state = this.account.state.borrow();
                let pinned = state.frecency.pinned_emoji_packs.contains(&id.0);
                pin.set_label(if pinned { "Unpin" } else { "Pin" });
                pin.set_sensitive(pinned || state.emoji_packs.pack(id).is_some_and(|p| p.available));
            }
        };
        update();
        pin.connect_clicked({
            let (this, library, handle, update, id) =
                (Rc::downgrade(self), library.clone(), handle.clone(), update, pack.id);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                let pinned = this.account.state.borrow().frecency.pinned_emoji_packs.contains(&id.0);
                this.pin_emoji_pack(id, !pinned);
                this.refresh_emoji_packs(&library);
                if let Some(handle) = handle.borrow().as_ref() {
                    handle.reload();
                }
                update();
            }
        });
        header.append(&title);
        header.append(&count);
        header.append(&pin);
        section.append(&header);
        if let Some(description) = pack.description.as_deref().filter(|d| !d.is_empty()) {
            section.append(
                &gtk::Label::builder().label(description).xalign(0.0).wrap(true).css_classes(["dim-label"]).build(),
            );
        }
        let grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(6)
            .max_children_per_line(12)
            .homogeneous(true)
            .build();
        for emoji in &pack.emojis {
            let item = {
                let state = self.account.state.borrow();
                library.borrow().pack_item(pack, emoji, &state.limits())
            };
            let button = gtk::Button::builder()
                .child(&glyph(&item, &self.images))
                .css_classes(["flat"])
                .tooltip_text(format!(":{}:", emoji.name))
                .build();
            if item.unavailable().is_some() {
                button.set_opacity(0.4);
            } else {
                let (this, picked, dialog) = (Rc::downgrade(self), picked.clone(), dialog.downgrade());
                button.connect_clicked(move |_| {
                    let (Some(this), Some(dialog)) = (this.upgrade(), dialog.upgrade()) else { return };
                    picked(&this, &item);
                    if !this.shift_held() {
                        dialog.close();
                    }
                });
            }
            grid.append(&button);
        }
        section.append(&grid);
        section.upcast()
    }
}
