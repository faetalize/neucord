//! Stage instances: the live session of a stage channel and the topic it
//! was started with, as the client's `StageInstanceStore` keeps them (446600).
//! A stage with an instance is live; the sidebar shows its topic under the
//! channel.

use std::collections::HashMap;

use serde_json::Value;

use crate::Snowflake;
use crate::voice_states::VoiceState;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageInstance {
    pub id: Snowflake,
    pub guild_id: Snowflake,
    pub channel_id: Snowflake,
    pub topic: String,
}

impl StageInstance {
    /// One instance of `stage_instances`, or of a `STAGE_INSTANCE_*` event.
    pub fn parse(value: &Value) -> Option<Self> {
        let id = |key: &str| value[key].as_str()?.parse().ok().map(Snowflake);
        Some(StageInstance {
            id: id("id")?,
            guild_id: id("guild_id")?,
            channel_id: id("channel_id")?,
            topic: value["topic"].as_str().unwrap_or_default().to_owned(),
        })
    }

    /// A server's `stage_instances`, none if it has no such list.
    pub fn parse_list(value: &Value) -> Vec<Self> {
        value.as_array().into_iter().flatten().filter_map(Self::parse).collect()
    }
}

/// The live stages, by channel.
#[derive(Clone, Debug, Default)]
pub struct StageInstances {
    by_channel: HashMap<Snowflake, StageInstance>,
}

impl StageInstances {
    /// What `guild` has live, in place of what was known of it (`GUILD_CREATE`
    /// and each server READY carries).
    pub fn replace_guild(&mut self, guild: Snowflake, instances: Vec<StageInstance>) {
        self.forget_guild(guild);
        self.by_channel.extend(instances.into_iter().map(|i| (i.channel_id, i)));
    }

    /// `STAGE_INSTANCE_CREATE` and `STAGE_INSTANCE_UPDATE`.
    pub fn set(&mut self, instance: StageInstance) {
        self.by_channel.insert(instance.channel_id, instance);
    }

    /// `STAGE_INSTANCE_DELETE`, and the channel going away.
    pub fn remove(&mut self, channel: Snowflake) {
        self.by_channel.remove(&channel);
    }

    /// A server was left or lost.
    pub fn forget_guild(&mut self, guild: Snowflake) {
        self.by_channel.retain(|_, instance| instance.guild_id != guild);
    }

    pub fn of(&self, channel: Snowflake) -> Option<&StageInstance> {
        self.by_channel.get(&channel)
    }

    pub fn is_live(&self, channel: Snowflake) -> bool {
        self.by_channel.contains_key(&channel)
    }
}

/// Who is in a stage, as its row in the sidebar tells it (`Pb` in 410828):
/// the speakers, listed, and how many are in the audience.
#[derive(Debug, PartialEq)]
pub struct Participants<'a> {
    pub speakers: Vec<&'a VoiceState>,
    pub audience: usize,
}

impl<'a> Participants<'a> {
    /// `in_channel` is the stage's voice states in the order the list shows
    /// them. A suppressed one is in the audience.
    pub fn of(in_channel: Vec<&'a VoiceState>) -> Self {
        let (audience, speakers): (Vec<_>, Vec<_>) = in_channel.into_iter().partition(|s| s.suppress);
        Participants { speakers, audience: audience.len() }
    }

    /// Nobody to show.
    pub fn is_empty(&self) -> bool {
        self.speakers.is_empty() && self.audience == 0
    }
}

/// The count of speakers as the row says it (`chmM9N`): a plain count, and
/// for none the line the client's catalog has, which reads "0 is speaking".
pub fn speaking_label(speakers: usize) -> String {
    if speakers == 0 { "0 is speaking".to_owned() } else { format!("{speakers} speaking") }
}

/// The count of the audience (`+v2pN2`).
pub fn audience_label(audience: usize) -> String {
    format!("{audience} in audience")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn instance(id: &str, guild: &str, channel: &str, topic: &str) -> Value {
        json!({"id": id, "guild_id": guild, "channel_id": channel, "topic": topic, "privacy_level": 2})
    }

    #[test]
    fn an_instance_names_its_channel_and_topic() {
        let parsed = StageInstance::parse(&instance("1", "2", "3", "Q&A")).unwrap();
        assert_eq!((parsed.id.0, parsed.guild_id.0, parsed.channel_id.0, parsed.topic.as_str()), (1, 2, 3, "Q&A"));
        assert!(StageInstance::parse(&json!({"id": "1", "topic": "x"})).is_none(), "no channel, no stage");
        assert!(StageInstance::parse_list(&Value::Null).is_empty(), "a server without the list has none");
        let both = json!([instance("1", "2", "3", "a"), {"nonsense": true}, instance("4", "2", "5", "")]);
        assert_eq!(StageInstance::parse_list(&both).len(), 2);
    }

    #[test]
    fn a_stage_is_live_from_its_create_to_its_delete() {
        let mut stages = StageInstances::default();
        let at = |s: &StageInstances| s.of(Snowflake(3)).map(|i| i.topic.clone());
        assert!(!stages.is_live(Snowflake(3)));
        stages.set(StageInstance::parse(&instance("1", "2", "3", "first")).unwrap());
        assert!(stages.is_live(Snowflake(3)));
        stages.set(StageInstance::parse(&instance("1", "2", "3", "second")).unwrap());
        assert_eq!(at(&stages).as_deref(), Some("second"), "an update replaces the topic");
        stages.remove(Snowflake(3));
        assert_eq!(at(&stages), None);
    }

    #[test]
    fn the_stage_row_lists_the_speakers_and_counts_the_audience() {
        let state = |user: u64, suppress: bool| VoiceState {
            user_id: Snowflake(user),
            channel_id: Snowflake(3),
            session_id: String::new(),
            mute: false,
            deaf: false,
            self_mute: false,
            self_deaf: false,
            self_video: false,
            self_stream: false,
            suppress,
            nick: None,
            user: None,
        };
        let all = [state(1, false), state(2, true), state(3, true), state(4, false)];
        let participants = Participants::of(all.iter().collect());
        assert_eq!(participants.speakers.iter().map(|s| s.user_id.0).collect::<Vec<_>>(), [1, 4], "in their order");
        assert_eq!(participants.audience, 2);
        assert!(!participants.is_empty() && Participants::of(Vec::new()).is_empty());
        assert_eq!(
            (speaking_label(0), speaking_label(1), speaking_label(4)),
            ("0 is speaking".into(), "1 speaking".into(), "4 speaking".into())
        );
        assert_eq!(audience_label(2), "2 in audience");
    }

    #[test]
    fn a_servers_list_replaces_what_was_known_of_it_and_leaving_drops_it() {
        let mut stages = StageInstances::default();
        let list = |channel: &str| vec![StageInstance::parse(&instance("1", "2", channel, "t")).unwrap()];
        stages.replace_guild(Snowflake(2), list("3"));
        stages.replace_guild(Snowflake(9), vec![StageInstance::parse(&instance("7", "9", "8", "o")).unwrap()]);
        stages.replace_guild(Snowflake(2), list("4"));
        assert!(!stages.is_live(Snowflake(3)) && stages.is_live(Snowflake(4)), "the old list is gone");
        assert!(stages.is_live(Snowflake(8)), "another server's stays");
        stages.forget_guild(Snowflake(2));
        assert!(!stages.is_live(Snowflake(4)) && stages.is_live(Snowflake(8)));
    }
}
