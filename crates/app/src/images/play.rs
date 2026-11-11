//! What the window and the pointer say about moving media, and the controller
//! that applies the rules of [`discord::animation`] to one widget.
//!
//! [`Motion`] holds what every picture shares: whether the window has focus,
//! the account's media settings and the system's wish for less motion.
//! [`attach`] watches one widget (is it mapped and in view, is the pointer on
//! it or on its row) and reports when its media should show moving and when
//! its frames should advance, and nothing else wakes up in between.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use discord::animation::{self, Budget, EMOJI_LIMIT, Facts, Subject};
use discord::settings::MediaSettings;
use gtk::glib;
use gtk::prelude::*;

type Listener = Weak<dyn Fn()>;

/// Shared by every animated picture of the window. Clone-cheap.
#[derive(Clone)]
pub struct Motion(Rc<Shared>);

struct Shared {
    focused: Cell<bool>,
    media: Cell<MediaSettings>,
    reduced: Cell<bool>,
    /// The group of messages the pointer is on in the message list.
    group: Cell<Option<u64>>,
    budget: RefCell<Budget>,
    /// Who is listening, by id, so a freed place wakes only who waits.
    listeners: RefCell<Vec<(u64, Listener)>>,
    next_id: Cell<u64>,
}

impl Motion {
    pub fn new() -> Self {
        Motion(Rc::new(Shared {
            focused: Cell::new(true),
            media: Cell::new(MediaSettings::default()),
            reduced: Cell::new(false),
            group: Cell::new(None),
            budget: RefCell::new(Budget::new(EMOJI_LIMIT)),
            listeners: RefCell::default(),
            next_id: Cell::new(0),
        }))
    }

    /// Whether the window has focus: an unfocused or hidden window moves nothing.
    pub fn set_focused(&self, focused: bool) {
        if self.0.focused.replace(focused) != focused {
            self.notify(None);
        }
    }

    /// The account's settings, as they stand.
    pub fn set_media(&self, media: MediaSettings) {
        if self.0.media.replace(media) != media {
            self.notify(None);
        }
    }

    /// Whether the system asks for less motion (GTK's `gtk-enable-animations`).
    pub fn set_reduced(&self, reduced: bool) {
        if self.0.reduced.replace(reduced) != reduced {
            self.notify(None);
        }
    }

    /// The pointer is on a message of the group that starts with message
    /// `first` (or on none).
    pub fn set_hovered_group(&self, first: Option<u64>) {
        if self.0.group.replace(first) != first {
            self.notify(None);
        }
    }

    fn focused(&self) -> bool {
        self.0.focused.get()
    }

    fn media(&self) -> MediaSettings {
        let media = self.0.media.get();
        if self.0.reduced.get() { media.reduced_motion() } else { media }
    }

    fn new_id(&self) -> u64 {
        self.0.next_id.set(self.0.next_id.get() + 1);
        self.0.next_id.get()
    }

    /// Tell `only` (everyone if `None`) to look again. Listeners whose widget
    /// is gone are dropped from the list on the way.
    fn notify(&self, only: Option<&[u64]>) {
        let mut listeners = self.0.listeners.borrow_mut();
        listeners.retain(|(_, listener)| listener.strong_count() > 0);
        let due: Vec<_> = listeners
            .iter()
            .filter(|(id, _)| only.is_none_or(|only| only.contains(id)))
            .filter_map(|(_, listener)| listener.upgrade())
            .collect();
        drop(listeners);
        for listener in due {
            listener();
        }
    }

    fn release_places(&self, who: u64) {
        let granted = self.0.budget.borrow_mut().release(who);
        if !granted.is_empty() {
            self.notify(Some(&granted));
        }
    }
}

/// Whose pointer counts for a watched widget besides its own.
#[derive(Clone, Debug)]
pub enum Row {
    /// Only the pointer on the widget.
    None,
    /// Also the pointer on the message or list row it is in.
    Parent,
    /// Also the pointer on any message of the group this says (by the id of
    /// its first message, which changes as the row shows others), as the
    /// client moves an avatar for any message of its group.
    Group(Rc<Cell<u64>>),
}

/// What a watched widget's media should do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Update {
    /// It shows its moving self rather than its still.
    pub moving: bool,
    /// Its frames advance.
    pub plays: bool,
}

struct Watch {
    motion: Motion,
    id: u64,
    widget: glib::WeakRef<gtk::Widget>,
    subject: Subject,
    row: Row,
    /// How many animated emoji it holds, if it takes places from the budget.
    emoji: u32,
    hovered: Cell<bool>,
    row_hovered: Cell<bool>,
    on_screen: Cell<bool>,
    /// It has been in view: until then its animation isn't even loaded.
    seen: Cell<bool>,
    /// The row whose pointer it follows, and the controller on it.
    parent: RefCell<Option<(glib::WeakRef<gtk::Widget>, gtk::EventControllerMotion)>>,
    scrolled: RefCell<Vec<(gtk::Adjustment, glib::SignalHandlerId)>>,
    shown: Cell<Option<Update>>,
    on_update: Box<dyn Fn(Update)>,
    /// What the motion calls to have it look again.
    listener: RefCell<Option<Rc<dyn Fn()>>>,
}

/// Watch `widget`, an animated `subject` (with `emoji` places to take from
/// the budget, if it is text with that many animated emoji), and call
/// `on_update` first with where it stands and then whenever that changes.
/// With a `row`, the pointer there counts as the pointer on it, where the
/// rules let a row move it. Lives as long as the widget.
pub fn attach(
    motion: &Motion,
    widget: &impl IsA<gtk::Widget>,
    subject: Subject,
    row: Row,
    emoji: u32,
    on_update: impl Fn(Update) + 'static,
) {
    let widget = widget.upcast_ref::<gtk::Widget>();
    let watch = Rc::new(Watch {
        motion: motion.clone(),
        id: motion.new_id(),
        widget: widget.downgrade(),
        subject,
        row,
        emoji,
        hovered: Cell::new(false),
        row_hovered: Cell::new(false),
        on_screen: Cell::new(false),
        seen: Cell::new(false),
        parent: RefCell::default(),
        scrolled: RefCell::default(),
        shown: Cell::new(None),
        on_update: Box::new(on_update),
        listener: RefCell::default(),
    });
    let weak = Rc::downgrade(&watch);
    let look: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(watch) = weak.upgrade() {
            watch.update();
        }
    });
    motion.0.listeners.borrow_mut().push((watch.id, Rc::downgrade(&look)));
    *watch.listener.borrow_mut() = Some(look);

    let pointer = gtk::EventControllerMotion::new();
    pointer.connect_enter({
        let watch = watch.clone();
        move |_, _, _| {
            watch.hovered.set(true);
            watch.update();
        }
    });
    pointer.connect_leave({
        let watch = watch.clone();
        move |_| {
            watch.hovered.set(false);
            watch.update();
        }
    });
    widget.add_controller(pointer);
    widget.connect_map({
        let watch = watch.clone();
        move |_| watch.mapped()
    });
    widget.connect_unmap({
        let watch = watch.clone();
        move |_| watch.unmapped()
    });
    if widget.is_mapped() {
        watch.mapped();
    } else {
        watch.update();
    }
}

impl Watch {
    fn update(&self) {
        let facts = Facts {
            focused: self.motion.focused(),
            on_screen: self.on_screen.get(),
            hovered: self.hovered.get(),
            row_hovered: match &self.row {
                Row::None => false,
                Row::Parent => self.row_hovered.get(),
                Row::Group(first) => self.motion.0.group.get() == Some(first.get()),
            },
        };
        let media = self.motion.media();
        let moving = self.seen.get() && animation::moving(self.subject, &media, facts);
        let mut plays = animation::plays(self.subject, &media, facts);
        if self.emoji > 0 {
            if plays {
                plays = self.motion.0.budget.borrow_mut().ask(self.id, self.emoji);
            } else {
                self.motion.release_places(self.id);
            }
        }
        let now = Update { moving, plays };
        if self.shown.replace(Some(now)) != Some(now) {
            (self.on_update)(now);
        }
    }

    fn mapped(self: &Rc<Self>) {
        let Some(widget) = self.widget.upgrade() else { return };
        if let Some(row) = matches!(self.row, Row::Parent).then(|| ancestor(&widget, is_row)).flatten() {
            let pointer = gtk::EventControllerMotion::new();
            pointer.connect_enter({
                let watch = Rc::downgrade(self);
                move |_, _, _| {
                    if let Some(watch) = watch.upgrade() {
                        watch.row_hovered.set(true);
                        watch.update();
                    }
                }
            });
            pointer.connect_leave({
                let watch = Rc::downgrade(self);
                move |_| {
                    if let Some(watch) = watch.upgrade() {
                        watch.row_hovered.set(false);
                        watch.update();
                    }
                }
            });
            row.add_controller(pointer.clone());
            self.row_hovered.set(pointer.contains_pointer());
            *self.parent.borrow_mut() = Some((row.downgrade(), pointer));
        }
        if let Some(scroller) = scroller(&widget).and_downcast::<gtk::ScrolledWindow>() {
            let mut scrolled = self.scrolled.borrow_mut();
            for adjustment in [scroller.hadjustment(), scroller.vadjustment()] {
                let watch = Rc::downgrade(self);
                let handler = adjustment.connect_value_changed(move |_| {
                    if let Some(watch) = watch.upgrade() {
                        watch.look();
                    }
                });
                scrolled.push((adjustment, handler));
            }
        }
        self.look();
    }

    fn unmapped(&self) {
        if let Some((row, pointer)) = self.parent.take()
            && let Some(row) = row.upgrade()
        {
            row.remove_controller(&pointer);
        }
        for (adjustment, handler) in self.scrolled.take() {
            adjustment.disconnect(handler);
        }
        // Leaving a mapped widget's pointer events can't be counted on once it
        // is unmapped.
        self.hovered.set(false);
        self.row_hovered.set(false);
        self.on_screen.set(false);
        self.update();
    }

    /// Whether any of it shows, then what follows from that.
    fn look(&self) {
        let seen = self.widget.upgrade().is_some_and(|widget| in_view(&widget));
        self.on_screen.set(seen);
        self.seen.set(self.seen.get() || seen);
        self.update();
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if self.emoji > 0 {
            self.motion.release_places(self.id);
        }
    }
}

/// The first ancestor of `widget` that `is` one, not looking past the window
/// or popover it is in.
fn ancestor(widget: &gtk::Widget, is: impl Fn(&gtk::Widget) -> bool) -> Option<gtk::Widget> {
    let mut up = widget.parent();
    while let Some(parent) = up {
        if is(&parent) {
            return Some(parent);
        }
        if parent.is::<gtk::Native>() {
            return None;
        }
        up = parent.parent();
    }
    None
}

/// A row of the message list, of another `ListView` or of a `ListBox`.
fn is_row(widget: &gtk::Widget) -> bool {
    widget.is::<gtk::ListBoxRow>() || matches!(widget.type_().name(), "NeucordMessageRow" | "GtkListItemWidget")
}

fn scroller(widget: &gtk::Widget) -> Option<gtk::Widget> {
    ancestor(widget, |w| w.is::<gtk::ScrolledWindow>())
}

/// Whether any of `widget` is in the part of its scroller that shows. Without
/// a scroller, or before it has a size, it counts as shown: the next scroll
/// says otherwise.
fn in_view(widget: &gtk::Widget) -> bool {
    if !widget.is_mapped() {
        return false;
    }
    let Some(scroller) = scroller(widget) else { return true };
    if widget.width() == 0 || widget.height() == 0 {
        return true;
    }
    let Some(bounds) = widget.compute_bounds(&scroller) else { return false };
    bounds.x() < scroller.width() as f32
        && bounds.x() + bounds.width() > 0.0
        && bounds.y() < scroller.height() as f32
        && bounds.y() + bounds.height() > 0.0
}
