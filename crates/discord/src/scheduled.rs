//! Scheduled messages: a message Discord holds and sends at a time we pick
//! (`/users/@me/scheduled-messages`). The client's rules for them are in
//! 566908 (anchor: the `Scheduled Messages` logger), its requests in 151282
//! and 138399's `_sendMessage`, and the list in the inbox's tab in 713024.
//!
//! In build 668923 the feature is on for every account. The limit on how
//! many may wait at once is [`crate::limits::Limits::scheduled_messages`].

use serde_json::Value;

use crate::model::Snowflake;

/// Discord's error code for one message too many waiting
/// (`TOO_MANY_SCHEDULED_MESSAGES`).
pub const TOO_MANY: u64 = 640_003;

/// The soonest a message may go out, from now: 10 minutes (`S` in 566908).
const MIN_AHEAD: i64 = 600;
/// The furthest, for a new one: 8 days (`N`).
const MAX_AHEAD: i64 = 691_200;
/// A waiting message may be moved up to 30 days after it was scheduled
/// (853297's `a`, read by `N`).
const MAX_WAIT: i64 = 2_592_000;

/// Why a time can't be chosen, in the client's words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    TooSoon,
    TooFar,
}

impl Refusal {
    pub fn text(self) -> &'static str {
        match self {
            Refusal::TooSoon => "Scheduled time is too soon",
            Refusal::TooFar => "Scheduled time is too far away",
        }
    }
}

/// Whether `at` (unix seconds) is a time a message may be sent at, `now`
/// being the time of the check (`Vf`). `waiting` is the scheduled message
/// being moved.
pub fn check(at: i64, now: i64, waiting: Option<Snowflake>) -> Result<(), Refusal> {
    if at < earliest(now) {
        Err(Refusal::TooSoon)
    } else if at > latest(now, waiting) {
        Err(Refusal::TooFar)
    } else {
        Ok(())
    }
}

/// The soonest time the picker allows (`_O`).
pub fn earliest(now: i64) -> i64 {
    now + MIN_AHEAD
}

/// The furthest it allows (`yp`): 8 days on, and for a message being moved
/// no later than 30 days after it was scheduled, which its id says.
pub fn latest(now: i64, waiting: Option<Snowflake>) -> i64 {
    let ahead = now + MAX_AHEAD;
    waiting.map_or(ahead, |id| ahead.min(id.unix_ms() / 1000 + MAX_WAIT))
}

/// One of the picks the attach menu offers before a custom time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Worded for its time, which doesn't move with it: the client's labels
    /// are fixed text.
    pub label: &'static str,
    /// Unix seconds.
    pub at: i64,
}

/// A local wall-clock time on the day `days` from `day`.
fn at_hour(day: &glib::DateTime, days: i32, hour: i32) -> Option<glib::DateTime> {
    let day = day.add_days(days).ok()?;
    glib::DateTime::new(&day.timezone(), day.year(), day.month(), day.day_of_month(), hour, 0, 0.0).ok()
}

/// 9:00 and 13:00 on the first day they are 10 minutes ahead, and the 9:00
/// of next Monday (`FF`). `tz` is where the user is.
pub fn presets(now: i64, tz: &glib::TimeZone) -> Vec<Preset> {
    let Ok(today) = glib::DateTime::from_unix_utc(now).and_then(|t| t.to_timezone(tz)) else { return Vec::new() };
    let soonest = earliest(now);
    let next = |hour, now_label, later_label| {
        let at = at_hour(&today, 0, hour)?;
        let (label, at) =
            if at.to_unix() > soonest { (now_label, at) } else { (later_label, at_hour(&today, 1, hour)?) };
        Some(Preset { label, at: at.to_unix() })
    };
    let monday =
        at_hour(&today, 8 - today.day_of_week(), 9).map(|at| Preset { label: "Monday at 9:00am", at: at.to_unix() });
    [next(9, "Today at 9:00am", "Tomorrow at 9:00am"), next(13, "Today at 1:00pm", "Tomorrow at 1:00pm"), monday]
        .into_iter()
        .flatten()
        .collect()
}

/// Where the picker starts: the next full hour that is 10 minutes ahead (`US`).
pub fn default_time(now: i64, tz: &glib::TimeZone) -> i64 {
    let hour = |t: &glib::DateTime| {
        glib::DateTime::new(tz, t.year(), t.month(), t.day_of_month(), t.hour(), 0, 0.0).and_then(|t| t.add_hours(1))
    };
    let Ok(first) = glib::DateTime::from_unix_utc(now).and_then(|t| t.to_timezone(tz)).and_then(|t| hour(&t)) else {
        return earliest(now);
    };
    let first = first.to_unix();
    if first < earliest(now) { first + 3600 } else { first }
}

/// `at` as the request names it, as JavaScript's `toISOString` writes it:
/// UTC, with milliseconds.
pub fn iso(at: i64) -> String {
    glib::DateTime::from_unix_utc(at)
        .and_then(|t| t.format("%Y-%m-%dT%H:%M:%S.000Z"))
        .map_or_else(|_| String::new(), String::from)
}

/// The flag that makes a message send without notifying (`SUPPRESS_NOTIFICATIONS`).
pub const SUPPRESS_NOTIFICATIONS: u64 = 1 << 12;
/// What a message starts with to send quietly (677413).
const SILENT: &str = "@silent";

/// The message as saved: a leading `@silent` becomes the flag, and is
/// dropped from the text (`UF`).
pub fn split_silent(content: &str, flags: u64) -> (String, u64) {
    match content.strip_prefix(SILENT).filter(|rest| rest.chars().next().is_none_or(char::is_whitespace)) {
        Some(rest) => (rest.trim().to_owned(), flags | SUPPRESS_NOTIFICATIONS),
        None => (content.to_owned(), flags),
    }
}

/// The text to edit a saved message from: the flag as `@silent` again (`zs`).
pub fn with_silent(content: &str, flags: u64) -> String {
    if flags & SUPPRESS_NOTIFICATIONS == SUPPRESS_NOTIFICATIONS {
        format!("{SILENT} {content}")
    } else {
        content.to_owned()
    }
}

/// Where a scheduled message stands (551640's enum).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Scheduled,
    ScheduledMessagesDisabled,
    UserNotFound,
    UserCannotUseScheduledMessages,
    ChannelNotFound,
    SendFailed,
    /// A number the client's list has no row for either.
    Unknown(u64),
}

impl State {
    fn from_number(n: u64) -> State {
        match n {
            0 => State::Scheduled,
            1 => State::ScheduledMessagesDisabled,
            2 => State::UserNotFound,
            3 => State::UserCannotUseScheduledMessages,
            4 => State::ChannelNotFound,
            5 => State::SendFailed,
            n => State::Unknown(n),
        }
    }

    /// What the row says instead of the time when sending failed (`Y8`).
    pub fn failure(self) -> Option<&'static str> {
        match self {
            State::Scheduled => None,
            State::UserCannotUseScheduledMessages => Some("Message could not be sent, permission was denied"),
            _ => Some("Message could not be sent"),
        }
    }
}

/// One scheduled message, as the list shows it (`Lg`).
#[derive(Clone, Debug, PartialEq)]
pub struct Scheduled {
    pub id: Snowflake,
    /// When it goes out, unix seconds.
    pub send_at: i64,
    pub channel: Snowflake,
    pub content: String,
    pub flags: u64,
    pub state: State,
    /// How many files it carries.
    pub attachments: usize,
    /// The question, for a poll.
    pub poll: Option<String>,
}

impl Scheduled {
    /// `None` for a row without an id or a channel, which the client's list
    /// couldn't draw either.
    pub fn parse(value: &Value) -> Option<Scheduled> {
        let args = &value["create_args"];
        Some(Scheduled {
            id: value["scheduled_message_id"].as_str()?.parse().ok().map(Snowflake)?,
            send_at: value["send_at_timestamp"]
                .as_str()
                .and_then(|t| glib::DateTime::from_iso8601(t, None).ok())
                .map_or(0, |t| t.to_unix()),
            channel: args["channel_id"].as_str()?.parse().ok().map(Snowflake)?,
            content: args["content"].as_str().unwrap_or_default().to_owned(),
            flags: args["flags"].as_u64().unwrap_or(0),
            state: State::from_number(value["state"].as_u64().unwrap_or(0)),
            attachments: value["attachment_uploads"].as_array().map_or(0, Vec::len),
            poll: value["message_preview"]["poll"]["question"]["text"].as_str().map(str::to_owned),
        })
    }

    /// The list: the furthest-off message first, as the client sorts it.
    pub fn parse_list(value: &Value) -> Vec<Scheduled> {
        let mut list: Vec<Scheduled> = value.as_array().into_iter().flatten().filter_map(Scheduled::parse).collect();
        list.sort_by_key(|s| std::cmp::Reverse(s.send_at));
        list
    }

    /// What editing its text starts from.
    pub fn editable_text(&self) -> String {
        with_silent(&self.content, self.flags)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn utc() -> glib::TimeZone {
        glib::TimeZone::utc()
    }

    /// 2027-01-15 is a Friday.
    fn at(y: i32, m: i32, d: i32, h: i32, min: i32, tz: &glib::TimeZone) -> i64 {
        glib::DateTime::new(tz, y, m, d, h, min, 0.0).unwrap().to_unix()
    }

    #[test]
    fn a_time_is_at_least_ten_minutes_and_at_most_eight_days_ahead() {
        assert_eq!(check(NOW + 599, NOW, None), Err(Refusal::TooSoon));
        assert_eq!(check(NOW + 600, NOW, None), Ok(()));
        assert_eq!(check(NOW + 691_200, NOW, None), Ok(()));
        assert_eq!(check(NOW + 691_201, NOW, None), Err(Refusal::TooFar));
        assert_eq!(Refusal::TooSoon.text(), "Scheduled time is too soon");
        assert_eq!(Refusal::TooFar.text(), "Scheduled time is too far away");
    }

    #[test]
    fn a_waiting_message_moves_within_thirty_days_of_its_making() {
        // Made 29 days and 23 hours ago: only an hour of its thirty days is left.
        let made = Snowflake(((NOW - 29 * 86_400 - 23 * 3600) as u64 * 1000 - 1_420_070_400_000) << 22);
        assert_eq!(check(NOW + 3600, NOW, Some(made)), Ok(()));
        assert_eq!(check(NOW + 3601, NOW, Some(made)), Err(Refusal::TooFar));
        assert_eq!(latest(NOW, Some(made)), NOW + 3600);
        assert_eq!(latest(NOW, None), NOW + 691_200);
        // A fresh one still has the eight days.
        let fresh = Snowflake(((NOW as u64) * 1000 - 1_420_070_400_000) << 22);
        assert_eq!(check(NOW + 691_200, NOW, Some(fresh)), Ok(()));
    }

    #[test]
    fn presets_skip_to_tomorrow_when_the_hour_is_too_close() {
        let tz = utc();
        // Friday 2027-01-15, 08:50: 9:00 is ten minutes off, which is not past them.
        let now = at(2027, 1, 15, 8, 50, &tz);
        let list = presets(now, &tz);
        assert_eq!(list[0], Preset { label: "Tomorrow at 9:00am", at: at(2027, 1, 16, 9, 0, &tz) });
        assert_eq!(list[1], Preset { label: "Today at 1:00pm", at: at(2027, 1, 15, 13, 0, &tz) });
        // 08:49 leaves 9:00 ahead of the ten minutes.
        let now = at(2027, 1, 15, 8, 49, &tz);
        assert_eq!(presets(now, &tz)[0], Preset { label: "Today at 9:00am", at: at(2027, 1, 15, 9, 0, &tz) });
        // After 1pm both are tomorrow's.
        let now = at(2027, 1, 15, 14, 0, &tz);
        let list = presets(now, &tz);
        assert_eq!(list[0].label, "Tomorrow at 9:00am");
        assert_eq!(list[1], Preset { label: "Tomorrow at 1:00pm", at: at(2027, 1, 16, 13, 0, &tz) });
    }

    #[test]
    fn the_third_preset_is_next_mondays_morning() {
        let tz = utc();
        let monday = Preset { label: "Monday at 9:00am", at: at(2027, 1, 18, 9, 0, &tz) };
        assert_eq!(presets(at(2027, 1, 15, 8, 0, &tz), &tz)[2], monday, "from a Friday");
        assert_eq!(presets(at(2027, 1, 11, 8, 0, &tz), &tz)[2], monday, "from the Monday before it");
        assert_eq!(presets(at(2027, 1, 17, 23, 0, &tz), &tz)[2], monday, "from the Sunday before it");
        let after = Preset { label: "Monday at 9:00am", at: at(2027, 1, 25, 9, 0, &tz) };
        assert_eq!(presets(at(2027, 1, 18, 8, 0, &tz), &tz)[2], after, "a Monday's next is a week on");
    }

    #[test]
    fn presets_follow_the_local_clock() {
        let tz = glib::TimeZone::from_identifier(Some("+02:00")).unwrap();
        let now = at(2027, 1, 15, 8, 0, &tz);
        assert_eq!(presets(now, &tz)[0].at, at(2027, 1, 15, 9, 0, &tz));
        assert_eq!(presets(now, &tz)[0].at, at(2027, 1, 15, 7, 0, &utc()));
    }

    #[test]
    fn the_picker_starts_at_the_next_full_hour_ten_minutes_ahead() {
        let tz = utc();
        assert_eq!(default_time(at(2027, 1, 15, 8, 20, &tz), &tz), at(2027, 1, 15, 9, 0, &tz));
        // Ten minutes ahead exactly is not before it, but a minute less is.
        assert_eq!(default_time(at(2027, 1, 15, 8, 50, &tz), &tz), at(2027, 1, 15, 9, 0, &tz));
        assert_eq!(default_time(at(2027, 1, 15, 8, 51, &tz), &tz), at(2027, 1, 15, 10, 0, &tz), "9:00 is too close");
        // Half an hour off UTC: the hour is the local one.
        let india = glib::TimeZone::from_identifier(Some("+05:30")).unwrap();
        assert_eq!(default_time(at(2027, 1, 15, 8, 20, &india), &india), at(2027, 1, 15, 9, 0, &india));
    }

    #[test]
    fn the_time_is_sent_as_javascript_writes_it() {
        assert_eq!(iso(at(2027, 1, 15, 9, 5, &utc())), "2027-01-15T09:05:00.000Z");
        let tz = glib::TimeZone::from_identifier(Some("+02:00")).unwrap();
        assert_eq!(iso(at(2027, 1, 15, 9, 5, &tz)), "2027-01-15T07:05:00.000Z", "always UTC");
    }

    #[test]
    fn silent_is_a_flag_in_the_saved_message() {
        assert_eq!(split_silent("@silent hi", 0), ("hi".to_owned(), SUPPRESS_NOTIFICATIONS));
        assert_eq!(split_silent("@silent", 4), (String::new(), 4 | SUPPRESS_NOTIFICATIONS));
        assert_eq!(split_silent("@silentx hi", 0), ("@silentx hi".to_owned(), 0), "only a word of its own");
        assert_eq!(split_silent("hi @silent", 0), ("hi @silent".to_owned(), 0));
        assert_eq!(with_silent("hi", SUPPRESS_NOTIFICATIONS | 64), "@silent hi");
        assert_eq!(with_silent("hi", 64), "hi");
    }

    #[test]
    fn states_say_why_a_message_failed() {
        assert_eq!(State::from_number(0).failure(), None);
        assert_eq!(State::from_number(3).failure(), Some("Message could not be sent, permission was denied"));
        for n in [1, 2, 4, 5, 9] {
            assert_eq!(State::from_number(n).failure(), Some("Message could not be sent"), "{n}");
        }
        assert_eq!(State::from_number(9), State::Unknown(9));
    }

    #[test]
    fn the_list_has_the_furthest_off_first() {
        let list = json!([
            {
                "user_id": "1",
                "scheduled_message_id": "30",
                "send_at_timestamp": "2027-01-15T09:00:00.000Z",
                "create_args": { "channel_id": "5", "content": "soon", "type": 0, "flags": 4096 },
                "state": 0,
                "attachment_uploads": [{ "filename": "a.png", "uploaded_filename": "u/a.png" }],
                "message_preview": { "content": "soon" }
            },
            {
                "scheduled_message_id": "31",
                "send_at_timestamp": "2027-01-17T09:00:00+00:00",
                "create_args": { "channel_id": "6", "content": "", "flags": 0 },
                "state": 4,
                "message_preview": { "poll": { "question": { "text": "Which?" } } }
            },
            { "scheduled_message_id": "32", "create_args": {} }
        ]);
        let list = Scheduled::parse_list(&list);
        assert_eq!(list.len(), 2, "a row without a channel isn't one");
        let (poll, text) = (&list[0], &list[1]);
        assert_eq!((poll.id, poll.channel), (Snowflake(31), Snowflake(6)));
        assert_eq!(poll.poll.as_deref(), Some("Which?"));
        assert_eq!(poll.state, State::ChannelNotFound);
        assert_eq!(poll.send_at, at(2027, 1, 17, 9, 0, &utc()));
        assert_eq!((text.id, text.attachments, text.poll.as_deref()), (Snowflake(30), 1, None));
        assert_eq!(text.send_at, at(2027, 1, 15, 9, 0, &utc()));
        assert_eq!(text.editable_text(), "@silent soon");
    }
}
