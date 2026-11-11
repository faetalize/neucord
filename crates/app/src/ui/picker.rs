//! What the emoji, sticker and GIF pickers share: a popover with a search box
//! and a scrolling list of sections, each a heading and rows of cells. What
//! the cells are, and what picking one does, is for each picker to say.

use std::rc::Rc;

use adw::prelude::*;
use discord::Sticker;
use gtk::{gio, glib};

use super::App;

/// What the picker's menu was opened on: an emoji or a sticker
/// (`expression-picker`, 233503), or a GIF (`gif-picker`, 400017).
pub(super) enum Target {
    Emoji(discord::emoji::Item),
    Sticker(Sticker),
    Gif(discord::gifs::Gif),
}

/// One line of the list.
pub(super) enum Row<T> {
    Header(String),
    Cells(Vec<T>),
}

impl<T: Clone> Row<T> {
    /// A section of `items` under `title`, `columns` to a row. Nothing for
    /// no items.
    pub(super) fn section(rows: &mut Vec<Row<T>>, columns: usize, title: String, items: Vec<T>) {
        if !items.is_empty() {
            rows.push(Row::Header(title));
            rows.extend(items.chunks(columns).map(|cells| Row::Cells(cells.to_vec())));
        }
    }
}

/// The sections for a query; all of them for none.
pub(super) type Rows<T> = Rc<dyn Fn(&str) -> Vec<Row<T>>>;
/// A cell: a button that hands what it holds to the callback it's given.
pub(super) type Cell<T> = Rc<dyn Fn(&Rc<App>, &T, &Rc<dyn Fn(&T)>) -> gtk::Widget>;
/// What Enter picks for a query: the best match, the first in the list.
pub(super) type Best<T> = Rc<dyn Fn(&str) -> Option<T>>;

/// What a picker lists and how it draws it.
pub(super) struct Spec<T> {
    pub(super) placeholder: &'static str,
    pub(super) columns: usize,
    /// How wide a cell is, which sizes the popover to its columns.
    pub(super) cell_width: i32,
    pub(super) rows: Rows<T>,
    pub(super) cell: Cell<T>,
    pub(super) best: Best<T>,
}

pub(super) type OnText = Rc<dyn Fn(&str)>;
pub(super) type OnScroll = Rc<dyn Fn(f64)>;

/// What a picker whose list is more than a search over a library adds: the
/// GIF picker searches Discord as the text is typed, scrolls for more and
/// has a front page to go back to.
#[derive(Default)]
pub(super) struct Hooks {
    /// The text was typed (not set through [`Handle::set_search`]), ahead of
    /// listing again. Without it the list follows the text after a pause,
    /// as GTK's search entry decides; with it, at every keystroke.
    pub(super) typed: Option<OnText>,
    /// The list scrolled, with the pixels left to its end.
    pub(super) scrolled: Option<OnScroll>,
    /// The back button, and Escape while it shows, go back.
    pub(super) back: Option<Rc<dyn Fn()>>,
    /// A control beside the search box, such as the emoji picker's skin tone.
    pub(super) trailing: Option<gtk::Widget>,
}

/// An open picker, for what is outside it to steer.
#[derive(Clone)]
pub(super) struct Handle {
    search: glib::WeakRef<gtk::SearchEntry>,
    back: glib::WeakRef<gtk::Button>,
    popover: glib::WeakRef<gtk::Popover>,
    /// The text is being set, which isn't typing.
    quiet: Rc<std::cell::Cell<bool>>,
    show: Rc<dyn Fn(&str)>,
}

impl Handle {
    /// List the sections again, for when what they hold has changed.
    pub(super) fn reload(&self) {
        if let Some(search) = self.search.upgrade() {
            (self.show)(search.text().trim());
        }
    }

    /// Put `text` in the search box, which doesn't count as typed.
    pub(super) fn set_search(&self, text: &str) {
        if let Some(search) = self.search.upgrade() {
            self.quiet.set(true);
            search.set_text(text);
            self.quiet.set(false);
            search.set_position(-1);
        }
    }

    /// What the search box asks for while it is empty.
    pub(super) fn set_placeholder(&self, text: &str) {
        if let Some(search) = self.search.upgrade() {
            search.set_placeholder_text(Some(text));
        }
    }

    /// Show or hide the search box and the back button.
    pub(super) fn set_chrome(&self, search: bool, back: bool) {
        if let Some(entry) = self.search.upgrade() {
            entry.set_visible(search);
            if search {
                entry.grab_focus();
            }
        }
        if let Some(button) = self.back.upgrade() {
            button.set_visible(back);
        }
    }

    pub(super) fn close(&self) {
        if let Some(popover) = self.popover.upgrade() {
            popover.popdown();
        }
    }
}

/// Open the picker over `parent`. `picked` hears each cell chosen; Shift
/// keeps the picker open for more, as in the client. Gives a function that
/// lists the sections again, for when what they hold has changed.
pub(super) fn open<T: Clone + 'static>(
    app: &Rc<App>,
    parent: &impl IsA<gtk::Widget>,
    spec: Spec<T>,
    picked: impl Fn(&Rc<App>, &T) + 'static,
) -> Rc<dyn Fn()> {
    let handle = open_with(app, parent, spec, Hooks::default(), picked);
    Rc::new(move || handle.reload())
}

/// [`open`], with `hooks`, and the picker to steer.
pub(super) fn open_with<T: Clone + 'static>(
    app: &Rc<App>,
    parent: &impl IsA<gtk::Widget>,
    spec: Spec<T>,
    hooks: Hooks,
    picked: impl Fn(&Rc<App>, &T) + 'static,
) -> Handle {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    let search = gtk::SearchEntry::builder().placeholder_text(spec.placeholder).build();
    let popover = gtk::Popover::new();
    let picked: Rc<dyn Fn(&T)> = Rc::new({
        let (this, popover) = (Rc::downgrade(app), popover.downgrade());
        move |item| {
            let (Some(this), Some(popover)) = (this.upgrade(), popover.upgrade()) else { return };
            picked(&this, item);
            if !this.shift_held() {
                popover.popdown();
            }
        }
    });

    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        item.set_activatable(false);
        item.set_selectable(false);
        item.set_child(Some(&gtk::Box::new(gtk::Orientation::Horizontal, 0)));
    });
    factory.connect_bind({
        let (this, picked, cell) = (Rc::downgrade(app), picked.clone(), spec.cell.clone());
        move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
            let (Some(this), Some(row), Some(line)) = (
                this.upgrade(),
                item.item().and_downcast::<glib::BoxedAnyObject>(),
                item.child().and_downcast::<gtk::Box>(),
            ) else {
                return;
            };
            match &*row.borrow::<Row<T>>() {
                Row::Header(title) => {
                    let label =
                        gtk::Label::builder().label(title).xalign(0.0).margin_top(6).css_classes(["heading"]).build();
                    line.append(&label);
                }
                Row::Cells(cells) => {
                    for item in cells {
                        line.append(&cell(&this, item, &picked));
                    }
                }
            }
        }
    });
    factory.connect_unbind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().expect("list item");
        if let Some(line) = item.child().and_downcast::<gtk::Box>() {
            while let Some(child) = line.first_child() {
                line.remove(&child);
            }
        }
    });
    let list = gtk::ListView::new(Some(gtk::NoSelection::new(Some(store.clone()))), Some(factory));

    let show: Rc<dyn Fn(&str)> = Rc::new({
        let (store, rows) = (store.clone(), spec.rows.clone());
        move |query| {
            let rows: Vec<glib::BoxedAnyObject> = rows(query).into_iter().map(glib::BoxedAnyObject::new).collect();
            store.splice(0, store.n_items(), &rows);
        }
    });
    show("");
    let quiet = Rc::new(std::cell::Cell::new(false));
    match hooks.typed {
        Some(typed) => search.connect_changed({
            let (show, quiet) = (show.clone(), quiet.clone());
            move |search| {
                let text = search.text();
                if !quiet.get() {
                    typed(text.trim());
                }
                show(text.trim());
            }
        }),
        None => search.connect_search_changed({
            let show = show.clone();
            move |search| show(search.text().trim())
        }),
    };
    search.connect_activate({
        let (best, picked) = (spec.best.clone(), picked.clone());
        move |search| {
            if let Some(item) = best(search.text().trim()) {
                picked(&item);
            }
        }
    });

    let scroller = gtk::ScrolledWindow::builder()
        .child(&list)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_width(spec.columns as i32 * spec.cell_width)
        .min_content_height(360)
        .build();
    let back = gtk::Button::builder().icon_name("go-previous-symbolic").tooltip_text("Back").visible(false).build();
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    header.append(&back);
    search.set_hexpand(true);
    header.append(&search);
    if let Some(trailing) = hooks.trailing {
        header.append(&trailing);
    }
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&header);
    content.append(&scroller);
    if let Some(scrolled) = hooks.scrolled {
        scroller
            .vadjustment()
            .connect_value_changed(move |adjustment| scrolled(adjustment.upper() - adjustment.value()));
    }
    if let Some(go_back) = hooks.back {
        back.connect_clicked({
            let go_back = go_back.clone();
            move |_| go_back()
        });
        // Escape first leaves what is open, and only then the picker.
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let back = back.downgrade();
            move |_, key, _, _| {
                if key == gtk::gdk::Key::Escape && back.upgrade().is_some_and(|b| b.is_visible()) {
                    go_back();
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            }
        });
        popover.add_controller(keys);
    }
    popover.set_child(Some(&content));
    popover.set_parent(parent);
    popover.connect_closed(|popover| {
        // Once the pick has been handled, which may come after closing.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
    search.grab_focus();
    Handle { search: search.downgrade(), back: back.downgrade(), popover: popover.downgrade(), quiet, show }
}
