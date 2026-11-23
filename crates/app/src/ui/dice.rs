//! A roll of the dice, from /roll-dice or a link to one: shown above the
//! message box while it rolls and once it lands (`tY` in 989161), then sent
//! as a message (`DiceRollLifecycleManager`, 545185). One roll at a time,
//! in the open channel only; leaving the channel ends it, and a roll still
//! rolling then sends nothing.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use discord::Snowflake;
use discord::dice::{self, Dice};
use gtk::glib;
use gtk::prelude::*;

use super::App;

/// How long the bar takes to go (`dismiss`).
const COLLAPSE_MS: u32 = 300;

pub(super) struct DiceBar {
    revealer: gtk::Revealer,
    label: gtk::Label,
    /// The roll there is, until its bar is gone.
    roll: RefCell<Option<Roll>>,
}

struct Roll {
    channel: Snowflake,
    /// What comes next: the dice landing, the bar going, or it gone.
    timer: Option<glib::SourceId>,
}

impl DiceBar {
    pub(super) fn new() -> Self {
        let label = gtk::Label::builder().xalign(0.0).hexpand(true).build();
        // Styled as the reply bar, which it stacks with.
        let bar = gtk::Box::builder().spacing(8).css_classes(["reply-bar"]).build();
        bar.append(&gtk::Label::new(Some("🎲")));
        bar.append(&label);
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .transition_duration(COLLAPSE_MS)
            .child(&bar)
            .build();
        DiceBar { revealer, label, roll: RefCell::default() }
    }

    fn set_timer(&self, timer: glib::SourceId) {
        if let Some(roll) = self.roll.borrow_mut().as_mut() {
            roll.timer = Some(timer);
        }
    }

    /// Stop what was to come next (`clearTimers`).
    fn clear_timer(&self) {
        if let Some(timer) = self.roll.borrow_mut().as_mut().and_then(|r| r.timer.take()) {
            timer.remove();
        }
    }
}

impl App {
    pub(super) fn build_dice_bar(&self, area: &gtk::Box) {
        area.append(&self.dice.revealer);
    }

    /// Roll `dice` in `channel` (`handleStartRoll`): only the open one, and
    /// not while another roll is there.
    pub(super) fn roll_dice(self: &Rc<Self>, channel: Snowflake, dice: Dice) {
        if self.account.state.borrow().channel != Some(channel) || self.dice.roll.borrow().is_some() {
            return;
        }
        let results: Vec<u32> =
            (0..dice.count).map(|_| glib::random_int_range(1, dice.sides as i32 + 1) as u32).collect();
        self.dice.label.set_text(dice::ROLLING);
        self.dice.revealer.set_reveal_child(true);
        *self.dice.roll.borrow_mut() = Some(Roll { channel, timer: None });
        let (this, epoch) = (Rc::downgrade(self), self.account.epoch.get());
        let timer = glib::timeout_add_local_once(Duration::from_millis(dice::ROLLING_MS.into()), move || {
            let Some(this) = this.upgrade() else { return };
            if let Some(roll) = this.dice.roll.borrow_mut().as_mut() {
                roll.timer = None;
            }
            this.dice_landed(channel, dice, &results, epoch);
        });
        self.dice.set_timer(timer);
    }

    /// `finishRoll`: say what they show, send it, and go a little later.
    fn dice_landed(self: &Rc<Self>, channel: Snowflake, dice: Dice, results: &[u32], epoch: u64) {
        self.dice.label.set_text(&dice::rolled(results));
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(Duration::from_millis(dice::SHOWN_MS.into()), move || {
            if let Some(this) = this.upgrade() {
                if let Some(roll) = this.dice.roll.borrow_mut().as_mut() {
                    roll.timer = None;
                }
                this.dismiss_dice();
            }
        });
        self.dice.set_timer(timer);
        if self.account.epoch.get() != epoch {
            return;
        }
        let (guild, rest) = {
            let state = self.account.state.borrow();
            let Some((guild, _)) = state.find_channel(channel) else { return };
            (guild.map(|g| g.id), state.rest.clone())
        };
        let Some(rest) = rest else { return };
        // Its `:game_die:`s go out as emoji, as typed ones do.
        let text = self.emoji_library(channel).translate(&dice.message(guild, channel, results)).text;
        let this = self.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = rest.send_message(channel, &text, None, &[], None, false).await {
                this.show_composer_error(&format!("Not sent: {e}"));
            }
        });
    }

    /// Hide the bar, and forget the roll once it is gone (`dismiss`).
    fn dismiss_dice(self: &Rc<Self>) {
        self.dice.revealer.set_reveal_child(false);
        let this = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(Duration::from_millis(COLLAPSE_MS.into()), move || {
            if let Some(this) = this.upgrade() {
                this.dice.roll.replace(None);
            }
        });
        self.dice.set_timer(timer);
    }

    /// `handleChannelSelect`: a roll in another channel than `channel`
    /// stops where it is and goes.
    pub(super) fn dice_channel_selected(self: &Rc<Self>, channel: Snowflake) {
        let elsewhere = self.dice.roll.borrow().as_ref().is_some_and(|r| r.channel != channel);
        if elsewhere {
            self.dice.clear_timer();
            self.dismiss_dice();
        }
    }

    /// Roll as a clicked link asks, if it is a link to a roll (`w` in
    /// 362280): in its channel, if that is the open one, then go there.
    pub(super) fn follow_dice_link(self: &Rc<Self>, url: &str) -> bool {
        let Some((channel, dice)) = dice::from_link(url) else { return false };
        self.roll_dice(channel, dice);
        if self.account.state.borrow().channel != Some(channel) {
            self.open_channel(channel);
        }
        true
    }
}
