//! Reactions in the window: the pills under a message and the button that
//! adds one, adding and taking back ours as the client does (shown at once,
//! put back if Discord refuses), and what the gateway changes.
//!
//! What the client rules and sends is `discord::reactions`'s and
//! `discord::permissions::reacting`'s; this is the drawing and the glue.

use std::future::Future;
use std::rc::Rc;

use adw::prelude::*;
use discord::emoji::Library;
use discord::model::Emoji;
use discord::reactions::{Kind, Location, Notice, Pill, Reacted, Refusal, Verdict, judge};
use discord::{ChannelKind, Message, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::menus::{Choice, Choices, Entry, Item, Kind as MenuKind, Style, Table};
use super::polls;
use crate::timelines;

/// The emoji the account reacts with most, after its favourites: up to four
/// at the top of the menu (`tr` in 761304), below nothing when text is
/// selected. The client draws them as a row of buttons, and its Add Reaction
/// opens a submenu of twelve more as well as the picker; this item opens
/// only the picker.
pub(super) const QUICK_REACTIONS: Entry<MessageTarget> = Entry::Choices(Choices {
    name: "quick-reaction",
    choices: |app, target| {
        let selected = target.selection.as_deref().is_some_and(|s| !s.is_empty());
        if selected || !app.offers_add_reaction(&target.message) {
            return Vec::new();
        }
        let list = app.quick_reactions(target.message.channel_id);
        let label = |item: &discord::emoji::Item| match item {
            discord::emoji::Item::Unicode(e) => e.glyph.to_owned(),
            custom => format!(":{}:", custom.name()),
        };
        list.iter()
            .enumerate()
            .map(|(n, item)| Choice { value: n.to_string(), label: label(item), ticked: false })
            .collect()
    },
    style: Style::Plain,
    choose: |app, target, value, _| {
        let channel = target.message.channel_id;
        let item = value.parse().ok().and_then(|n: usize| app.quick_reactions(channel).into_iter().nth(n));
        if let Some(item) = item {
            app.add_reaction(channel, target.message.id, Emoji::from(&item), Location::ContextMenu, true);
        }
    },
});

/// Opens the emoji picker, as the client's does. The picker lists the
/// favourites and the emoji used most first.
pub(super) const ADD_REACTION: Entry<MessageTarget> = Entry::Item(Item {
    name: "add-reaction",
    label: "Add _Reaction",
    shows: |app, target| app.offers_add_reaction(&target.message),
    kind: MenuKind::Action(|app, target| app.pick_reaction(&target.message, &target.row)),
});

/// Remove Reactions, one emoji at a time, with Manage Messages (761304).
pub(super) const REMOVE_EMOJI_REACTIONS: Entry<MessageTarget> = Entry::Item(Item {
    name: "remove-emoji-reactions-menu",
    label: "Re_move Reactions",
    shows: |app, target| app.may_remove_reactions(&target.message),
    kind: MenuKind::Submenu(EMOJI_TO_REMOVE),
});

const EMOJI_TO_REMOVE: Table<MessageTarget> = &[&[Entry::Choices(Choices {
    name: "remove-emoji-reactions",
    choices: |_, target| {
        let labels = emoji_with_reactions(&target.message).into_iter().map(|emoji| emoji_label(&emoji));
        labels.enumerate().map(|(n, label)| Choice { value: n.to_string(), label, ticked: false }).collect()
    },
    style: Style::Plain,
    choose: |app, target, value, _| {
        let emoji = value.parse().ok().and_then(|n: usize| emoji_with_reactions(&target.message).into_iter().nth(n));
        if let Some(emoji) = emoji {
            app.remove_emoji_reactions(&target.message, emoji);
        }
    },
})]];

/// Shift removes without asking, as in the client.
pub(super) const REMOVE_ALL_REACTIONS: Entry<MessageTarget> = Entry::Item(Item {
    name: "remove-reactions",
    label: "Remove _All Reactions",
    shows: |app, target| app.may_remove_reactions(&target.message),
    kind: MenuKind::Action(|app, target| app.remove_all_reactions(&target.message)),
});

/// Each emoji the message has reactions with, once, in order.
fn emoji_with_reactions(message: &Message) -> Vec<Emoji> {
    let mut found: Vec<Emoji> = Vec::new();
    for reaction in &message.reactions {
        if !found.iter().any(|e| reaction.emoji.same(e)) {
            found.push(reaction.emoji.clone());
        }
    }
    found
}

/// What the client calls an emoji (`b3` in 815807): `:name:`, or the emoji
/// where it has no name for it.
pub(super) fn emoji_label(emoji: &Emoji) -> String {
    let name = emoji.name.as_deref().unwrap_or_default();
    match emoji.id {
        Some(_) => format!(":{name}:"),
        None => Some(discord::emoji::unicode_label(name)).filter(|l| !l.is_empty()).unwrap_or_else(|| name.to_owned()),
    }
}

/// How asking Discord ended, once the client's retry rule had its say.
enum Settled {
    Done,
    /// Neither sent again nor put back: see [`Verdict::Keep`], and a thread
    /// that wouldn't reopen.
    Kept,
    Undo(Option<Notice>),
}

impl App {
    /// Whether Add Reaction shows for `message`.
    pub(super) fn offers_add_reaction(&self, message: &Message) -> bool {
        let state = self.state.borrow();
        state.settings.media.render_reactions && state.reacting(message.channel_id).create
    }

    /// The emoji the quick reactions offer in `channel`: the favourites then
    /// the reactions used most, each once, up to four, without those Nitro
    /// locks and without the faces the client leaves out (`getEmojiInPriorityOrder`,
    /// 121681, and 761304).
    pub(super) fn quick_reactions(&self, channel: Snowflake) -> Vec<discord::emoji::Item> {
        use discord::emoji::{Intention, Item as EmojiItem, Library};
        use discord::limits::Unavailable;
        const LEFT_OUT: [&str; 5] = ["slight_smile", "frowning", "smile", "stuck_out_tongue", "wink"];
        let library = self.emoji_library(channel);
        let tone = self.state.borrow().settings.skin_tone;
        let shortcuts = self.emoji_shortcuts(&library, Intention::Reaction);
        let mut seen = Vec::new();
        shortcuts
            .favorites
            .into_iter()
            .chain(shortcuts.frequent)
            .filter(|item| {
                let key = Library::usage_key(item);
                let new = !seen.contains(&key);
                seen.push(key);
                new
            })
            .filter(|item| match item {
                EmojiItem::Unicode(e) => !LEFT_OUT.contains(&e.names[0]),
                EmojiItem::Custom(n) => n.status != Err(Unavailable::Nitro),
            })
            .take(4)
            .map(|item| super::emoji::toned(item, tone))
            .collect()
    }

    /// Put the reactions under `message` at the end of `content`.
    pub(super) fn append_reactions(self: &Rc<Self>, content: &gtk::Box, message: &Message) {
        if let Some(reactions) = self.reactions_row(message, content) {
            content.append(&reactions);
        }
    }

    /// The reactions under `message`, and the button that adds another:
    /// nothing without a reaction, or where the account hides them. A
    /// thread's first message stands for one in the parent, whose reactions
    /// this copy doesn't keep current. The add button's picker opens over
    /// `content`, which stays while the row is built again: the button
    /// itself goes with the next change, and a picker left on it would
    /// outlive it.
    fn reactions_row(self: &Rc<Self>, message: &Message, content: &gtk::Box) -> Option<gtk::Widget> {
        if message.thread_starter().is_some() {
            return None;
        }
        let pills = message.reaction_pills();
        let (show, access) = {
            let state = self.state.borrow();
            (state.settings.media.render_reactions, state.reacting(message.channel_id))
        };
        if !show || pills.is_empty() {
            return None;
        }
        let row = adw::WrapBox::builder().child_spacing(4).line_spacing(4).margin_top(4).build();
        for pill in &pills {
            row.append(&self.pill(message, pill, access.update));
        }
        if access.create && !message.is_ephemeral() {
            let add = gtk::Button::builder().icon_name("face-smile-symbolic").tooltip_text("Add Reaction").build();
            let (this, content) = (Rc::downgrade(self), content.downgrade());
            let (channel, id) = (message.channel_id, message.id);
            add.connect_clicked(move |_| {
                if let (Some(this), Some(content)) = (this.upgrade(), content.upgrade()) {
                    this.pick_emoji(channel, id, content.upcast_ref(), Location::InlineButton);
                }
            });
            row.append(&add);
        }
        Some(row.upcast())
    }

    /// One reaction: its emoji and count, lit when it's ours. Clicking adds
    /// or takes back ours, unless it can't be clicked. A super reaction is
    /// shown and counted but never clicked: Nitro's to send, and to take
    /// back.
    pub(super) fn pill(self: &Rc<Self>, message: &Message, pill: &Pill<'_>, clickable: bool) -> gtk::Widget {
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        content.append(&polls::emoji_widget(pill.emoji, &self.images));
        content.append(&gtk::Label::new(Some(&pill.count.to_string())));
        let button = gtk::ToggleButton::builder().child(&content).active(pill.me).build();
        if !clickable || pill.kind == Kind::Burst {
            button.set_can_target(false);
            button.set_can_focus(false);
            return button.upcast();
        }
        let (this, emoji, mine) = (Rc::downgrade(self), pill.emoji.clone(), pill.me);
        let (channel, id) = (message.channel_id, message.id);
        self.attach_reactors_tooltip(&button, channel, id, emoji.clone(), pill.count);
        button.connect_clicked(move |button| {
            // What it shows is the message's to say, once it changes.
            button.set_active(mine);
            let Some(this) = this.upgrade() else { return };
            match mine {
                true => this.take_back_reaction(channel, id, emoji.clone(), None, Location::InlineButton),
                false => this.add_reaction(channel, id, emoji.clone(), Location::InlineButton, false),
            }
        });
        button.upcast()
    }

    /// The emoji picker over `parent`, for a reaction to `message`.
    pub(super) fn pick_reaction(self: &Rc<Self>, message: &Message, parent: &gtk::Widget) {
        self.pick_emoji(message.channel_id, message.id, parent, Location::ReactionPicker);
    }

    fn pick_emoji(self: &Rc<Self>, channel: Snowflake, message: Snowflake, parent: &gtk::Widget, at: Location) {
        self.open_emoji_picker(channel, parent, discord::emoji::Intention::Reaction, move |app, item| {
            app.add_reaction(channel, message, Emoji::from(item), at, true);
        });
    }

    /// `+:name:` in the message box reacts to the newest message in place of
    /// sending (`reaction` in 751258). Says whether it did, or held back
    /// where the client wouldn't offer it: then the box has nothing to send.
    /// It's an ordinary message when the channel isn't showing its newest
    /// message, or no emoji has the name.
    pub(super) fn react_by_shortcut(self: &Rc<Self>, channel: Snowflake, typed: &str, library: &Library) -> bool {
        let Some(item) = discord::reactions::shortcut(typed).and_then(|name| library.find(name)) else {
            return false;
        };
        let newest = {
            let timelines = self.timelines.borrow();
            let present = timelines.get(channel).filter(|t| !t.detached);
            present.and_then(|t| timelines::newest_id(&t.store))
        };
        let Some(message) = newest else { return false };
        if item.unavailable().is_none() {
            self.add_reaction(channel, message, Emoji::from(&item), Location::Message, true);
        }
        true
    }

    /// React to `message` with `emoji`, as `BB` in 649963 does: the reaction
    /// shows at once, and goes if Discord refuses. `creating` is a reaction
    /// that may be a new one, which takes Add Reactions; clicking one that
    /// is there doesn't.
    pub(super) fn add_reaction(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        emoji: Emoji,
        location: Location,
        creating: bool,
    ) {
        let (rest, access) = {
            let state = self.state.borrow();
            (state.rest.clone(), state.reacting(channel))
        };
        // Our permissions may have changed since the reaction was offered.
        let Some(rest) = rest.filter(|_| if creating { access.create } else { access.update }) else { return };
        let clash = self.message_in(channel, message).is_some_and(|m| m.reacted_the_other_way(&emoji, Kind::Normal));
        if clash {
            return self.show_reaction_notice(Notice::Clash);
        }
        // Counted as it shows, whether or not Discord takes it.
        if let Some(key) = discord::emoji::reaction_key(&emoji) {
            self.track_reaction_use(&key);
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let settled = this
                .settle(|| {
                    let (this, rest, emoji) = (this.clone(), rest.clone(), emoji.clone());
                    async move {
                        this.change_message(channel, message, |m, dm| {
                            m.add_reaction(&emoji, true, Kind::Normal, &[], dm)
                        });
                        this.reopen(&rest, channel).await.ok()?;
                        Some(rest.add_reaction(channel, message, &emoji, location).await)
                    }
                })
                .await;
            if let Settled::Undo(notice) = settled {
                this.change_message(channel, message, |m, _| m.remove_reaction(&emoji, true, Kind::Normal));
                notice.inspect(|&notice| this.show_reaction_notice(notice));
            }
        });
    }

    /// Take back a reaction with `emoji`, ours or `user`'s, as `et` in 649963
    /// does. Ours goes at once and comes back if Discord refuses; someone
    /// else's isn't ours to show or hide, and changes when the gateway says.
    pub(super) fn take_back_reaction(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        emoji: Emoji,
        user: Option<Snowflake>,
        location: Location,
    ) {
        let (rest, access, manager, me) = {
            let state = self.state.borrow();
            (state.rest.clone(), state.reacting(channel), state.can_remove_reactions(channel), state.me())
        };
        let ours = user.is_none_or(|u| Some(u) == me);
        let user = user.filter(|_| !ours);
        let Some(rest) = rest.filter(|_| if ours { access.update } else { manager }) else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let settled = this
                .settle(|| {
                    let (this, rest, emoji) = (this.clone(), rest.clone(), emoji.clone());
                    async move {
                        if ours {
                            this.change_message(channel, message, |m, _| m.remove_reaction(&emoji, true, Kind::Normal));
                        }
                        this.reopen(&rest, channel).await.ok()?;
                        Some(rest.remove_reaction(channel, message, &emoji, location, user).await)
                    }
                })
                .await;
            if let Settled::Undo(notice) = settled {
                if ours {
                    this.change_message(channel, message, |m, dm| m.add_reaction(&emoji, true, Kind::Normal, &[], dm));
                }
                notice.inspect(|&notice| this.show_reaction_notice(notice));
            }
        });
    }

    /// Whether the moderators' entries show: Manage Messages, and something
    /// to remove.
    fn may_remove_reactions(&self, message: &Message) -> bool {
        !message.reactions.is_empty() && self.state.borrow().can_remove_reactions(message.channel_id)
    }

    /// Remove everyone's reactions with `emoji` (`Jf` in 649963), after
    /// asking unless Shift is held.
    fn remove_emoji_reactions(self: &Rc<Self>, message: &Message, emoji: Emoji) {
        let (channel, id) = (message.channel_id, message.id);
        let send = move |app: &Rc<App>| {
            let emoji = emoji.clone();
            app.moderate(channel, move |rest| {
                let (rest, emoji) = (rest.clone(), emoji.clone());
                async move { rest.remove_emoji_reactions(channel, id, &emoji).await }
            });
        };
        self.ask_then(
            "Remove Reactions for Emoji",
            "Are you sure you want to remove all of the reactions for this emoji from this message?",
            send,
        );
    }

    /// Remove every reaction from `message` (`Go` in 649963), after asking
    /// unless Shift is held.
    fn remove_all_reactions(self: &Rc<Self>, message: &Message) {
        let (channel, id) = (message.channel_id, message.id);
        let send = move |app: &Rc<App>| {
            app.moderate(channel, move |rest| {
                let rest = rest.clone();
                async move { rest.remove_reactions(channel, id).await }
            });
        };
        self.ask_then("Remove All Reactions", "Are you sure you want to remove all reactions from this message?", send);
    }

    /// Run `send` now if Shift is held, else once "Yes" is answered.
    fn ask_then(self: &Rc<Self>, heading: &str, body: &str, send: impl Fn(&Rc<App>) + 'static) {
        if self.shift_held() {
            return send(self);
        }
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_responses(&[("no", "_No"), ("yes", "_Yes")]);
        dialog.set_response_appearance("yes", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("no"));
        dialog.set_close_response("no");
        let this = self.clone();
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await == "yes" {
                send(&this);
            }
        });
    }

    /// Send a moderator's removal, reopening the thread first, with the
    /// client's retry rule. Nothing shows until the gateway says it's done.
    fn moderate<F, Fut>(self: &Rc<Self>, channel: Snowflake, request: F)
    where
        F: Fn(&discord::Rest) -> Fut + 'static,
        Fut: Future<Output = Result<(), Refusal>> + 'static,
    {
        // Our permissions may have changed since the menu opened.
        let rest = self.state.borrow().rest.clone().filter(|_| self.state.borrow().can_remove_reactions(channel));
        let Some(rest) = rest else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let settled = this
                .settle(|| {
                    let (this, rest) = (this.clone(), rest.clone());
                    let sent = request(&rest);
                    async move {
                        this.reopen(&rest, channel).await.ok()?;
                        Some(sent.await)
                    }
                })
                .await;
            if let Settled::Undo(Some(notice)) = settled {
                this.show_reaction_notice(notice);
            }
        });
    }

    /// Run `attempt` until the client's rule (see [`judge`]) says it's over.
    /// An attempt that is `None` never got to ask Discord (the thread
    /// wouldn't reopen), which the client leaves as it is.
    async fn settle<F, Fut>(&self, mut attempt: F) -> Settled
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Option<Result<(), Refusal>>>,
    {
        let mut retried = false;
        loop {
            let refusal = match attempt().await {
                None => return Settled::Kept,
                Some(Ok(())) => return Settled::Done,
                Some(Err(refusal)) => refusal,
            };
            match judge(&refusal, retried) {
                Verdict::Retry(wait) => {
                    retried = true;
                    if !wait.is_zero() {
                        glib::timeout_future(wait).await;
                    }
                }
                Verdict::Keep => return Settled::Kept,
                Verdict::Undo(notice) => return Settled::Undo(notice),
            }
        }
    }

    /// Change `message` wherever we hold it, open or not, and fill its row
    /// in again. `change` is told whether the channel is a DM, where the
    /// other side's reactions count once.
    fn change_message(self: &Rc<Self>, channel: Snowflake, message: Snowflake, change: impl Fn(&mut Message, bool)) {
        let dm = self.state.borrow().find_channel(channel).is_some_and(|(_, c)| c.kind == ChannelKind::Dm);
        // A post's card shows the top reaction of its first message.
        self.forum_change_first(channel, message, |m| change(m, dm));
        let changed = self.timelines.borrow().change(channel, message, |m| change(m, dm));
        if let Some((store, position)) = changed {
            self.rebind(&store, position, 1);
        }
        self.refresh_reactors_view(channel, message);
    }

    fn show_reaction_notice(self: &Rc<Self>, notice: Notice) {
        let dialog = adw::AlertDialog::new(Some(notice.title()), Some(notice.body()));
        dialog.add_response("okay", notice.confirm());
        let this = self.clone();
        glib::spawn_future_local(async move {
            dialog.choose_future(Some(&this.window)).await;
        });
    }

    /// Someone reacted, or took a reaction back. Held channels count too:
    /// the client keeps their reactions current, and so must we.
    pub(super) fn on_reaction(self: &Rc<Self>, reacted: &Reacted) {
        let Reacted { channel_id, message_id, user_id, emoji, kind, colors, added } = reacted;
        let mine = self.state.borrow().me() == Some(*user_id);
        {
            let mut state = self.state.borrow_mut();
            let known = match (&state.me, state.users.get(&user_id.0)) {
                (Some(me), _) if mine => Some(me.clone()),
                (_, Some(name)) => Some(discord::User {
                    id: *user_id,
                    username: name.clone(),
                    global_name: None,
                    avatar: None,
                    bot: false,
                    system: false,
                    discriminator: None,
                }),
                _ => None,
            };
            state.reactors.reacted(*message_id, emoji, *kind, *user_id, known, *added);
        }
        self.change_message(*channel_id, *message_id, |m, dm| match added {
            true => m.add_reaction(emoji, mine, *kind, colors, dm),
            false => m.remove_reaction(emoji, mine, *kind),
        });
    }

    /// Several people reacted at once.
    pub(super) fn on_reactions_added(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        reactions: &[(Emoji, Vec<Snowflake>)],
    ) {
        let me = self.state.borrow().me();
        self.change_message(channel, message, |m, _| m.add_reactions(reactions, me));
    }

    /// A moderator removed every reaction, or one emoji's.
    pub(super) fn on_reactions_cleared(self: &Rc<Self>, channel: Snowflake, message: Snowflake, emoji: Option<&Emoji>) {
        self.state.borrow_mut().reactors.removed(message, emoji);
        self.change_message(channel, message, |m, _| match emoji {
            Some(emoji) => m.remove_reactions_for_emoji(emoji),
            None => m.reactions.clear(),
        });
    }
}
