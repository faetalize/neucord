//! System messages: joins, pins, boosts, calls and the like, which Discord
//! sends as messages of their own type. What they say is worded as the
//! official client words it.

use crate::model::{Message, Snowflake};

/// How a message shows in the list.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// Something someone wrote: avatar, name, content.
    Normal,
    /// A line about something that happened.
    System(System),
    /// Nothing at all, as in the official client.
    Hidden,
}

#[derive(Clone, Debug, PartialEq)]
pub struct System {
    pub icon: Icon,
    pub parts: Vec<Part>,
}

impl System {
    /// The line as plain text.
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .map(|part| match part {
                Part::Text(t) | Part::Name(t) | Part::Strong(t) | Part::Jump { label: t, .. } => t.as_str(),
            })
            .collect()
    }
}

/// What a system message is about, for the icon beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Join,
    Leave,
    Call,
    MissedCall,
    Pin,
    Boost,
    Edit,
    Thread,
    Stage,
    Alert,
    Poll,
    Other,
}

/// A piece of a system message's text.
#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Text(String),
    /// Someone's name: the author's, or someone they acted on.
    Name(String),
    /// Something named, like a channel or the boost level reached.
    Strong(String),
    /// Text that jumps to a message in the same channel.
    Jump {
        label: String,
        to: Snowflake,
    },
}

/// The message types people write; everything else is a system message.
const USER_MESSAGES: [u8; 9] = [0, 19, 20, 23, 26, 41, 45, 47, 68];
/// System messages the official client shows nothing for.
const HIDDEN: [u8; 7] = [22, 40, 42, 55, 56, 57, 63];

/// The official client picks a welcome by the message's time.
const JOINS: [(&str, &str); 13] = [
    ("", " joined the party."),
    ("", " is here."),
    ("Welcome, ", ". We hope you brought pizza."),
    ("A wild ", " appeared."),
    ("", " just landed."),
    ("", " just slid into the server."),
    ("", " just showed up!"),
    ("Welcome ", ". Say hi!"),
    ("", " hopped into the server."),
    ("Everyone welcome ", "!"),
    ("Glad you're here, ", "."),
    ("Good to see you, ", "."),
    ("Yay you made it, ", "!"),
];

impl Message {
    /// Something someone wrote, not a system message.
    pub fn is_user_message(&self) -> bool {
        USER_MESSAGES.contains(&self.kind)
    }
}

/// Where a message was sent, as far as its system text needs to know.
pub struct Place<'a> {
    /// Us, to tell a missed call from one we were in.
    pub me: Option<Snowflake>,
    pub guild_name: Option<&'a str>,
    /// A group DM, where a name change renames the group.
    pub group: bool,
}

/// How `message` shows.
pub fn kind(message: &Message, place: &Place) -> Kind {
    if message.is_user_message() {
        return Kind::Normal;
    }
    if HIDDEN.contains(&message.kind) {
        return Kind::Hidden;
    }
    let author = || Part::Name(message.author.display_name().to_owned());
    let text = |s: &str| Part::Text(s.to_owned());
    let other = message.mentions.first();
    let guild = place.guild_name.unwrap_or("this server").to_owned();
    let (icon, parts) = match message.kind {
        1 => (
            Icon::Join,
            vec![
                author(),
                text(" added "),
                Part::Name(other.map_or("someone", |u| u.display_name()).to_owned()),
                text(" to the group."),
            ],
        ),
        2 => match other.filter(|u| u.id != message.author.id) {
            Some(other) => (
                Icon::Leave,
                vec![
                    author(),
                    text(" removed "),
                    Part::Name(other.display_name().to_owned()),
                    text(" from the group."),
                ],
            ),
            None => (Icon::Leave, vec![author(), text(" left the group.")]),
        },
        3 => {
            let missed = message.call.as_ref().is_some_and(|call| {
                call.ended_timestamp.is_some() && place.me.is_some_and(|me| !call.participants.contains(&me))
            });
            let lasted = message
                .call
                .as_ref()
                .and_then(|call| call_duration(&message.timestamp, call.ended_timestamp.as_deref()?));
            match (missed, lasted) {
                (true, Some(lasted)) => (
                    Icon::MissedCall,
                    vec![text("You missed a call from "), author(), text(&format!(" that lasted {lasted}."))],
                ),
                (true, None) => (Icon::MissedCall, vec![text("You missed a call from "), author(), text(".")]),
                (false, Some(lasted)) => {
                    (Icon::Call, vec![author(), text(&format!(" started a call that lasted {lasted}."))])
                }
                (false, None) => (Icon::Call, vec![author(), text(" started a call.")]),
            }
        }
        4 if place.group && message.content.is_empty() => {
            (Icon::Edit, vec![author(), text(" removed the custom group name.")])
        }
        4 => {
            let what = if place.group { " changed the group name: " } else { " changed the channel name: " };
            (Icon::Edit, vec![author(), text(what), Part::Strong(message.content.clone())])
        }
        5 => (Icon::Edit, vec![author(), text(" changed the group icon.")]),
        6 => {
            let pinned = match message.message_reference.as_ref().and_then(|r| r.message_id) {
                Some(to) => Part::Jump { label: "a message".into(), to },
                None => text("a message"),
            };
            (Icon::Pin, vec![author(), text(" pinned "), pinned, text(" to this channel.")])
        }
        7 if message.author.bot => (Icon::Join, vec![author(), text(" was added to the server.")]),
        7 => {
            let (before, after) = JOINS[join_variant(message.id)];
            (Icon::Join, vec![text(before), author(), text(after)])
        }
        8..=11 => {
            // The content is how many boosts it was.
            let count = message.content.parse::<u32>().ok().filter(|&n| n > 0).unwrap_or(1);
            let mut parts = vec![author()];
            if count > 1 {
                parts.extend([text(" just boosted the server "), Part::Strong(count.to_string()), text(" times!")]);
            } else {
                parts.push(text(" just boosted the server!"));
            }
            if message.kind > 8
                && let Some(guild) = place.guild_name
            {
                parts.extend([
                    text(&format!(" {guild} has achieved ")),
                    Part::Strong(format!("Level {}!", message.kind - 8)),
                ]);
            }
            (Icon::Boost, parts)
        }
        12 => (
            Icon::Other,
            vec![
                author(),
                text(" has added "),
                Part::Strong(message.content.clone()),
                text(" to this channel. Its most important updates will show up here."),
            ],
        ),
        14 => (
            Icon::Alert,
            vec![text(&format!(
                "{guild} has been removed from Server Discovery because it no longer passes all the requirements. \
             Check Server Settings for more details."
            ))],
        ),
        15 => (
            Icon::Other,
            vec![text("This server is eligible for Server Discovery again and has been automatically relisted!")],
        ),
        16 => (
            Icon::Alert,
            vec![text(&format!(
                "{guild} has failed Discovery activity requirements for 1 week. If it fails for 4 weeks in a row, \
             it will be automatically removed from Discovery."
            ))],
        ),
        17 => (
            Icon::Alert,
            vec![text(&format!(
                "{guild} has failed Discovery activity requirements for 3 weeks in a row. If it fails for 1 more \
             week, it will be removed from Discovery."
            ))],
        ),
        18 => (
            Icon::Thread,
            vec![author(), text(" started a thread: "), Part::Strong(message.content.clone()), text(".")],
        ),
        27 => (Icon::Stage, vec![author(), text(" started "), Part::Strong(message.content.clone())]),
        28 => (Icon::Stage, vec![author(), text(" ended "), Part::Strong(message.content.clone())]),
        29 => (Icon::Stage, vec![author(), text(" is now a speaker.")]),
        30 => (Icon::Stage, vec![author(), text(" requested to speak.")]),
        31 => (Icon::Stage, vec![author(), text(" changed the Stage topic: "), Part::Strong(message.content.clone())]),
        37 => (Icon::Alert, vec![author(), text(" disabled security actions.")]),
        38 => (Icon::Alert, vec![author(), text(&format!(" reported a raid in {guild}."))]),
        39 => (Icon::Alert, vec![author(), text(" resolved an Activity Alert.")]),
        46 => {
            let question = poll_result_field(message, "poll_question_text").unwrap_or("A poll").to_owned();
            let question = match message.message_reference.as_ref().and_then(|r| r.message_id) {
                Some(to) => Part::Jump { label: question, to },
                None => Part::Strong(question),
            };
            (Icon::Poll, vec![author(), text("'s poll "), question, text(" has closed.")])
        }
        67 => (Icon::Join, vec![author(), text(" accepted your friend request.")]),
        // What the official client draws from data we don't follow yet.
        _ => (Icon::Other, vec![author(), text(" sent something Neucord can't show yet.")]),
    };
    Kind::System(System { icon, parts })
}

/// One of the fields of a POLL_RESULT message's embed.
pub fn poll_result_field<'m>(message: &'m Message, name: &str) -> Option<&'m str> {
    let embed = message.embeds.iter().find(|e| e.kind == crate::embed::EmbedKind::PollResult)?;
    embed.fields.iter().find(|f| f.name == name).map(|f| f.value.as_str())
}

/// Which welcome a join shows. The official client converts the id to a
/// double first, which this matches.
fn join_variant(id: Snowflake) -> usize {
    let unix_ms = (id.0 as f64 / 4_194_304.0).floor() + 1_420_070_400_000.0;
    (unix_ms % JOINS.len() as f64) as usize
}

/// How long a call lasted, in the official client's (moment.js) words.
fn call_duration(started: &str, ended: &str) -> Option<String> {
    let started = glib::DateTime::from_iso8601(started, None).ok()?;
    let ended = glib::DateTime::from_iso8601(ended, None).ok()?;
    Some(humanize(ended.difference(&started).as_seconds().max(0)))
}

/// A duration as moment.js's `humanize` puts it.
pub fn humanize(seconds: i64) -> String {
    let round = |unit: f64| (seconds as f64 / unit).round() as i64;
    let (minutes, hours, days) = (round(60.0), round(3600.0), round(86400.0));
    match () {
        _ if seconds < 45 => "a few seconds".into(),
        _ if seconds < 90 => "a minute".into(),
        _ if minutes < 45 => format!("{minutes} minutes"),
        _ if minutes < 90 => "an hour".into(),
        _ if hours < 22 => format!("{hours} hours"),
        _ if hours < 36 => "a day".into(),
        _ if days < 26 => format!("{days} days"),
        _ if days < 45 => "a month".into(),
        _ if days < 320 => format!("{} months", (days as f64 / 30.4).round() as i64),
        _ if days < 548 => "a year".into(),
        _ => format!("{} years", (days as f64 / 365.0).round() as i64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(json: &str) -> Message {
        let base = r#""id":"1088216706570268682","channel_id":"1","author":{"id":"5","username":"alice"},
                      "timestamp":"2026-10-26T12:00:00.000000+00:00""#;
        serde_json::from_str(&format!("{{{base},{json}}}")).unwrap()
    }

    fn line(kind: Kind) -> String {
        let Kind::System(system) = kind else { panic!("not a system message: {kind:?}") };
        system.text()
    }

    const DM: Place = Place { me: Some(Snowflake(5)), guild_name: None, group: false };

    #[test]
    fn ordinary_and_hidden() {
        assert_eq!(kind(&message(r#""type":0"#), &DM), Kind::Normal);
        assert_eq!(kind(&message(r#""type":19"#), &DM), Kind::Normal);
        assert_eq!(kind(&message(r#""type":63"#), &DM), Kind::Hidden);
    }

    #[test]
    fn group_changes() {
        let bob = r#""mentions":[{"id":"6","username":"bob"}]"#;
        assert_eq!(line(kind(&message(&format!(r#""type":1,{bob}"#)), &DM)), "alice added bob to the group.");
        assert_eq!(line(kind(&message(&format!(r#""type":2,{bob}"#)), &DM)), "alice removed bob from the group.");
        assert_eq!(
            line(kind(&message(r#""type":2,"mentions":[{"id":"5","username":"alice"}]"#), &DM)),
            "alice left the group."
        );
        let group = Place { group: true, ..DM };
        assert_eq!(line(kind(&message(r#""type":4,"content":"crew""#), &group)), "alice changed the group name: crew");
        assert_eq!(line(kind(&message(r#""type":4,"content":"""#), &group)), "alice removed the custom group name.");
        assert_eq!(line(kind(&message(r#""type":4,"content":"news""#), &DM)), "alice changed the channel name: news");
    }

    #[test]
    fn calls() {
        let call = |participants: &str, ended: &str| {
            message(&format!(r#""type":3,"call":{{"participants":[{participants}],"ended_timestamp":{ended}}}"#))
        };
        let me = Place { me: Some(Snowflake(7)), ..DM };
        let ended = r#""2026-10-26T12:05:10.000000+00:00""#;
        assert_eq!(line(kind(&call(r#""5""#, ended), &me)), "You missed a call from alice that lasted 5 minutes.");
        assert_eq!(line(kind(&call(r#""5","7""#, ended), &me)), "alice started a call that lasted 5 minutes.");
        assert_eq!(line(kind(&call(r#""5""#, "null"), &me)), "alice started a call.", "still going");
    }

    #[test]
    fn pins_joins_and_boosts() {
        let pin = message(r#""type":6,"message_reference":{"message_id":"99"}"#);
        assert_eq!(line(kind(&pin, &DM)), "alice pinned a message to this channel.");
        // 1088216706570268682 reads as 1679521480935 ms, which is 12 mod 13.
        assert_eq!(line(kind(&message(r#""type":7"#), &DM)), "Yay you made it, alice!");
        let guild = Place { guild_name: Some("Crew"), ..DM };
        assert_eq!(line(kind(&message(r#""type":8,"content":"""#), &guild)), "alice just boosted the server!");
        assert_eq!(
            line(kind(&message(r#""type":10,"content":"3""#), &guild)),
            "alice just boosted the server 3 times! Crew has achieved Level 2!"
        );
        assert_eq!(line(kind(&message(r#""type":99"#), &guild)), "alice sent something Neucord can't show yet.");
    }

    #[test]
    fn humanized_durations() {
        assert_eq!(humanize(10), "a few seconds");
        assert_eq!(humanize(60), "a minute");
        assert_eq!(humanize(300), "5 minutes");
        assert_eq!(humanize(3600), "an hour");
        assert_eq!(humanize(5 * 3600), "5 hours");
        assert_eq!(humanize(30 * 3600), "a day");
        assert_eq!(humanize(3 * 86400), "3 days");
    }
}
