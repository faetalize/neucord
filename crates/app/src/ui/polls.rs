//! Polls, and the results of closed ones. Read-only for now: Neucord
//! doesn't vote yet.

use discord::poll::{Emoji, Outcome, Poll};
use gtk::pango;
use gtk::prelude::*;

use super::state::now_unix;
use crate::images::Images;

/// Custom emoji beside an answer (the official client's size).
const EMOJI: i32 = 22;

/// A poll: its question, each answer with its share once counts show, and
/// how many voted and for how long.
pub(super) fn poll(poll: &Poll, images: &Images) -> gtk::Widget {
    let now = now_unix();
    let shows = poll.shows_counts(now);
    let closed = poll.is_closed(now);
    let (frame, column) = card();
    let question = poll.question.text.as_deref().unwrap_or_default();
    column.append(&wrapping(question, &["heading"]));
    if !closed {
        column.append(&wrapping(poll.prompt(), &["caption", "dimmed"]));
    }
    for answer in &poll.answers {
        let id = answer.answer_id;
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        if let Some(emoji) = &answer.poll_media.emoji {
            line.append(&emoji_widget(emoji, images));
        }
        let text = wrapping(answer.poll_media.text.as_deref().unwrap_or_default(), &[]);
        text.set_hexpand(true);
        // The winner of a closed poll stands out.
        if closed && poll.is_leader(id) {
            text.add_css_class("heading");
        }
        line.append(&text);
        if poll.me_voted(id) {
            line.append(&gtk::Image::builder().icon_name("object-select-symbolic").tooltip_text("Your vote").build());
        }
        let row = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
        row.append(&line);
        if shows {
            let votes = match poll.votes(id) {
                1 => "1 vote".to_owned(),
                n => format!("{n} votes"),
            };
            line.append(
                &gtk::Label::builder()
                    .label(format!("{votes}  {}%", poll.percent(id)))
                    .css_classes(["caption", "numeric"])
                    .build(),
            );
            row.append(&gtk::ProgressBar::builder().fraction(poll.percent(id) as f64 / 100.0).build());
        }
        column.append(&row);
    }
    column.append(&wrapping(&poll.footer(now), &["caption", "dimmed"]));
    frame.upcast()
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
fn emoji_widget(emoji: &Emoji, images: &Images) -> gtk::Widget {
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
