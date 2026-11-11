//! Saving the account's settings, the way the official client does
//! (`UserSettingsProtoStore`, and the class holding `markDirty` and
//! `persistChanges` in module 594061).
//!
//! The client keeps its copy of `PreloadedUserSettings` and a set of edited
//! messages not saved yet. Changing a setting copies the message it lives in,
//! sets the field, and puts the whole message in both. A timer then sends
//! everything edited as one partial `PreloadedUserSettings` in
//! `PATCH /users/@me/settings-proto/1`; the answer is the account's settings
//! as Discord now has them, which replace the copy. Nothing here touches the
//! network or a timer: [`SettingsSync`] says what to send and when, and its
//! caller does it.
//!
//! A change made while the gateway was down is an offline edit. The client
//! stops its timer when the connection closes and remembers the
//! `data_version` of its copy; it sends the edit after the next READY, with
//! that version as `required_data_version`, so Discord can tell the copy
//! the edit was made against is out of date.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::protobuf::Message;
use crate::settings::SettingsPatch;
use crate::settings_proto::{self, Setting};

/// The longest the client waits to retry after a rate limit, whatever
/// `Retry-After` says.
const MAX_RETRY: Duration = Duration::from_secs(30);
/// How long to wait when `Retry-After` is missing or not a number.
const DEFAULT_RETRY: Duration = Duration::from_secs(60);
/// An offline edit goes out this long after READY, plus up to as much again.
const OFFLINE_DELAY_MS: i32 = 5000;

/// A save the caller should start a timer for. When it fires, hand the
/// generation back to [`SettingsSync::save`]: a timer that has been replaced
/// since finds a different one and does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    pub generation: u64,
    pub after: Duration,
}

/// One `PATCH /users/@me/settings-proto/1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Save {
    /// The partial `PreloadedUserSettings`, base64.
    pub settings: String,
    /// Only for an offline edit.
    pub required_data_version: Option<u64>,
    /// The messages this carries, to tell what was edited since.
    sent: BTreeMap<u32, Vec<u8>>,
}

impl Save {
    #[cfg(test)]
    pub(crate) fn test(settings: &str, required_data_version: Option<u64>) -> Self {
        Save { settings: settings.to_owned(), required_data_version, sent: BTreeMap::new() }
    }

    /// The request's body, `settings` then `required_data_version`.
    pub(crate) fn body(&self) -> serde_json::Value {
        let mut body = serde_json::json!({ "settings": self.settings });
        if let Some(version) = self.required_data_version {
            body["required_data_version"] = version.into();
        }
        body
    }
}

/// What Discord answers to a [`Save`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Saved {
    /// The account's settings now.
    pub settings: Option<Vec<u8>>,
    /// The edit was made against settings that had changed since; Discord
    /// keeps its own, and the answer is those.
    pub out_of_date: bool,
}

/// Why a [`Save`] failed, as far as the client tells them apart.
#[derive(Debug)]
pub enum SaveError {
    /// 429, with the `Retry-After` header if it had one.
    RateLimited {
        retry_after: Option<String>,
    },
    /// 400 with the code `INVALID_USER_SETTINGS_DATA`: Discord took what was
    /// sent for settings that cannot be.
    Invalid,
    Other(crate::Error),
}

/// A change made locally: what to show now, and a save to start if none is
/// waiting.
#[derive(Debug, PartialEq)]
pub struct Edited {
    pub patch: SettingsPatch,
    pub schedule: Option<Schedule>,
}

/// The settings after Discord answered: all of them, with any change made
/// since the save was sent put back on top.
#[derive(Debug, PartialEq)]
pub struct Applied {
    pub patch: SettingsPatch,
    pub schedule: Option<Schedule>,
}

#[derive(Default)]
pub struct SettingsSync {
    /// `PreloadedUserSettings` as the account has it, edits included. Until
    /// READY there is none, and nothing can be edited.
    proto: Option<Vec<u8>>,
    /// The messages edited and not saved yet, by field number (`protoToSave`).
    to_save: BTreeMap<u32, Vec<u8>>,
    /// The generation of the save waiting for its timer.
    timer: Option<u64>,
    rate_limited: bool,
    offline_data_version: Option<u64>,
    generation: u64,
}

impl SettingsSync {
    pub fn new() -> Self {
        Self::default()
    }

    fn schedule(&mut self, after: Duration) -> Schedule {
        self.generation += 1;
        self.timer = Some(self.generation);
        Schedule { generation: self.generation, after }
    }

    /// The gateway closed or resumed: a save waiting for its timer becomes
    /// an offline edit, made against the version we have.
    fn park(&mut self) {
        if self.timer.take().is_some() {
            self.rate_limited = false;
            self.offline_data_version = Some(self.proto.as_deref().map_or(0, settings_proto::data_version));
        }
    }

    pub fn disconnected(&mut self) {
        self.park();
    }

    /// READY carried `proto`. An offline edit is sent a few seconds on.
    pub fn connected(&mut self, proto: Vec<u8>) -> Option<Schedule> {
        self.proto = Some(proto);
        self.park();
        if self.offline_data_version.is_some() && !self.to_save.is_empty() {
            let after = Duration::from_millis((OFFLINE_DELAY_MS + glib::random_int_range(0, OFFLINE_DELAY_MS)) as u64);
            return Some(self.schedule(after));
        }
        None
    }

    /// A USER_SETTINGS_PROTO_UPDATE: the account's settings, or with
    /// `partial` the messages that changed in them. Edits not yet saved are
    /// left alone, and win when they are.
    pub fn received(&mut self, update: &[u8], partial: bool) {
        let Some(proto) = &self.proto else { return };
        if !partial {
            self.proto = Some(update.to_vec());
            return;
        }
        let Some(groups) = Message::parse(update).map(|m| m.nested()) else { return };
        if let Some(merged) = settings_proto::replace_groups(proto, &groups) {
            self.proto = Some(merged);
        }
    }

    /// Change a setting. The client waits no time before saving a setting
    /// the user changed (`INFREQUENT_USER_ACTION`), so a save is due at
    /// once, unless one is waiting to retry after a rate limit.
    pub fn edit(&mut self, setting: Setting) -> Option<Edited> {
        let proto = self.proto.as_deref()?;
        let (group, written) = setting.written(proto)?;
        let merged = settings_proto::replace_groups(proto, &[(group, &written)])?;
        let patch = SettingsPatch::from_proto_bytes(&settings_proto::replace_groups(&[], &[(group, &written)])?)?;
        self.proto = Some(merged);
        self.to_save.insert(group, written);
        let schedule =
            if self.timer.is_some() && self.rate_limited { None } else { Some(self.schedule(Duration::ZERO)) };
        Some(Edited { patch, schedule })
    }

    /// The timer of `generation` fired: what to send, if anything.
    pub fn save(&mut self, generation: u64) -> Option<Save> {
        if self.timer != Some(generation) {
            return None;
        }
        self.timer = None;
        self.rate_limited = false;
        if self.to_save.is_empty() {
            return None;
        }
        let groups: Vec<(u32, &[u8])> = self.to_save.iter().map(|(n, g)| (*n, g.as_slice())).collect();
        let partial = settings_proto::replace_groups(&[], &groups)?;
        Some(Save {
            settings: settings_proto::encode(&partial),
            required_data_version: self.offline_data_version,
            sent: self.to_save.clone(),
        })
    }

    /// Discord took `save` and answered with `saved`. Its settings replace
    /// ours, and a change made while it was on its way is put back and
    /// saved next, which the client's copy would lose.
    pub fn saved(&mut self, save: &Save, saved: &Saved) -> Option<Applied> {
        self.offline_data_version = None;
        self.rate_limited = false;
        self.to_save.retain(|number, group| save.sent.get(number) != Some(group));
        let proto = saved.settings.as_deref()?;
        let groups: Vec<(u32, &[u8])> = self.to_save.iter().map(|(n, g)| (*n, g.as_slice())).collect();
        let proto = settings_proto::replace_groups(proto, &groups)?;
        let patch = SettingsPatch::from_proto_bytes(&proto)?;
        self.proto = Some(proto);
        let schedule = if self.to_save.is_empty() { None } else { Some(self.schedule(Duration::ZERO)) };
        Some(Applied { patch, schedule })
    }

    /// Discord is limiting us: try again after `Retry-After`, at most 30
    /// seconds.
    pub fn rate_limited(&mut self, retry_after: Option<&str>) -> Schedule {
        let wait = retry_after.and_then(|v| v.trim().parse::<u64>().ok()).map_or(DEFAULT_RETRY, Duration::from_secs);
        self.rate_limited = true;
        self.schedule(wait.min(MAX_RETRY))
    }

    /// The save failed some other way. What was edited stays, and goes with
    /// the next change.
    pub fn failed(&mut self) {
        self.rate_limited = false;
    }

    /// Discord refused the settings as invalid; `proto` is what it holds.
    /// Edits not saved are dropped.
    pub fn reloaded(&mut self, proto: Vec<u8>) -> Option<SettingsPatch> {
        self.to_save.clear();
        self.timer = None;
        self.rate_limited = false;
        self.offline_data_version = None;
        let patch = SettingsPatch::from_proto_bytes(&proto);
        self.proto = Some(proto);
        patch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protobuf::Value;
    use crate::protobuf::tests::{bool_wrapper, bytes_field, varint_field};

    const TEXT_AND_IMAGES: u32 = 6;
    const GIF_AUTO_PLAY: u32 = 11;
    const RENDER_EMBEDS: u32 = 12;

    fn partial(groups: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut proto = Vec::new();
        for (number, group) in groups {
            proto = Message::parse(&proto).unwrap().with(*number, Some(Value::Bytes(group.clone())));
        }
        proto
    }

    fn account() -> Vec<u8> {
        [
            bytes_field(1, &varint_field(3, 41)),
            bytes_field(
                TEXT_AND_IMAGES,
                &[bool_wrapper(GIF_AUTO_PLAY, true), bool_wrapper(RENDER_EMBEDS, true)].concat(),
            ),
        ]
        .concat()
    }

    fn loaded() -> SettingsSync {
        let mut sync = SettingsSync::new();
        assert_eq!(sync.connected(account()), None);
        sync
    }

    fn sent(save: &Save) -> Vec<u8> {
        settings_proto::decode(&save.settings)
    }

    fn text(wrappers: &[Vec<u8>]) -> Vec<u8> {
        bytes_field(TEXT_AND_IMAGES, &wrappers.concat())
    }

    #[test]
    fn nothing_is_edited_before_ready() {
        assert!(SettingsSync::new().edit(Setting::GifAutoPlay(false)).is_none());
    }

    #[test]
    fn a_change_saves_its_whole_message_at_once() {
        let mut sync = loaded();
        let edited = sync.edit(Setting::GifAutoPlay(false)).unwrap();
        assert_eq!(edited.patch.media.gif_auto_play, Some(false));
        let schedule = edited.schedule.unwrap();
        assert_eq!(schedule.after, Duration::ZERO);

        let save = sync.save(schedule.generation).unwrap();
        assert_eq!(sent(&save), text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, true)]));
        assert_eq!(save.required_data_version, None, "only an offline edit names a version");
        assert!(sync.save(schedule.generation).is_none(), "a timer fires once");
    }

    #[test]
    fn changes_before_the_timer_go_together() {
        let mut sync = loaded();
        let first = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        let second = sync.edit(Setting::RenderEmbeds(false)).unwrap().schedule.unwrap();
        assert!(sync.save(first.generation).is_none(), "the first timer was replaced");
        let developer = sync.edit(Setting::DeveloperMode(true)).unwrap().schedule.unwrap();
        assert!(sync.save(second.generation).is_none());

        let save = sync.save(developer.generation).unwrap();
        let expected = [
            text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, false)]),
            bytes_field(13, &varint_field(2, 1)),
        ];
        assert_eq!(sent(&save), expected.concat(), "the second change saw the first");
    }

    #[test]
    fn the_answer_replaces_the_copy_and_ends_the_edit() {
        let mut sync = loaded();
        let schedule = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        let save = sync.save(schedule.generation).unwrap();
        let answer = [account(), bytes_field(13, &varint_field(2, 1))].concat();
        let applied = sync.saved(&save, &Saved { settings: Some(answer), out_of_date: false }).unwrap();
        assert_eq!(applied.patch.developer_mode, Some(true), "what another client changed comes with it");
        assert_eq!(applied.patch.media.gif_auto_play, Some(true), "Discord's word, as the edit is not in it");
        assert_eq!(applied.schedule, None);
        assert!(sync.to_save.is_empty());
    }

    #[test]
    fn a_change_made_while_saving_is_not_lost() {
        let mut sync = loaded();
        let schedule = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        let save = sync.save(schedule.generation).unwrap();
        let during = sync.edit(Setting::RenderEmbeds(false)).unwrap();
        assert!(during.schedule.is_some(), "nothing is waiting, as the save is already on its way");

        let answer = text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, true)]);
        let applied = sync.saved(&save, &Saved { settings: Some(answer), out_of_date: false }).unwrap();
        assert_eq!(applied.patch.media.render_embeds, Some(false), "put back on top of the answer");
        let next = sync.save(applied.schedule.unwrap().generation).unwrap();
        assert_eq!(sent(&next), text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, false)]));
    }

    #[test]
    fn an_edit_made_offline_names_the_version_it_was_made_against() {
        let mut sync = loaded();
        let schedule = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        sync.disconnected();
        assert!(sync.save(schedule.generation).is_none(), "the timer was stopped");

        let schedule = sync.connected(account()).expect("sent after READY");
        assert!(schedule.after >= Duration::from_secs(5) && schedule.after < Duration::from_secs(10));
        let save = sync.save(schedule.generation).unwrap();
        assert_eq!(save.required_data_version, Some(41));

        let applied = sync.saved(&save, &Saved { settings: Some(account()), out_of_date: true }).unwrap();
        assert_eq!(applied.patch.media.gif_auto_play, Some(true), "out of date: Discord's settings stand");
        assert_eq!(sync.offline_data_version, None);
        let next = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        assert_eq!(sync.save(next.generation).unwrap().required_data_version, None);
    }

    #[test]
    fn a_rate_limit_waits_for_retry_after_but_not_long() {
        let mut sync = loaded();
        let schedule = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        sync.save(schedule.generation).unwrap();
        assert_eq!(sync.rate_limited(Some("7")).after, Duration::from_secs(7));
        assert_eq!(sync.rate_limited(Some("600")).after, Duration::from_secs(30));
        assert_eq!(sync.rate_limited(Some("soon")).after, Duration::from_secs(30), "60, cut to 30");
        assert_eq!(sync.rate_limited(None).after, Duration::from_secs(30));

        let retry = sync.rate_limited(Some("5"));
        let edited = sync.edit(Setting::RenderEmbeds(false)).unwrap();
        assert_eq!(edited.schedule, None, "the retry stands, and takes this change too");
        let save = sync.save(retry.generation).unwrap();
        assert_eq!(sent(&save), text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, false)]));
    }

    #[test]
    fn a_failed_save_keeps_the_edit_for_the_next_change() {
        let mut sync = loaded();
        let schedule = sync.edit(Setting::GifAutoPlay(false)).unwrap().schedule.unwrap();
        sync.save(schedule.generation).unwrap();
        sync.failed();
        let next = sync.edit(Setting::RenderEmbeds(false)).unwrap().schedule.unwrap();
        let save = sync.save(next.generation).unwrap();
        assert_eq!(sent(&save), text(&[bool_wrapper(GIF_AUTO_PLAY, false), bool_wrapper(RENDER_EMBEDS, false)]));
    }

    #[test]
    fn updates_from_other_clients_merge_into_the_copy() {
        let mut sync = loaded();
        sync.received(&partial(&[(13, varint_field(2, 1))]), true);
        let edited = sync.edit(Setting::GifAutoPlay(false)).unwrap();
        assert_eq!(edited.patch.media.gif_auto_play, Some(false));
        let patch = SettingsPatch::from_proto_bytes(sync.proto.as_deref().unwrap()).unwrap();
        assert_eq!((patch.developer_mode, patch.media.gif_auto_play), (Some(true), Some(false)));
        // A full update replaces the copy.
        sync.received(&account(), false);
        assert_eq!(sync.proto, Some(account()));
        sync.received(b"\x08", true);
        assert_eq!(sync.proto, Some(account()), "garbage changes nothing");
    }

    #[test]
    fn a_reload_drops_what_was_not_saved() {
        let mut sync = loaded();
        sync.edit(Setting::DeveloperMode(true)).unwrap();
        let patch = sync.reloaded(account()).unwrap();
        assert_eq!(patch.developer_mode, None);
        assert!(sync.to_save.is_empty() && sync.timer.is_none());
    }
}
