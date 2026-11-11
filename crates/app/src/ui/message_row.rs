//! A row of the message list. The list keeps a few dozen and hands them from
//! message to message as it scrolls, so a row is built once and each bind
//! only fills it in. Parts few messages need are made the first time one
//! does, then kept.

use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use discord::{Message, Snowflake};
use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;

use crate::timelines::Run;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MessageRow {
        pub(in crate::ui) parts: OnceCell<super::Parts>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MessageRow {
        const NAME: &'static str = "NeucordMessageRow";
        type Type = super::MessageRow;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for MessageRow {}
    impl WidgetImpl for MessageRow {}
    impl BoxImpl for MessageRow {}
}

glib::wrapper! {
    pub struct MessageRow(ObjectSubclass<imp::MessageRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

pub(super) struct Parts {
    /// The list item the row belongs to, which says what it shows.
    pub(super) item: glib::WeakRef<gtk::ListItem>,
    /// A date where a new day starts, and "New" above the first unread message.
    pub(super) divider: gtk::Box,
    divider_parts: OnceCell<Divider>,
    /// Stands in for a run of blocked users' messages.
    pub(super) bar: gtk::Box,
    pub(super) bar_parts: OnceCell<(gtk::Label, gtk::Button)>,
    /// The run the bar stands for.
    pub(super) run: Cell<Option<Run>>,
    /// The message itself.
    pub(super) row: gtk::Box,
    /// The line above a reply.
    pub(super) reply: RefCell<Option<gtk::Widget>>,
    pub(super) gutter: gtk::Box,
    /// The id of the first message of the group the row's message is in, for
    /// the avatar to move when any message of it is under the pointer.
    pub(super) group: Rc<Cell<u64>>,
    /// The avatar, and whose (id and picture) it is.
    avatar: RefCell<Option<(Snowflake, Option<String>, gtk::Widget)>>,
    /// A system message's icon, in the avatar's place.
    icon: OnceCell<gtk::Image>,
    pub(super) header: gtk::Box,
    pub(super) name: gtk::Label,
    pub(super) time: gtk::Label,
    /// What the message says and shows.
    pub(super) content: gtk::Box,
    /// The box for the thread the message started, below the content, and
    /// what it holds.
    pub(super) thread_slot: gtk::Box,
    pub(super) thread: RefCell<Option<gtk::Widget>>,
    pub(super) edit_button: gtk::Button,
    pub(super) reply_button: gtk::Button,
    pub(super) more: gtk::Button,
    /// The message the content and reply line were made from, and the
    /// [`super::App`]'s `row_epoch` then.
    pub(super) shown: RefCell<Option<(Message, u64)>>,
}

pub(super) struct Divider {
    date: gtk::Label,
    /// The line after the date, which "New" alone goes without.
    date_line: gtk::Separator,
    new: gtk::Label,
}

impl MessageRow {
    pub(super) fn new(item: &gtk::ListItem) -> Self {
        let cell: Self = glib::Object::builder().property("orientation", gtk::Orientation::Vertical).build();
        let divider =
            gtk::Box::builder().visible(false).spacing(8).margin_top(12).margin_start(18).margin_end(18).build();
        let bar = gtk::Box::builder().visible(false).css_classes(["blocked-bar"]).build();
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        cell.append(&divider);
        cell.append(&bar);
        cell.append(&row);

        let main = gtk::Box::builder().spacing(16).css_classes(["message-main"]).build();
        let gutter = gtk::Box::builder().width_request(40).valign(gtk::Align::Start).build();
        main.append(&gutter);
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).hexpand(true).build();
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let name = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).css_classes(["heading"]).build();
        let time = gtk::Label::builder().css_classes(["caption", "dimmed"]).build();
        header.append(&name);
        header.append(&time);
        body.append(&header);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        body.append(&content);
        let thread_slot = gtk::Box::new(gtk::Orientation::Vertical, 0);
        body.append(&thread_slot);
        main.append(&body);

        // Over the row, not in it: as part of the layout they would make
        // every one-line message as tall as the buttons.
        let actions = gtk::Box::builder().halign(gtk::Align::End).valign(gtk::Align::Start).build();
        let edit_button = gtk::Button::builder()
            .icon_name("document-edit-symbolic")
            .tooltip_text("Edit")
            .css_classes(["flat", "message-action"])
            .build();
        let reply_button = gtk::Button::builder()
            .icon_name("mail-reply-sender-symbolic")
            .tooltip_text("Reply (Shift: without pinging)")
            .css_classes(["flat", "message-action"])
            .build();
        let more = gtk::Button::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More")
            // Leaving the focus where it is keeps a selection to copy.
            .focus_on_click(false)
            .css_classes(["flat", "message-action"])
            .build();
        actions.append(&edit_button);
        actions.append(&reply_button);
        actions.append(&more);
        let overlay = gtk::Overlay::builder().child(&main).build();
        overlay.add_overlay(&actions);
        row.append(&overlay);

        let parts = Parts {
            item: item.downgrade(),
            divider,
            divider_parts: OnceCell::new(),
            bar,
            bar_parts: OnceCell::new(),
            run: Cell::new(None),
            row,
            reply: RefCell::new(None),
            gutter,
            group: Rc::default(),
            avatar: RefCell::new(None),
            icon: OnceCell::new(),
            header,
            name,
            time,
            content,
            thread_slot,
            thread: RefCell::new(None),
            edit_button,
            reply_button,
            more,
            shown: RefCell::new(None),
        };
        assert!(cell.imp().parts.set(parts).is_ok());
        cell
    }

    pub(super) fn parts(&self) -> &Parts {
        self.imp().parts.get().expect("built in new")
    }
}

impl Parts {
    /// Show the divider for a new day, with its `date`, and "New", or hide it
    /// for neither.
    pub(super) fn set_divider(&self, new_day: bool, date: Option<&str>, new: bool) {
        self.divider.set_visible(new_day || new);
        if !self.divider.is_visible() {
            return;
        }
        let parts = self.divider_parts.get_or_init(|| {
            let line = || gtk::Separator::builder().hexpand(true).valign(gtk::Align::Center).build();
            let date = gtk::Label::builder().css_classes(["caption", "dimmed"]).build();
            let date_line = line();
            let new = gtk::Label::builder().label("New").css_classes(["caption-heading", "accent"]).build();
            self.divider.append(&line());
            self.divider.append(&date);
            self.divider.append(&date_line);
            self.divider.append(&new);
            Divider { date, date_line, new }
        });
        parts.date.set_label(date.unwrap_or_default());
        parts.date.set_visible(date.is_some());
        parts.date_line.set_visible(date.is_some());
        parts.new.set_visible(new);
    }

    /// Show `user`'s avatar in the gutter, making it if it's someone else's.
    pub(super) fn show_avatar(&self, user: &discord::User, make: impl FnOnce() -> gtk::Widget) {
        let mut avatar = self.avatar.borrow_mut();
        let current = avatar.as_ref().filter(|(id, hash, _)| *id == user.id && *hash == user.avatar);
        if current.is_none() {
            if let Some((_, _, old)) = avatar.take() {
                self.gutter.remove(&old);
            }
            let widget = make();
            self.gutter.append(&widget);
            *avatar = Some((user.id, user.avatar.clone(), widget));
        }
        if let Some((_, _, widget)) = avatar.as_ref() {
            widget.set_visible(true);
        }
        if let Some(icon) = self.icon.get() {
            icon.set_visible(false);
        }
    }

    /// Whether the gutter shows an avatar: of the first message of a run.
    pub(super) fn shows_avatar(&self) -> bool {
        self.avatar.borrow().as_ref().is_some_and(|(_, _, widget)| widget.is_visible())
    }

    /// Show a system message's icon in the gutter.
    pub(super) fn show_icon(&self, name: &str) {
        self.hide_gutter();
        let icon = self.icon.get_or_init(|| {
            let icon = gtk::Image::builder().halign(gtk::Align::End).build();
            self.gutter.append(&icon);
            icon
        });
        icon.set_icon_name(Some(name));
        icon.set_visible(true);
    }

    /// Leave the gutter empty, as for a message grouped under the one before.
    pub(super) fn hide_gutter(&self) {
        if let Some((_, _, widget)) = self.avatar.borrow().as_ref() {
            widget.set_visible(false);
        }
        if let Some(icon) = self.icon.get() {
            icon.set_visible(false);
        }
    }
}
