//! Running the client's built-in commands (12115, 743361): never an
//! interaction, each its own way, from what `discord::builtins` makes of
//! the options. Results and refusals that aren't a message show as Clyde's
//! local lines, which only this window sees (`sendBotMessage`).

use std::rc::Rc;

use adw::prelude::*;
use discord::builtins::{self, Notice, Outcome};
use discord::commands::Command;
use discord::permissions::{self, Permissions};
use discord::thread_creation::{self, Refused};
use discord::{Message, Snowflake, scheduled};
use gtk::glib;
use serde_json::Value;

use super::App;
use super::create_thread::refusal_alert;
use super::friends::confirm;
use super::scheduling::default_start;
use super::slash::Keep;
use super::state::{now_ms, now_unix};
use crate::timelines;

impl App {
    /// Run `command`, a built-in, in `channel` with the options `run_options`
    /// gave (`execute` through `A` in 545152). Whether the command is done
    /// with; /thread stays in the box until its thread is there, and one
    /// that can't run shakes the box (`SHAKE_APP`).
    pub(super) fn run_built_in(self: &Rc<Self>, channel: Snowflake, command: &Command, options: &[Value]) -> bool {
        let Some(builtin) = command.builtin else { return false };
        let Some(outcome) = builtins::outcome(builtin, options) else {
            self.composer.error_bell();
            return false;
        };
        if matches!(outcome, Outcome::Thread { .. }) && self.slash.thread_busy.get() {
            return false;
        }
        let (guild, rest) = {
            let state = self.account.state.borrow();
            (state.find_channel(channel).and_then(|(guild, _)| guild.map(|g| g.id)), state.rest.clone())
        };
        let Some(rest) = rest else { return false };
        self.command_used(command, guild);
        match outcome {
            Outcome::Message { content, tts } => {
                if !self.send_text(channel, content.clone(), tts) {
                    // What it made goes back in the box, to be sent as typed.
                    self.end_command(Keep::Nothing);
                    self.set_composer_text(&content);
                }
            }
            Outcome::Nick(nick) => {
                let Some(guild) = guild else { return true };
                self.after_reopen(channel, async move |this| {
                    let answer = rest.change_nickname(guild, &nick).await;
                    this.clyde(channel, Notice::of_nick(&answer));
                });
            }
            Outcome::Kick { user, reason } => self.moderate_member(channel, user, Moderation::Kick(reason)),
            Outcome::Ban { user, delete_message_seconds, reason } => {
                self.moderate_member(channel, user, Moderation::Ban(delete_message_seconds, reason))
            }
            Outcome::Timeout { user, seconds, reason } => {
                self.moderate_member(channel, user, Moderation::Timeout(seconds, reason))
            }
            Outcome::Msg { user, message } => {
                let this = self.clone();
                glib::spawn_future_local(async move {
                    match this.open_dm_with(user).await {
                        Ok(dm) => _ = this.send_text(dm, message, false),
                        Err(_) => this.clyde(channel, Notice::MsgFailed),
                    }
                });
            }
            Outcome::Leave { silent } => self.confirm_leave(channel, silent),
            Outcome::Schedule(message) => {
                let this = Rc::downgrade(self);
                let picker = super::scheduling::Picker::NEW;
                self.pick_time(&self.window, picker, default_start(), None, None, move |at| {
                    if let Some(this) = this.upgrade() {
                        this.set_schedule(channel, at);
                        this.send_text(channel, message.clone(), false);
                    }
                });
            }
            Outcome::Thread { name, message } => {
                self.start_thread(channel, rest, name, message);
                return false;
            }
        }
        true
    }

    /// Run `then` once `channel` is reopened, if it is a thread that has to
    /// be first, as `A` in 545152 does before `execute`.
    fn after_reopen(self: &Rc<Self>, channel: Snowflake, then: impl AsyncFnOnce(Rc<App>) + 'static) {
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let (this, epoch) = (self.clone(), self.account.epoch.get());
        glib::spawn_future_local(async move {
            let reopened = this.reopen(&rest, channel).await;
            if this.account.epoch.get() != epoch {
                return;
            }
            match reopened {
                Ok(()) => then(this).await,
                Err(why) => this.show_composer_error(&format!("Not sent: {why}")),
            }
        });
    }

    /// /kick, /ban and /timeout: the client's checks, then the request
    /// (778858), with Clyde saying how it went. Nothing is sent where the
    /// checks say no, or for a /kick of someone nobody here knows.
    fn moderate_member(self: &Rc<Self>, channel: Snowflake, user: Snowflake, action: Moderation) {
        let (allowed, known, guild) = {
            let state = self.account.state.borrow();
            let Some((Some(guild), _)) = state.find_channel(channel) else { return };
            let Some(me) = state.account.as_ref() else { return };
            let roles = state.members.roles(guild.id, user);
            let known = state.whole_users.get(&user.0).cloned();
            let now = now_unix();
            let allowed = match &action {
                Moderation::Kick(_) => permissions::can_manage(guild, me, user, roles, Permissions::KICK_MEMBERS, now),
                Moderation::Ban(..) => permissions::can_manage(guild, me, user, roles, Permissions::BAN_MEMBERS, now),
                Moderation::Timeout(..) => permissions::can_time_out(guild, me, known.as_ref(), roles, now),
            };
            (allowed, known, guild.id)
        };
        if !allowed {
            return self.clyde(channel, action.forbidden());
        }
        if matches!(action, Moderation::Kick(_)) && known.is_none() {
            return self.clyde(channel, Notice::KickFailed);
        }
        let Some(rest) = self.account.state.borrow().rest.clone() else { return };
        let name = builtins::target_name(known.as_ref(), user);
        self.after_reopen(channel, async move |this| {
            let notice = match action {
                Moderation::Kick(reason) => match rest.kick(guild, user, &reason).await {
                    Ok(()) => Notice::Kicked(name),
                    Err(_) => Notice::KickFailed,
                },
                Moderation::Ban(seconds, reason) => match rest.ban(guild, user, seconds, &reason).await {
                    Ok(()) => Notice::Banned(name),
                    Err(_) => Notice::BanFailed,
                },
                Moderation::Timeout(seconds, reason) => {
                    let until = builtins::timeout_until(now_ms(), seconds);
                    match rest.time_out(guild, user, &until, &reason).await {
                        Ok(()) => Notice::TimedOut { user: name, seconds },
                        Err(_) => Notice::TimeoutFailed,
                    }
                }
            };
            this.clyde(channel, notice);
        });
    }

    /// /leave: ask as the client does (743361), then leave the group, which
    /// goes from here before Discord answers (`closePrivateChannel`, 308528).
    fn confirm_leave(self: &Rc<Self>, channel: Snowflake, silent: bool) {
        let (name, managed) = {
            let state = self.account.state.borrow();
            let Some((None, group)) = state.find_channel(channel) else { return };
            (group.title(), group.application_id.is_some())
        };
        let (heading, body) = builtins::leave_dialog(&name, managed);
        let dialog = confirm(&heading, &body, "_Leave Group", adw::ResponseAppearance::Destructive);
        dialog.set_body_use_markup(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != "confirm" {
                return;
            }
            let Some(rest) = this.account.state.borrow().rest.clone() else { return };
            this.on_private_channel_delete(channel);
            if let Err(e) = rest.leave_group(channel, silent).await {
                eprintln!("leave: {e}");
            }
        });
    }

    /// /thread: a public thread in `channel` (`Nw` in 971389), then its
    /// first message, without going there. The command stays in the box
    /// until the thread is here; a refusal leaves it, with an alert where
    /// the client shows one.
    fn start_thread(self: &Rc<Self>, channel: Snowflake, rest: discord::Rest, name: String, message: String) {
        let Some(body) =
            self.account.state.borrow().find_channel(channel).map(|(_, c)| thread_creation::slash_body(c, &name))
        else {
            return;
        };
        self.slash.thread_busy.set(true);
        self.after_reopen(channel, async move |this| {
            let made = rest.create_thread(channel, None, body).await;
            this.slash.thread_busy.set(false);
            let thread = match made {
                Ok(thread) => thread,
                Err(e) => {
                    let refused = Refused::from(&e);
                    if refused.alerts_after_slash_command()
                        && let Some((heading, body)) = refusal_alert(&refused, &e)
                    {
                        let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
                        dialog.add_response("okay", "_Okay");
                        dialog.present(Some(&this.window));
                    }
                    return;
                }
            };
            this.thread_arrived(thread.id).await;
            if this.slash.active.borrow().as_ref().is_some_and(|a| a.channel == channel) {
                this.end_command(Keep::Nothing);
            }
            let text = this.emoji_library(thread.id).translate(&message).text;
            if !text.trim().is_empty()
                && let Err(e) = rest.send_message(thread.id, &text, None, &[], None, false).await
            {
                this.show_composer_error(&format!("Not sent: {e}"));
            }
        });
    }

    /// Clyde's line in `channel`, as the client's `sendBotMessage` adds it.
    fn clyde(self: &Rc<Self>, channel: Snowflake, notice: Notice) {
        self.local_message(notice.message(channel, Snowflake::now(), &scheduled::iso(now_unix())));
    }

    /// Add `message` to its channel's timeline only: no read state, no
    /// notification, nothing sent. Dismiss takes it back out.
    fn local_message(&self, message: Message) {
        let channel = message.channel_id;
        let timelines = self.account.timelines.borrow();
        let Some(store) = timelines.get(channel).filter(|t| !t.detached).map(|t| t.store.clone()) else { return };
        drop(timelines);
        let at_bottom = self.account.state.borrow().channel == Some(channel) && self.at_bottom();
        store.append(&timelines::boxed(message));
        if at_bottom {
            self.scroll_to_end();
        }
    }
}

/// What /kick, /ban and /timeout do, with their reasons.
enum Moderation {
    Kick(String),
    /// The messages to delete, in seconds, and the reason.
    Ban(u64, String),
    /// How long, in seconds, and the reason.
    Timeout(u64, String),
}

impl Moderation {
    /// Clyde's line where the checks say no.
    fn forbidden(&self) -> Notice {
        match self {
            Moderation::Kick(_) => Notice::KickForbidden,
            Moderation::Ban(..) => Notice::BanForbidden,
            Moderation::Timeout(..) => Notice::TimeoutForbidden,
        }
    }
}
