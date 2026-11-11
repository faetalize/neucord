//! What the emoji and sticker pickers share: a popover with a search box and
//! a scrolling list of sections, each a heading and rows of cells. What the
//! cells are, and what picking one does, is for each picker to say.

use std::rc::Rc;

use adw::prelude::*;
use discord::Sticker;
use gtk::{gio, glib};

use super::App;

/// What the picker's menu (`expression-picker`, 233503) was opened on.
pub(super) enum Target {
    Emoji(discord::emoji::Item),
    Sticker(Sticker),
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

/// Open the picker over `parent`. `picked` hears each cell chosen; Shift
/// keeps the picker open for more, as in the client. Gives a function that
/// lists the sections again, for when what they hold has changed.
pub(super) fn open<T: Clone + 'static>(
    app: &Rc<App>,
    parent: &impl IsA<gtk::Widget>,
    spec: Spec<T>,
    picked: impl Fn(&Rc<App>, &T) + 'static,
) -> Rc<dyn Fn()> {
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
    search.connect_search_changed({
        let show = show.clone();
        move |search| show(search.text().trim())
    });
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
    let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
    content.append(&search);
    content.append(&scroller);
    popover.set_child(Some(&content));
    popover.set_parent(parent);
    popover.connect_closed(|popover| {
        // Once the pick has been handled, which may come after closing.
        let popover = popover.clone();
        glib::idle_add_local_once(move || popover.unparent());
    });
    popover.popup();
    search.grab_focus();
    Rc::new({
        let search = search.downgrade();
        move || {
            if let Some(search) = search.upgrade() {
                show(search.text().trim());
            }
        }
    })
}
