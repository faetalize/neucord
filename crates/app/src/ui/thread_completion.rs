//! The `@` and `:` lists in the new-thread form's message. The official
//! client's form is a message box of its own, with the box's autocomplete;
//! what the lists offer is the message box's ([`App::chat_mentions`],
//! [`App::name_candidates`]), and a sticker picked from one waits for the
//! first message as the form's picker's does.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::command_line::Header;
use gtk::glib;

use super::App;
use super::composer::{Completion, emoji_token, mention_token};
use super::create_thread::Form;
use super::pills::header_row;

/// The list's popup, under the form's message, and what it lists.
pub(super) struct Popup {
    popover: gtk::Popover,
    list: gtk::ListBox,
    candidates: RefCell<Vec<Completion>>,
    headers: RefCell<Vec<(usize, Header)>>,
    /// Char offset of the `@` or `:` being completed.
    start: Cell<i32>,
}

impl Popup {
    pub(super) fn new() -> Self {
        let list = gtk::ListBox::builder().css_classes(["navigation-sidebar"]).build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(240)
            .build();
        let popover = gtk::Popover::builder().autohide(false).has_arrow(false).position(gtk::PositionType::Top).build();
        popover.set_child(Some(&scroller));
        Popup { popover, list, candidates: RefCell::default(), headers: RefCell::default(), start: Cell::new(0) }
    }

    /// A manually parented popover must be unparented before its parent goes.
    pub(super) fn unparent(&self) {
        self.popover.unparent();
    }

    fn move_selection(&self, delta: i32) {
        let count = self.candidates.borrow().len() as i32;
        if count == 0 {
            return;
        }
        // With none selected, Down takes the first and Up the last.
        let next = match self.list.selected_row() {
            Some(row) => (row.index() + delta).rem_euclid(count),
            None if delta > 0 => 0,
            None => count - 1,
        };
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
        }
    }
}

impl App {
    /// Wire the form's message to its lists: they follow the caret, and
    /// while one is open it owns the navigation keys, ahead of Enter's.
    pub(super) fn connect_thread_completion(self: &Rc<Self>, form: &Rc<Form>) {
        let popup = &form.completion;
        popup.popover.set_parent(&form.message);
        popup.list.set_header_func({
            let form = Rc::downgrade(form);
            move |row, _| {
                let Some(form) = form.upgrade() else { return };
                let index = row.index().max(0) as usize;
                let header =
                    form.completion.headers.borrow().iter().find(|(at, _)| *at == index).map(|(_, h)| h.clone());
                row.set_header(header.map(|h| header_row(&h, None)).as_ref());
            }
        });
        popup.list.connect_row_activated({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_, row| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.accept_thread_completion(&form, row.index());
                }
            }
        });
        popup.popover.connect_closed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.suggestions_closed();
                }
            }
        });
        form.message.buffer().connect_changed({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_| {
                if let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) {
                    this.update_thread_completion(&form);
                }
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed({
            let (this, form) = (Rc::downgrade(self), Rc::downgrade(form));
            move |_, key, _, _| {
                use gtk::gdk::Key;
                let (Some(this), Some(form)) = (this.upgrade(), form.upgrade()) else {
                    return glib::Propagation::Proceed;
                };
                let popup = &form.completion;
                if !popup.popover.is_visible() {
                    return glib::Propagation::Proceed;
                }
                match key {
                    Key::Escape => popup.popover.popdown(),
                    Key::Down | Key::Tab => popup.move_selection(1),
                    Key::Up => popup.move_selection(-1),
                    // Enter takes the row selected, or the first.
                    Key::Return | Key::KP_Enter => {
                        let selected = popup.list.selected_row().map_or(0, |row| row.index());
                        this.accept_thread_completion(&form, selected);
                    }
                    _ => return glib::Propagation::Proceed,
                }
                glib::Propagation::Stop
            }
        });
        form.message.add_controller(keys);
    }

    /// Show or hide the list for the `@word` or `:word` at the caret.
    fn update_thread_completion(self: &Rc<Self>, form: &Rc<Form>) {
        let popup = &form.completion;
        let buffer = form.message.buffer();
        let (start, candidates, headers) = if let Some((start, prefix)) = mention_token(&buffer) {
            let (candidates, header) = self.chat_mentions(&prefix);
            (start, candidates, header.map(|h| vec![(0, h)]).unwrap_or_default())
        } else if let Some((start, query)) = emoji_token(&buffer) {
            let (candidates, headers) = self.name_candidates(&query);
            (start, candidates, headers)
        } else {
            return popup.popover.popdown();
        };
        // The people listed are checked to still be members, as the message box's are.
        let people: Vec<_> = candidates
            .iter()
            .filter_map(|c| match c {
                Completion::Mention(id, _) => Some(discord::Snowflake(*id)),
                _ => None,
            })
            .collect();
        match people.is_empty() {
            true => self.suggestions_closed(),
            false => self.suggestions_listed(people),
        }
        if candidates.is_empty() {
            return popup.popover.popdown();
        }
        while let Some(child) = popup.list.first_child() {
            popup.list.remove(&child);
        }
        popup.headers.replace(headers);
        for candidate in &candidates {
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&candidate.row(&self.images)));
            popup.list.append(&row);
        }
        popup.candidates.replace(candidates);
        popup.start.set(start);
        popup.list.invalidate_headers();
        if let Some(row) = popup.list.row_at_index(0) {
            popup.list.select_row(Some(&row));
        }
        let buffer = form.message.buffer();
        let iter = buffer.iter_at_offset(buffer.property::<i32>("cursor-position"));
        let strong = form.message.cursor_locations(Some(&iter)).0;
        let (x, y) = form.message.buffer_to_window_coords(gtk::TextWindowType::Widget, strong.x(), strong.y());
        popup.popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x, y, 1, strong.height())));
        popup.popover.popup();
    }

    /// Put row `index` in place of the word being completed. A sticker takes
    /// the word out of the text and waits for the message.
    fn accept_thread_completion(self: &Rc<Self>, form: &Rc<Form>, index: i32) {
        let popup = &form.completion;
        let (text, sticker) = match popup.candidates.borrow().get(index.max(0) as usize) {
            Some(Completion::Sticker(entry)) => (String::new(), Some((**entry).clone())),
            Some(candidate) => (candidate.text(), None),
            None => return,
        };
        let buffer = form.message.buffer();
        let cursor = buffer.property::<i32>("cursor-position");
        let mut from = buffer.iter_at_offset(popup.start.get());
        let mut to = buffer.iter_at_offset(cursor);
        buffer.delete(&mut from, &mut to);
        let mut at = buffer.iter_at_offset(popup.start.get());
        buffer.insert(&mut at, &text);
        popup.popover.popdown();
        form.message.grab_focus();
        if let Some(entry) = sticker {
            self.pick_form_sticker(form, &entry);
        }
    }
}
