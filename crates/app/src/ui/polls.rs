//! Polls, and the results of closed ones. Voting follows the official
//! client's (180662, 951727): answers are picked, then sent with Vote, which
//! shows the new counts at once and puts them back if Discord says no.

use std::rc::{Rc, Weak};

use adw::prelude::*;
use discord::model::Emoji;
use discord::permissions;
use discord::poll::{Ballot, Outcome, Poll, Primary};
use discord::{Message, Snowflake, verification};
use gtk::{glib, pango};

use super::menus::{Entry, Item, Kind};
use super::state::now_unix;
use super::{App, friends::confirm};
use crate::images::Images;
use crate::timelines;

/// Custom emoji beside an answer (the official client's size).
const EMOJI: i32 = 22;

/// "End Poll Now" on our own poll, which the client (761304) offers until
/// its expiry; the poll ending is what its message update says.
pub(super) const END_POLL: Entry<super::chat::MessageTarget> = Entry::Item(Item {
    name: "end-poll",
    label: "End _Poll Now",
    shows: |app, target| {
        let message = &target.message;
        message.poll.as_ref().is_some_and(|p| p.time_left(now_unix()).is_some())
            && app.state.borrow().me() == Some(message.author.id)
    },
    kind: Kind::Action(|app, target| app.confirm_end_poll(&target.message)),
});

/// A poll: its question, each answer with its share once counts show, and
/// how many voted and for how long.
pub(super) fn poll(app: &Rc<App>, message: &Message, poll: &Poll) -> gtk::Widget {
    let now = now_unix();
    let held_back = {
        let state = app.state.borrow();
        permissions::vote_held_back(state.find_channel(message.channel_id).and_then(|(guild, _)| guild), now)
    };
    let view = poll.view(held_back, now);
    let closed = poll.is_closed(now);
    let (channel, id) = (message.channel_id, message.id);
    let this = Rc::downgrade(app);
    let (frame, column) = card();
    let question = poll.question.text.as_deref().unwrap_or_default();
    column.append(&wrapping(question, &["heading"]));
    if let Some(prompt) = view.prompt {
        column.append(&wrapping(prompt, &["caption", "dimmed"]));
    }
    for answer in &poll.answers {
        let answer_id = answer.answer_id;
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if view.tappable {
            line.append(&pick_mark(poll, poll.ballot.picked.contains(&answer_id)));
        }
        if let Some(emoji) = &answer.poll_media.emoji {
            line.append(&emoji_widget(emoji, &app.images));
        }
        let text = wrapping(answer.poll_media.text.as_deref().unwrap_or_default(), &[]);
        text.set_hexpand(true);
        // The winner of a closed poll stands out.
        if closed && poll.is_leader(answer_id) {
            text.add_css_class("heading");
        }
        line.append(&text);
        if poll.me_voted(answer_id) {
            line.append(&gtk::Image::builder().icon_name("object-select-symbolic").tooltip_text("Your vote").build());
        }
        let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        if view.tappable {
            let button = gtk::Button::builder()
                .child(&line)
                .css_classes(["flat"])
                .accessible_role(if poll.allow_multiselect {
                    gtk::AccessibleRole::Checkbox
                } else {
                    gtk::AccessibleRole::Radio
                })
                .build();
            let checked = match poll.ballot.picked.contains(&answer_id) {
                true => gtk::AccessibleTristate::True,
                false => gtk::AccessibleTristate::False,
            };
            button.update_state(&[gtk::accessible::State::Checked(checked)]);
            button.connect_clicked(with(&this, move |app| app.tap_answer(channel, id, answer_id)));
            row.append(&button);
        } else {
            row.append(&line);
        }
        if view.counts {
            let votes = match poll.votes(answer_id) {
                1 => "1 vote".to_owned(),
                n => format!("{n} votes"),
            };
            line.append(
                &gtk::Label::builder()
                    .label(format!("{votes}  {}%", poll.percent(answer_id)))
                    .css_classes(["caption", "numeric"])
                    .build(),
            );
            row.append(&gtk::ProgressBar::builder().fraction(poll.percent(answer_id) as f64 / 100.0).build());
        }
        column.append(&row);
    }
    column.append(&wrapping(&poll.footer(now), &["caption", "dimmed"]));
    let actions = gtk::Box::builder().spacing(8).halign(gtk::Align::End).build();
    let button = |label: &str, classes: &[&str]| gtk::Button::builder().label(label).css_classes(classes).build();
    if view.show_results {
        let show = button("Show results", &["flat"]);
        show.connect_clicked(with(&this, move |app| app.toggle_results(channel, id)));
        actions.append(&show);
    }
    if view.cancel {
        let cancel = button("Cancel", &["flat"]);
        cancel.connect_clicked(with(&this, move |app| app.cancel_vote(channel, id)));
        actions.append(&cancel);
    }
    match view.primary {
        Some(Primary::Vote { enabled }) => {
            let vote = button("Vote", &["suggested-action"]);
            vote.set_sensitive(enabled);
            vote.connect_clicked(with(&this, move |app| app.submit_vote(channel, id, false)));
            actions.append(&vote);
        }
        Some(Primary::RemoveVote) => {
            let remove = button("Remove Vote", &[]);
            remove.connect_clicked(with(&this, move |app| app.submit_vote(channel, id, true)));
            actions.append(&remove);
        }
        Some(Primary::GoBackToVote) => {
            let back = button("Go back to vote", &[]);
            back.connect_clicked(with(&this, move |app| app.toggle_results(channel, id)));
            actions.append(&back);
        }
        None => {}
    }
    if actions.first_child().is_some() {
        column.append(&actions);
    }
    frame.upcast()
}

/// The radio button or checkbox before an answer.
fn pick_mark(poll: &Poll, picked: bool) -> gtk::Image {
    let icon = match (poll.allow_multiselect, picked) {
        (false, false) => "radio-symbolic",
        (false, true) => "radio-checked-symbolic",
        (true, false) => "checkbox-symbolic",
        (true, true) => "checkbox-checked-symbolic",
    };
    gtk::Image::from_icon_name(icon)
}

/// A handler that does nothing once the app is gone.
fn with<W>(app: &Weak<App>, act: impl Fn(&Rc<App>) + 'static) -> impl Fn(&W) + 'static {
    let app = app.clone();
    move |_| {
        if let Some(app) = app.upgrade() {
            act(&app);
        }
    }
}

impl App {
    /// `change` made to the poll of a message held in `channel`, and its row
    /// drawn again, if the message is held. What it gives back is `None`
    /// when it isn't.
    fn edit_poll<R>(
        self: &Rc<Self>,
        channel: Snowflake,
        message: Snowflake,
        change: impl FnOnce(&mut Poll) -> R,
    ) -> Option<R> {
        let store = self.timelines.borrow().get(channel).map(|t| t.store.clone())?;
        let mut result = None;
        let at = timelines::edit(&store, message, |m| result = m.poll.as_mut().map(change))?;
        self.rebind(&store, at, 1);
        result
    }

    /// An answer was tapped. Only ones the view offers are: the rows of a
    /// poll that shows counts aren't buttons.
    fn tap_answer(self: &Rc<Self>, channel: Snowflake, message: Snowflake, answer: u32) {
        let now = now_unix();
        let held_back = self.votes_held_back(channel, now);
        self.edit_poll(channel, message, |poll| {
            if poll.view(held_back, now).tappable {
                poll.ballot.tap(answer, poll.allow_multiselect);
            }
        });
    }

    /// "Show results", and "Go back to vote".
    fn toggle_results(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
        self.edit_poll(channel, message, |poll| poll.ballot.toggle_results());
    }

    /// "Cancel" on a vote being edited.
    fn cancel_vote(self: &Rc<Self>, channel: Snowflake, message: Snowflake) {
        self.edit_poll(channel, message, |poll| poll.ballot.clear());
    }

    fn votes_held_back(&self, channel: Snowflake, now: i64) -> bool {
        let state = self.state.borrow();
        permissions::vote_held_back(state.find_channel(channel).and_then(|(guild, _)| guild), now)
    }

    /// Whether the server's verification stops us from chatting there, as
    /// `canChatInGuild` (834942) says. Membership screening isn't part of it.
    fn verification_blocks(&self, channel: Snowflake) -> bool {
        let state = self.state.borrow();
        match (state.find_channel(channel), &state.account) {
            (Some((Some(guild), _)), Some(me)) => {
                verification::gate(guild, me, now_unix()).is_some_and(|gate| gate != verification::Gate::Screening)
            }
            _ => false,
        }
    }

    /// Send the picked answers as our vote, or with `remove`, none. The counts
    /// change first and go back if Discord refuses (`handlePollSubmitVote`).
    fn submit_vote(self: &Rc<Self>, channel: Snowflake, message: Snowflake, remove: bool) {
        if self.verification_blocks(channel) {
            return self.notice(
                "You can't vote on this poll yet!",
                "You must complete the verification steps required by this server.",
            );
        }
        let Some(rest) = self.state.borrow().rest.clone() else { return };
        let (now, held_back) = (now_unix(), self.votes_held_back(channel, now_unix()));
        let started = self.edit_poll(channel, message, |poll| {
            if remove {
                poll.ballot.clear();
            } else if poll.view(held_back, now).primary != Some(Primary::Vote { enabled: true }) {
                return None;
            }
            let before = poll.my_votes();
            let answers = poll.ballot.start_submit();
            poll.set_my_votes(&answers);
            Some((before, answers))
        });
        let Some(Some((before, answers))) = started else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            match rest.vote(channel, message, &answers).await {
                Ok(()) => {
                    this.edit_poll(channel, message, |poll| poll.ballot = Ballot::default());
                    let said = if answers.is_empty() { "You have removed your vote" } else { "You have voted" };
                    this.window.upcast_ref::<gtk::Widget>().announce(said, gtk::AccessibleAnnouncementPriority::Medium);
                }
                Err(e) => {
                    this.notice("Oh no! Something went wrong.", &e.to_string());
                    this.edit_poll(channel, message, |poll| {
                        poll.set_my_votes(&before);
                        poll.ballot.failed();
                    });
                }
            }
        });
    }

    /// Ask, then end `message`'s poll (`endPollEarly`).
    pub(super) fn confirm_end_poll(self: &Rc<Self>, message: &Message) {
        let dialog = confirm(
            "End Poll Now?",
            "This will close the poll immediately and reveal the results.",
            "_Okay",
            adw::ResponseAppearance::Suggested,
        );
        let (this, channel, id) = (self.clone(), message.channel_id, message.id);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != "confirm" {
                return;
            }
            let Some(rest) = this.state.borrow().rest.clone() else { return };
            // The gateway's update is what closes it in the list.
            if let Err(e) = rest.end_poll(channel, id).await {
                this.notice("Oh no! Something went wrong.", &e.to_string());
            }
        });
    }

    /// A dialog with a message and "Okay".
    fn notice(&self, heading: &str, body: &str) {
        let dialog = adw::AlertDialog::new(Some(heading), Some(body));
        dialog.add_response("ok", "_Okay");
        dialog.present(Some(&self.window));
    }
}

/// What a closed poll came to, under the system message saying so.
pub(super) fn outcome(outcome: &Outcome, images: &Images) -> gtk::Widget {
    let (frame, column) = card();
    match outcome {
        Outcome::NoVotes => column.append(&wrapping("There was no winner", &["heading"])),
        Outcome::Tie => column.append(&wrapping("The results were tied", &["heading"])),
        Outcome::Winner { text, emoji, percent } => {
            let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if let Some(emoji) = emoji {
                line.append(&emoji_widget(emoji, images));
            }
            line.append(&wrapping(text, &["heading"]));
            column.append(&line);
            column.append(&wrapping(&format!("Winning answer • {percent}%"), &["caption", "dimmed"]));
        }
    }
    frame.upcast()
}

/// A libadwaita card, and the column inside it.
fn card() -> (gtk::Box, gtk::Box) {
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    let frame =
        gtk::Box::builder().halign(gtk::Align::Start).width_request(320).margin_top(4).css_classes(["card"]).build();
    frame.append(&column);
    (frame, column)
}

fn wrapping(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(classes)
        .build()
}

/// A custom emoji's image, or a Unicode one as text.
pub(super) fn emoji_widget(emoji: &Emoji, images: &Images) -> gtk::Widget {
    match emoji.id {
        Some(id) => {
            let picture = gtk::Picture::builder()
                .width_request(EMOJI)
                .height_request(EMOJI)
                .content_fit(gtk::ContentFit::Contain)
                .valign(gtk::Align::Center)
                .build();
            if let Some(name) = &emoji.name {
                picture.set_tooltip_text(Some(&format!(":{name}:")));
            }
            let size = ((EMOJI * images.scale()) as u32).next_power_of_two().clamp(16, 4096) as u16;
            images.set(&picture, &discord::cdn::emoji_url(id.0, emoji.animated, size), emoji.animated);
            picture.upcast()
        }
        None => gtk::Label::new(emoji.name.as_deref()).upcast(),
    }
}
