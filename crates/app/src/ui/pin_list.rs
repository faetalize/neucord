//! A channel's pinned messages, in a popover from the header (the client's
//! pins popout, 945830): newest pinned first, each with Jump and, where we may
//! pin, Unpin; the next page by the button or by scrolling near the end.

use std::rc::Rc;

use adw::prelude::*;
use discord::pins::State;
use discord::search::Context;
use discord::{Message, Snowflake};
use gtk::glib;

use super::App;

/// How close to the end a scroll asks for the next page (`getScrollerState`
/// in 945830), in pixels.
const NEAR_END: f64 = 250.0;

/// The popover's parts, kept to fill it in again as pins load and change.
pub(super) struct PinsView {
    popover: gtk::Popover,
    list: gtk::Box,
    scroller: gtk::ScrolledWindow,
}

impl PinsView {
    pub(super) fn new(app: &Rc<App>) -> PinsView {
        let list = gtk::Box::builder().orientation(gtk::Orientation::Vertical).build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(420)
            .min_content_height(120)
            .max_content_height(480)
            .propagate_natural_height(true)
            .build();
        let popover = gtk::Popover::builder().child(&scroller).build();
        popover.add_css_class("menu");
        scroller.vadjustment().connect_value_changed({
            let app = Rc::downgrade(app);
            move |adjustment| {
                let Some(app) = app.upgrade() else { return };
                let left = adjustment.upper() - adjustment.value() - adjustment.page_size();
                if left < NEAR_END {
                    app.more_pins();
                }
            }
        });
        popover.connect_show({
            let app = Rc::downgrade(app);
            move |_| {
                if let Some(app) = app.upgrade() {
                    app.pins_opened();
                }
            }
        });
        PinsView { popover, list, scroller }
    }

    pub(super) fn popover(&self) -> &gtk::Popover {
        &self.popover
    }
}

impl App {
    /// The popover opened: ask for the first page, or again if what we hold
    /// has links that expired.
    fn pins_opened(self: &Rc<Self>) {
        let Some(channel) = self.account.state.borrow().channel else { return };
        let expired =
            self.account.state.borrow().pins.expired(channel, u64::try_from(super::state::now_ms()).unwrap_or(0));
        self.fetch_pins(channel, None, expired);
        self.show_pins();
    }

    /// The next page, from the end of the list.
    pub(super) fn more_pins(self: &Rc<Self>) {
        let (channel, before) = {
            let state = self.account.state.borrow();
            let Some(channel) = state.channel else { return };
            (channel, state.pins.before(channel))
        };
        if before.is_some() {
            self.fetch_pins(channel, before, false);
        }
    }

    /// Ask for a page when the client's rule says to (`fetchPins`).
    fn fetch_pins(self: &Rc<Self>, channel: Snowflake, before: Option<String>, reset: bool) {
        let rest = {
            let mut state = self.account.state.borrow_mut();
            if !reset && !state.pins.wants(channel, before.as_deref()) {
                return;
            }
            let Some(rest) = state.rest.clone() else { return };
            state.pins.started(channel, reset);
            rest
        };
        self.show_pins();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = rest.pins(channel, before.as_deref()).await;
            {
                let mut state = this.account.state.borrow_mut();
                match result {
                    Ok(page) => state.pins.loaded(channel, page),
                    Err(e) => {
                        eprintln!("pins: {e}");
                        state.pins.failed(channel);
                    }
                }
            }
            this.show_pins();
        });
    }

    /// Fill the popover in from what we hold, if it is open.
    pub(super) fn show_pins(self: &Rc<Self>) {
        let view = self.pins_view.borrow();
        let Some(view) = view.as_ref().filter(|v| v.popover.is_visible()) else { return };
        while let Some(child) = view.list.first_child() {
            view.list.remove(&child);
        }
        let (channel, private, can_unpin, list) = {
            let state = self.account.state.borrow();
            let Some(channel) = state.channel else { return };
            let private = state.find_channel(channel).is_some_and(|(guild, _)| guild.is_none());
            let can = |m: &Message| state.can_pin(m);
            let list = state.pins.get(channel).cloned();
            let can_unpin: Vec<bool> = list.iter().flat_map(|l| &l.items).map(|p| can(&p.message)).collect();
            (channel, private, can_unpin, list)
        };
        let Some(list) = list else { return };
        for (pin, can) in list.items.iter().zip(can_unpin) {
            view.list.append(&self.pin_row(channel, &pin.message, can));
        }
        let note = |text: &str| {
            let label = gtk::Label::builder()
                .label(text)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .margin_top(24)
                .margin_bottom(24)
                .margin_start(12)
                .margin_end(12)
                .css_classes(["dimmed"])
                .build();
            view.list.append(&label);
        };
        match list.state {
            State::Loading => {
                let spinner = adw::Spinner::builder().margin_top(24).margin_bottom(24).build();
                view.list.append(&spinner);
            }
            State::Failed => note("Couldn't load the pinned messages."),
            State::HasMore => {
                let more = gtk::Button::builder()
                    .label("Load more messages")
                    .margin_top(6)
                    .margin_bottom(6)
                    .margin_start(12)
                    .margin_end(12)
                    .build();
                let this = Rc::downgrade(self);
                more.connect_clicked(move |_| {
                    if let Some(this) = this.upgrade() {
                        this.more_pins();
                    }
                });
                view.list.append(&more);
            }
            State::Finished => {}
        }
        if list.items.is_empty() && list.state != State::Loading && list.state != State::Failed {
            note(if private {
                "This direct message doesn't have\nany pinned messages... yet.\n\nYou can pin a message from its context menu."
            } else {
                "This channel doesn't have any\npinned messages... yet.\n\nUsers with the 'Pin Messages' permission can pin a message from its context menu."
            });
        }
        let _ = &view.scroller;
    }

    /// One pin: the message, then Jump and Unpin.
    fn pin_row(self: &Rc<Self>, channel: Snowflake, message: &Message, can_unpin: bool) -> gtk::ListBoxRow {
        let actions = gtk::Box::builder().spacing(6).halign(gtk::Align::End).margin_top(4).build();
        let jump = gtk::Button::builder().label("Jump").css_classes(["flat"]).build();
        jump.connect_clicked({
            let (this, id) = (Rc::downgrade(self), message.id);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if let Some(view) = this.pins_view.borrow().as_ref() {
                    view.popover.popdown();
                }
                this.open_channel_at(channel, Some(id));
            }
        });
        actions.append(&jump);
        if can_unpin {
            let unpin = gtk::Button::builder()
                .icon_name("window-close-symbolic")
                .tooltip_text("Unpin Message")
                .css_classes(["flat"])
                .build();
            unpin.connect_clicked({
                let (this, message) = (Rc::downgrade(self), message.clone());
                move |_| {
                    let Some(this) = this.upgrade() else { return };
                    match this.shift_held() {
                        true => this.unpin_message(&message),
                        false => this.confirm_unpin(&message),
                    }
                }
            });
            actions.append(&unpin);
        }
        self.message_summary_row(message, Context::Channel(channel), Some(actions.upcast_ref()))
    }
}

/// Whether the pins button shows for the open channel: one with messages.
pub(super) fn offered(kind: discord::ChannelKind) -> bool {
    !kind.is_forum_like() && !kind.is_vocal()
}
