//! Pressing a message's buttons and picking from its menus, as the official
//! client does: what it sends (`sendComponentInteraction` in 964363), what a
//! message shows until Discord answers (`InteractionStore`, 436283), and how a
//! refusal reads (`T` in 964363).

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::Snowflake;
use crate::component::Component;
use crate::model::{Message, User};

/// `MESSAGE_COMPONENT` in 127258's interaction types.
const MESSAGE_COMPONENT: u8 = 3;
/// `STRING_SELECT` in 155718's component types.
pub const STRING_SELECT: u8 = 3;
pub const BUTTON: u8 = 2;

/// ApiErrorCode values the client tells apart in a refusal (652215).
const INVALID_FORM_BODY: u64 = 50035;
const UNKNOWN_INTEGRATION: u64 = 10005;
/// The form errors that say a command is out of date.
const COMMAND_OUT_OF_DATE: [&str; 2] =
    ["INTERACTION_APPLICATION_COMMAND_INVALID_VERSION", "INTERACTION_APPLICATION_COMMAND_INVALID"];

/// Why an answer is only ours to see, as the footer under it says it
/// (`ephemerality_reason` in 376767). A reason it doesn't know, and `NONE`,
/// read as an unknown error.
pub fn ephemerality_text(reason: u64) -> &'static str {
    match reason {
        1 => "This feature is temporarily limited",
        2 => "This feature is temporarily limited for this server",
        3 => "This feature is temporarily limited for you",
        4 => "You are sending messages too quickly",
        5 => "You are being rate limited",
        6 => "You cannot message this user",
        7 => "You do not meet this server's verification level",
        8 => "You cannot unarchive this thread",
        9 => "You cannot join this thread",
        10 => "You cannot send messages in this channel",
        11 => "You cannot send attachments in this channel",
        12 => "You cannot send embeds in this channel",
        13 => "You cannot send stickers in this channel",
        14 => "This message was blocked by AutoMod. The message may also be viewed by the community owners.",
        15 => "This message contains a link blocked by Discord.",
        16 => "You cannot use this command in this channel",
        17 => "Only you can see this message during beta",
        18 => "You cannot use external apps in this channel",
        _ => "An unknown error occurred",
    }
}

/// The interaction a message is waiting on.
#[derive(Clone, Debug, PartialEq)]
pub struct Pending {
    pub nonce: Snowflake,
    /// The `custom_id` of what was pressed.
    pub component: String,
}

/// What we know of a message's components that Discord doesn't: the
/// interaction on its way and what was picked in its menus. Kept with the
/// message, so a change redraws it and a held channel keeps it.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Local {
    pending: Option<Pending>,
    /// What each menu was last given, by `custom_id`, until the message
    /// changes (the client's `LocalInteractionComponentStateStore`).
    picked: BTreeMap<String, Vec<String>>,
}

/// How a button or menu is drawn while an interaction is on its way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visual {
    Normal,
    /// The one that was pressed.
    Loading,
    /// The others, which wait their turn.
    Disabled,
}

impl Local {
    /// Whether another interaction may be sent: one at a time per message
    /// (`canQueueInteraction`). A refused one no longer counts.
    pub fn can_queue(&self) -> bool {
        self.pending.is_none()
    }

    /// An interaction is sent. A menu keeps what was picked.
    pub fn queue(&mut self, nonce: Snowflake, component: &str, picked: Option<&[String]>) {
        self.pending = Some(Pending { nonce, component: component.to_owned() });
        if let Some(values) = picked {
            self.picked.insert(component.to_owned(), values.to_vec());
        }
    }

    /// Discord answered: the app responded, or showed a form. What was picked
    /// stays until the message changes.
    pub fn answered(&mut self, nonce: Snowflake) -> bool {
        self.pending.take_if(|p| p.nonce == nonce).is_some()
    }

    /// Discord or the request refused. A menu goes back to what the message says.
    pub fn refused(&mut self, nonce: Snowflake) -> bool {
        match self.pending.take_if(|p| p.nonce == nonce) {
            Some(pending) => {
                self.picked.remove(&pending.component);
                true
            }
            None => false,
        }
    }

    /// The message changed or was edited: menus show what it says again.
    pub fn forget_picks(&mut self) {
        self.picked.clear();
    }

    /// What a menu was last given, if anything.
    pub fn picked(&self, component: &str) -> Option<&[String]> {
        self.picked.get(component).map(Vec::as_slice)
    }

    /// The pending interaction, if any.
    pub fn pending(&self) -> Option<&Pending> {
        self.pending.as_ref()
    }

    /// How `component` is drawn (`visualState` in 911408).
    pub fn visual(&self, component: &Component) -> Visual {
        let id = match component {
            Component::Button(button) => button.custom_id.as_deref(),
            Component::Select { custom_id, .. } => custom_id.as_deref(),
            _ => None,
        };
        match &self.pending {
            Some(pending) if id == Some(&pending.component) => Visual::Loading,
            Some(_) if component.is_interactive() => Visual::Disabled,
            _ => Visual::Normal,
        }
    }
}

/// What is wrong with picking `count` options from a menu that takes `min` to
/// `max` (the select validator in 911408): picking none is fine where
/// `empty_ok` (`Lr` in 257249: a message's menu whose `min` is 0, a form's
/// that isn't required), and a missing limit isn't checked.
pub fn pick_error(count: usize, empty_ok: bool, min: Option<usize>, max: Option<usize>) -> Option<String> {
    match count {
        0 if empty_ok => None,
        0 => Some("This field is required.".into()),
        n if min.is_some_and(|min| n < min) => Some(format!("Select at least {}", min.unwrap_or_default())),
        n if max.is_some_and(|max| n > max) => Some(format!("Select at most {}", max.unwrap_or_default())),
        _ => None,
    }
}

/// What a press sends.
pub struct Press<'a> {
    pub guild: Option<Snowflake>,
    pub channel: Snowflake,
    pub message: Snowflake,
    pub message_flags: u64,
    /// The app that made the message: its `application_id`, else its author.
    pub application: Snowflake,
    pub session: &'a str,
    pub component_type: u8,
    pub custom_id: &'a str,
    /// What a menu was given.
    pub values: Option<&'a [String]>,
}

impl Press<'_> {
    /// The request body (`S` in 964363). A menu's data also carries its type
    /// and values, since the client spreads its state into it.
    pub fn body(&self, nonce: Snowflake) -> Value {
        let mut data = json!({ "component_type": self.component_type, "custom_id": self.custom_id });
        if let Some(values) = self.values {
            data["type"] = json!(STRING_SELECT);
            data["values"] = json!(values);
        }
        json!({
            "type": MESSAGE_COMPONENT,
            "nonce": nonce.to_string(),
            "guild_id": self.guild,
            "channel_id": self.channel,
            "message_flags": self.message_flags,
            "message_id": self.message,
            "application_id": self.application,
            "session_id": self.session,
            "data": data,
        })
    }
}

/// Why an interaction failed, as the client takes it from the response.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Failure {
    pub code: Option<u64>,
    pub message: Option<String>,
    /// Discord says the command (or the app) isn't what we have: the index
    /// it came from is out of date (`APPLICATION_COMMAND_EXECUTE_BAD_VERSION`).
    pub stale_command: bool,
}

impl Failure {
    /// Whether the client says so in the chat: a code with no message of
    /// Discord's. Otherwise the component just goes back to normal.
    pub fn is_announced(&self) -> bool {
        self.code.is_some() && self.message.is_none()
    }

    /// From a response that wasn't a success (`T` in 964363).
    pub fn from_response(status: u32, body: &[u8]) -> Failure {
        let body: Option<Value> = serde_json::from_slice(body).ok().filter(|v: &Value| v.is_object());
        let code = body.as_ref().and_then(|b| b["code"].as_u64());
        let message = body.as_ref().and_then(|b| b["message"].as_str().map(String::from));
        let Some(body) = body.filter(|_| (400..500).contains(&status)) else {
            return Failure { code, ..Failure::default() };
        };
        if code == Some(INVALID_FORM_BODY) && body.get("errors").is_some_and(|e| !e.is_null()) {
            let first = first_error(&body["errors"]);
            let stale = first.and_then(|e| e["code"].as_str()).is_some_and(|c| COMMAND_OUT_OF_DATE.contains(&c));
            let message = first.and_then(|e| e["message"].as_str()).map(String::from);
            return Failure { code: None, message, stale_command: stale };
        }
        match code {
            Some(UNKNOWN_INTEGRATION) => Failure { code: None, message, stale_command: true },
            _ => Failure { code, message, stale_command: false },
        }
    }
}

/// The first error: one in this level's `_errors`, else the first nested
/// level that has one (the client's `e` in `T`).
fn first_error(errors: &Value) -> Option<&Value> {
    if let Some(first) = errors["_errors"].as_array().and_then(|list| list.first()) {
        return Some(first);
    }
    errors
        .as_object()?
        .iter()
        .filter(|(k, _)| *k != "_errors")
        .filter(|(_, v)| v.is_object())
        .find_map(|(_, v)| first_error(v))
}

/// `APPLICATION_COMMAND` in 127258's interaction types.
const APPLICATION_COMMAND: u8 = 2;
/// `PRIMARY_ENTRY_POINT` in 155718's command types: an app's launch command.
const PRIMARY_ENTRY_POINT: u8 = 4;
/// `INTERACTION_PREMIUM_UPSELL` in 448761's message types.
const PREMIUM_UPSELL: u8 = 26;

/// The line above an app's answer: who ran which command, on what
/// (`ExecutedCommand`, 267508).
#[derive(Debug, PartialEq)]
pub struct Executed<'a> {
    /// Who ran it (`interaction.user`).
    pub user: &'a User,
    pub what: Ran,
    pub target: Option<Target<'a>>,
}

/// What the line says was run.
#[derive(Debug, PartialEq)]
pub enum Ran {
    /// "used" a slash command, after the apps icon. The client opens the
    /// run's options from it.
    Slash(String),
    /// "used" an app's launch command, after the apps icon, its name as
    /// `kF` (735991) writes it. The client opens the launcher from it.
    EntryPoint(String),
    /// "used" a user or message command, plainly.
    Plain(String),
    /// "started an activity".
    Activity,
}

/// What a user or message command was run on, after a chevron.
#[derive(Debug, PartialEq)]
pub enum Target<'a> {
    /// A message command's message, which the message references.
    Message(Snowflake),
    User(&'a User),
}

/// The line above `message`, if it answers a command with a name. The
/// target comes from a modal submit's triggering interaction when there
/// is one (`Am` in 168186); a message command's message wins over a user.
pub fn executed(message: &Message) -> Option<Executed<'_>> {
    let interaction = message.interaction.as_ref()?;
    let name = interaction.display_name();
    if name.is_empty() {
        return None;
    }
    let outer = message.interaction_metadata.as_ref();
    // `V` in 597929 reads the message's own metadata, not the triggering one.
    let entry = outer.is_some_and(|m| m.kind == APPLICATION_COMMAND && m.command_type == Some(PRIMARY_ENTRY_POINT));
    if message.activity_instance.is_some() && !entry {
        return Some(Executed { user: &interaction.user, what: Ran::Activity, target: None });
    }
    let what = if matches!(message.kind, Message::CHAT_INPUT_COMMAND | PREMIUM_UPSELL) {
        Ran::Slash(name.to_owned())
    } else if entry {
        Ran::EntryPoint(launch_name(name))
    } else {
        Ran::Plain(name.to_owned())
    };
    let meta = outer.and_then(|m| m.triggering_interaction_metadata.as_deref().or(Some(m)));
    let target = meta.filter(|m| m.kind == APPLICATION_COMMAND).and_then(|m| {
        match message.message_reference.as_ref().and_then(|r| r.message_id) {
            Some(id) => Some(Target::Message(id)),
            None => m.target_user.as_ref().map(Target::User),
        }
    });
    Some(Executed { user: &interaction.user, what, target })
}

/// `kF` in 735991: the first letter upper-cased, underscores as spaces.
fn launch_name(name: &str) -> String {
    let mut chars = name.chars();
    let first = chars.next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    (first + chars.as_str()).replace('_', " ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::{Button, ButtonStyle};
    use serde_json::json;

    fn button(id: &str, style: ButtonStyle) -> Component {
        Component::Button(Button {
            custom_id: Some(id.into()),
            style,
            label: None,
            emoji: None,
            url: None,
            disabled: false,
        })
    }

    #[test]
    fn one_interaction_at_a_time_and_the_rest_wait() {
        let mut local = Local::default();
        let (a, b, link) =
            (button("a", ButtonStyle::Primary), button("b", ButtonStyle::Secondary), button("l", ButtonStyle::Link));
        assert!(local.can_queue() && local.visual(&a) == Visual::Normal);
        local.queue(Snowflake(7), "a", None);
        assert!(!local.can_queue());
        assert_eq!(
            (local.visual(&a), local.visual(&b), local.visual(&link)),
            (Visual::Loading, Visual::Disabled, Visual::Normal)
        );
        assert!(!local.answered(Snowflake(8)), "someone else's nonce");
        assert!(local.answered(Snowflake(7)));
        assert!(local.can_queue() && local.visual(&b) == Visual::Normal);
    }

    #[test]
    fn a_refused_menu_goes_back_but_an_answered_one_keeps_its_picks() {
        let mut local = Local::default();
        let picks = ["x".to_owned()];
        local.queue(Snowflake(1), "m", Some(&picks));
        assert_eq!(local.picked("m"), Some(&picks[..]));
        local.answered(Snowflake(1));
        assert_eq!(local.picked("m"), Some(&picks[..]), "until the message changes");
        local.queue(Snowflake(2), "m", Some(&picks));
        assert!(local.refused(Snowflake(2)));
        assert_eq!(local.picked("m"), None);
        local.queue(Snowflake(3), "m", Some(&picks));
        local.answered(Snowflake(3));
        local.forget_picks();
        assert_eq!(local.picked("m"), None);
    }

    fn press(values: Option<&[String]>, guild: Option<u64>) -> Press<'_> {
        Press {
            guild: guild.map(Snowflake),
            channel: Snowflake(2),
            message: Snowflake(3),
            message_flags: 64,
            application: Snowflake(4),
            session: "s",
            component_type: if values.is_some() { STRING_SELECT } else { BUTTON },
            custom_id: "c",
            values,
        }
    }

    #[test]
    fn a_button_press_sends_what_the_client_sends() {
        let body = press(None, Some(1)).body(Snowflake(9));
        assert_eq!(
            body,
            json!({"type": 3, "nonce": "9", "guild_id": "1", "channel_id": "2", "message_flags": 64,
                   "message_id": "3", "application_id": "4", "session_id": "s",
                   "data": {"component_type": 2, "custom_id": "c"}})
        );
        assert!(
            press(None, None).body(Snowflake(9))["guild_id"].is_null(),
            "a DM's channel has none: null, not absent"
        );
    }

    #[test]
    fn a_menu_sends_its_type_and_values_in_its_data() {
        let values = ["a".to_owned(), "b".to_owned()];
        let data = press(Some(&values), Some(1)).body(Snowflake(9))["data"].clone();
        assert_eq!(data, json!({"component_type": 3, "custom_id": "c", "type": 3, "values": ["a", "b"]}));
    }

    #[test]
    fn picks_are_checked_against_the_menus_limits() {
        let message = |count, min: usize, max| pick_error(count, min == 0, Some(min), Some(max));
        assert_eq!(message(1, 1, 1), None);
        assert_eq!(message(0, 0, 3), None, "a menu that takes none");
        assert_eq!(message(0, 1, 3).as_deref(), Some("This field is required."));
        assert_eq!(message(1, 2, 3).as_deref(), Some("Select at least 2"));
        assert_eq!(message(4, 1, 3).as_deref(), Some("Select at most 3"));
    }

    fn failure(status: u32, body: &str) -> Failure {
        Failure::from_response(status, body.as_bytes())
    }

    #[test]
    fn a_refusal_reads_as_the_client_reads_it() {
        // Server trouble, or no body: only a code makes the client say anything.
        assert_eq!(failure(500, ""), Failure::default());
        assert_eq!(failure(502, r#"{"code": 0}"#), Failure { code: Some(0), ..Failure::default() });
        assert!(failure(500, r#"{"code": 7}"#).is_announced() && !failure(500, "").is_announced());
        // A form error: the first message in the errors.
        let nested = r#"{"code": 50035, "message": "Invalid Form Body", "errors": {"data": {"custom_id":
            {"_errors": [{"code": "X", "message": "Unknown component"}]}}}}"#;
        assert_eq!(failure(400, nested), Failure { message: Some("Unknown component".into()), ..Failure::default() });
        let top =
            r#"{"code": 50035, "errors": {"_errors": [{"message": "bad"}], "data": {"_errors": [{"message": "no"}]}}}"#;
        assert_eq!(failure(400, top).message.as_deref(), Some("bad"), "this level before the ones inside");
        // Any other 4xx keeps the code and says why.
        assert_eq!(
            failure(403, r#"{"code": 50013, "message": "Missing Permissions"}"#),
            Failure { code: Some(50013), message: Some("Missing Permissions".into()), ..Failure::default() }
        );
        assert!(!failure(403, r#"{"code": 50013, "message": "Missing Permissions"}"#).is_announced());
        // An app that is no longer installed has no code of its own to show.
        assert_eq!(
            failure(404, r#"{"code": 10005, "message": "Unknown Integration"}"#),
            Failure { message: Some("Unknown Integration".into()), stale_command: true, ..Failure::default() }
        );
    }

    #[test]
    fn an_answer_only_we_see_says_why_as_the_client_does() {
        assert_eq!(ephemerality_text(4), "You are sending messages too quickly");
        assert_eq!(ephemerality_text(18), "You cannot use external apps in this channel");
        // `NONE` is a reason too, with no words of its own.
        assert_eq!(ephemerality_text(0), "An unknown error occurred");
        assert_eq!(ephemerality_text(99), "An unknown error occurred");
        let metadata: crate::model::InteractionMetadata =
            serde_json::from_str(r#"{"user": {"id": "7", "username": "u"}, "ephemerality_reason": 10}"#).unwrap();
        assert_eq!(metadata.ephemerality_reason, Some(10));
    }

    #[test]
    fn a_command_discord_calls_out_of_date_says_to_fetch_the_index_again() {
        let invalid = |code| {
            format!(
                r#"{{"code": 50035, "errors": {{"data": {{"_errors": [{{"code": "{code}", "message": "Command changed"}}]}}}}}}"#
            )
        };
        let stale = failure(400, &invalid("INTERACTION_APPLICATION_COMMAND_INVALID_VERSION"));
        assert!(stale.stale_command && stale.message.as_deref() == Some("Command changed"));
        assert!(failure(400, &invalid("INTERACTION_APPLICATION_COMMAND_INVALID")).stale_command);
        assert!(!failure(400, &invalid("SOMETHING_ELSE")).stale_command);
        assert!(failure(404, r#"{"code": 10005, "message": "Unknown Integration"}"#).stale_command);
        assert!(!failure(403, r#"{"code": 50013, "message": "Missing Permissions"}"#).stale_command);
    }

    fn answer(kind: u8, extra: Value) -> Message {
        let mut message = json!({"id": "3", "channel_id": "9", "type": kind, "timestamp": "",
            "author": {"id": "7", "username": "app", "bot": true},
            "interaction": {"id": "8", "type": 2, "name": "settings set", "user": {"id": "5", "username": "me"}},
            "interaction_metadata": {"id": "8", "type": 2, "user": {"id": "5", "username": "me"}}});
        merge(&mut message, extra);
        serde_json::from_value(message).unwrap()
    }

    fn merge(into: &mut Value, from: Value) {
        match (into, from) {
            (Value::Object(into), Value::Object(from)) => {
                for (key, value) in from {
                    merge(into.entry(key).or_insert(Value::Null), value);
                }
            }
            (into, from) => *into = from,
        }
    }

    fn line(message: &Message) -> Option<(Snowflake, Ran, Option<Snowflake>)> {
        let line = executed(message)?;
        let target = line.target.map(|t| match t {
            Target::Message(id) => id,
            Target::User(user) => user.id,
        });
        Some((line.user.id, line.what, target))
    }

    #[test]
    fn a_slash_commands_answer_says_who_used_it() {
        let slash = |name: &str| Ran::Slash(name.into());
        assert_eq!(line(&answer(20, json!({}))), Some((Snowflake(5), slash("settings set"), None)));
        assert_eq!(line(&answer(26, json!({}))), Some((Snowflake(5), slash("settings set"), None)), "an upsell");
        assert_eq!(line(&answer(20, json!({"flags": 64}))).map(|l| l.1), Some(slash("settings set")), "ephemeral");
        let localized = json!({"interaction": {"name_localized": "réglages"}});
        assert_eq!(line(&answer(20, localized)).map(|l| l.1), Some(slash("réglages")));
    }

    #[test]
    fn no_line_without_a_named_interaction() {
        assert_eq!(line(&answer(20, json!({"interaction": null}))), None);
        assert_eq!(line(&answer(20, json!({"interaction": {"name": ""}}))), None, "a component's answer");
        assert_eq!(line(&answer(20, json!({"interaction": {"name_localized": ""}}))), None, "?? keeps it empty");
    }

    #[test]
    fn a_user_or_message_command_names_what_it_ran_on() {
        let them = json!({"id": "6", "username": "them"});
        let on_user = answer(
            23,
            json!({"interaction": {"name": "Report User"},
                                        "interaction_metadata": {"command_type": 2, "target_user": them}}),
        );
        assert_eq!(line(&on_user), Some((Snowflake(5), Ran::Plain("Report User".into()), Some(Snowflake(6)))));
        let reference = json!({"message_id": "40", "channel_id": "9"});
        let on_message = answer(
            23,
            json!({"interaction": {"name": "Quote Message"}, "message_reference": reference,
                                           "interaction_metadata": {"command_type": 3, "target_user": them}}),
        );
        assert_eq!(line(&on_message).and_then(|l| l.2), Some(Snowflake(40)), "the message wins");
        let component = answer(23, json!({"interaction_metadata": {"type": 3, "target_user": them}}));
        assert_eq!(line(&component).and_then(|l| l.2), None, "only a command has a target");
    }

    #[test]
    fn a_modal_submits_answer_takes_the_target_of_what_opened_it() {
        let them = json!({"id": "6", "username": "them"});
        let submit = answer(
            23,
            json!({"interaction_metadata": {"type": 5, "triggering_interaction_metadata":
            {"id": "9", "type": 2, "user": {"id": "5", "username": "me"}, "target_user": them}}}),
        );
        assert_eq!(line(&submit).and_then(|l| l.2), Some(Snowflake(6)));
        let plain = answer(23, json!({"interaction_metadata": {"type": 5, "target_user": them}}));
        assert_eq!(line(&plain).and_then(|l| l.2), None, "a submit has no target of its own");
    }

    #[test]
    fn a_launch_command_and_an_activity_read_as_the_client_writes_them() {
        let entry = json!({"interaction": {"name": "launch_game"}, "interaction_metadata": {"command_type": 4}});
        assert_eq!(line(&answer(23, entry.clone())).map(|l| l.1), Some(Ran::EntryPoint("Launch game".into())));
        let mut launched = entry;
        merge(&mut launched, json!({"activity_instance": {"id": "a1"}}));
        assert_eq!(line(&answer(23, launched)).map(|l| l.1), Some(Ran::EntryPoint("Launch game".into())));
        let activity = answer(20, json!({"activity_instance": {"id": "a1"}}));
        assert_eq!(line(&activity), Some((Snowflake(5), Ran::Activity, None)));
    }
}
