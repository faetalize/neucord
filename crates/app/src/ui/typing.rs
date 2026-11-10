//! Who is typing in the open channel, under the message box, and telling
//! Discord when we are. The rules are `discord::typing`'s.

use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::Snowflake;
use discord::typing::{Input, Piece, Scheduled, sentence, shown};
use gtk::glib;

use super::App;
use super::state::now_ms;

impl App {
    /// Put `text` in the message box without it counting as typing.
    pub(super) fn set_composer_text(&self, text: &str) {
        self.composer_programmatic.set(true);
        self.composer.buffer().set_text(text);
        self.composer_programmatic.set(false);
    }

    /// The text in the message box changed because the user changed it.
    pub(super) fn composer_typed(self: &Rc<Self>) {
        // Editing happens in the same box, and is no message being written.
        if self.composer_programmatic.get() || self.editing().is_some() || !self.composer_box.is_visible() {
            return;
        }
        let buffer = self.composer.buffer();
        let input = Input::of(&buffer.text(&buffer.start_iter(), &buffer.end_iter(), false));
        let scheduled = {
            let mut state = self.state.borrow_mut();
            let (Some(channel), Some(me)) = (state.channel, state.me()) else { return };
            match input {
                Input::Typed => state.typing.typed(channel, me, now_ms()),
                Input::Cleared => {
                    state.typing.cleared(channel, me);
                    None
                }
                Input::Ignored => None,
            }
        };
        if let Some(Scheduled { token, delay_ms }) = scheduled {
            let this = Rc::downgrade(self);
            glib::timeout_add_local_once(Duration::from_millis(delay_ms as u64), move || {
                if let Some(this) = this.upgrade() {
                    this.send_typing(token);
                }
            });
        }
    }

    /// The wait for a typing notice ended: send it, and take the slowmode
    /// wait Discord may answer with.
    fn send_typing(self: &Rc<Self>, token: u64) {
        let (channel, rest) = {
            let mut state = self.state.borrow_mut();
            (state.typing.send_due(token, now_ms()), state.rest.clone())
        };
        let (Some(channel), Some(rest)) = (channel, rest) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rest.typing(channel).await {
                Ok(wait) if wait > 0 => this.record_slowmode(channel, |slowmode, c, bypass| {
                    slowmode.told(c, wait, bypass, now_ms());
                }),
                Ok(_) => {}
                Err(e) => eprintln!("typing {channel}: {e}"),
            }
        });
    }

    /// Someone started typing in `channel`.
    pub(super) fn on_typing(self: &Rc<Self>, channel: Snowflake, user: Snowflake, named: Option<&discord::User>) {
        self.learn_users(named.into_iter());
        self.state.borrow_mut().typing.start(channel, user, now_ms());
        self.refresh_typing();
    }

    /// Draw who is typing in the open channel, and again when the first
    /// of them lapses.
    pub(super) fn refresh_typing(self: &Rc<Self>) {
        if let Some(tick) = self.typing_tick.take() {
            tick.remove();
        }
        let now = now_ms();
        let (names, next) = {
            let mut state = self.state.borrow_mut();
            state.typing.prune(now);
            let Some((channel, me)) = state.channel.zip(state.me()) else { return self.show_typing(&[]) };
            let typers = state.typing.typers(channel, now);
            let names =
                shown(&typers, me, |id| state.relationships.is_blocked(id), |id| state.users.get(&id.0).cloned());
            (names, state.typing.next_lapse(channel, now))
        };
        self.show_typing(&names);
        if let Some(lapses) = next {
            let this = Rc::downgrade(self);
            let tick =
                glib::timeout_add_local_once(Duration::from_millis((lapses - now).max(0) as u64 + 1), move || {
                    if let Some(this) = this.upgrade() {
                        this.typing_tick.take();
                        this.refresh_typing();
                    }
                });
            self.typing_tick.replace(Some(tick));
        }
    }

    fn show_typing(&self, names: &[String]) {
        let markup = |narrow| -> String {
            sentence(names, narrow)
                .iter()
                .map(|piece| match piece {
                    Piece::Name(name) => format!("<b>{}</b>", glib::markup_escape_text(name)),
                    Piece::Words(words) => (*words).to_owned(),
                })
                .collect()
        };
        let line = &self.typing_line;
        line.set_visible(!names.is_empty());
        line.set_markup(&markup(false));
        // The names that don't fit give way to "Multiple people".
        let room = self.composer_box.width();
        if room > 0 && line.measure(gtk::Orientation::Horizontal, -1).1 > room {
            line.set_markup(&markup(true));
        }
    }
}
