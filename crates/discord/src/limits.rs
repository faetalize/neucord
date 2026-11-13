//! What Nitro, a server's boosts and Discord's experiments allow us. The
//! official client checks these before sending anything, so a request it
//! would hold back never reaches Discord; Neucord checks them the same way,
//! and never works around one (no links standing in for Nitro emoji).
//!
//! Nitro is read from our user, in READY and USER_UPDATE. Most of it is
//! perks: the client asks the `perks` bitmask when Discord sends one, and
//! falls back to what the `premium_type` includes (722178's `R`). Upload
//! sizes go by `premium_type` alone (`getUserMaxFileSize`).

use serde_json::Value;

use crate::emoji::{GuildEmoji, Place};
use crate::experiments::Experiments;
use crate::model::{Guild, Snowflake};
use crate::stickers::{Origin, Place as StickerPlace};

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// `premium_type`. The product names are Discord's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PremiumType {
    /// Nitro Basic (3).
    Tier0,
    /// Nitro Classic (1), no longer sold.
    Tier1,
    /// Nitro (2).
    Tier2,
}

impl PremiumType {
    /// 0, or a type this doesn't know, is none: the client finds no perks
    /// for an unknown type.
    fn from_value(value: &Value) -> Option<PremiumType> {
        match value.as_u64()? {
            3 => Some(PremiumType::Tier0),
            1 => Some(PremiumType::Tier1),
            2 => Some(PremiumType::Tier2),
            _ => None,
        }
    }

    /// The perks it includes, for an account Discord sends no `perks` for.
    fn includes(self, perk: Perk) -> bool {
        use Perk::*;
        match self {
            PremiumType::Tier0 => matches!(perk, AnimatedEmojis | EmojisEverywhere | StickersEverywhere),
            PremiumType::Tier1 => matches!(perk, AnimatedEmojis | EmojisEverywhere | AnimatedAvatar | StreamMidQuality),
            PremiumType::Tier2 => matches!(
                perk,
                AnimatedEmojis
                    | EmojisEverywhere
                    | StickersEverywhere
                    | AnimatedAvatar
                    | StreamMidQuality
                    | StreamHighQuality
                    | IncreasedMessageLength
            ),
        }
    }
}

/// What Nitro unlocks, numbered as in `perks`' bitmask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Perk {
    AnimatedEmojis = 1,
    /// Custom emoji from one server in any other, and in DMs.
    EmojisEverywhere = 2,
    /// Server stickers outside their server.
    StickersEverywhere = 3,
    AnimatedAvatar = 5,
    /// Streaming above 720p, or at 60 fps (see [`Limits::can_stream`]).
    StreamMidQuality = 9,
    /// Streaming at the source's resolution.
    StreamHighQuality = 10,
    /// Messages of 4000 characters instead of 2000.
    IncreasedMessageLength = 15,
}

/// Our Nitro, from our user object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nitro {
    pub premium_type: Option<PremiumType>,
    /// `perks.active_perks_bitmask`: bit `n % 64` of word `n / 64` grants
    /// perk `n`. Its `config_by_perk` only restricts perks Neucord doesn't
    /// read (name styles, themes, app icons).
    perks: Option<Vec<u64>>,
    /// Staff (user flag 1) have their own upload size.
    staff: bool,
}

impl Nitro {
    pub fn from_user(user: &Value) -> Nitro {
        let word = |w: &Value| w.as_u64().or_else(|| w.as_str()?.parse().ok()).unwrap_or(0);
        let perks = user["perks"]
            .as_object()
            .map(|p| p.get("active_perks_bitmask").and_then(Value::as_array).into_iter().flatten().map(word).collect());
        let flags = user["flags"].as_u64().unwrap_or(0) | user["public_flags"].as_u64().unwrap_or(0);
        Nitro { premium_type: PremiumType::from_value(&user["premium_type"]), perks, staff: flags & 1 != 0 }
    }

    pub fn has(&self, perk: Perk) -> bool {
        let bit = perk as usize;
        match &self.perks {
            Some(words) => words.get(bit / 64).is_some_and(|w| w >> (bit % 64) & 1 == 1),
            None => self.premium_type.is_some_and(|t| t.includes(perk)),
        }
    }

    /// One file's size before a server raises it.
    fn file_size(&self, upload_rollout: bool) -> u64 {
        if self.staff {
            return GIB;
        }
        match self.premium_type {
            None => DEFAULT_FILE_SIZE,
            Some(PremiumType::Tier2) if upload_rollout => GIB,
            Some(PremiumType::Tier2) => 500 * MIB,
            Some(PremiumType::Tier0 | PremiumType::Tier1) => 50 * MIB,
        }
    }
}

/// One file's size without Nitro or a server raising it (`TbF`).
const DEFAULT_FILE_SIZE: u64 = 10 * MIB;
/// No file limit for a message is under this (409481), so without Nitro
/// it's this, not [`DEFAULT_FILE_SIZE`].
const MESSAGE_FILE_FLOOR: u64 = 20 * MIB;

/// A message's length as the client counts it: in UTF-16 units, so most
/// emoji count twice.
pub fn message_length(text: &str) -> usize {
    text.encode_utf16().count()
}

/// A message's length without Nitro, and with the perk.
const MESSAGE: usize = 2000;
const LONG_MESSAGE: usize = 4000;

/// How many scheduled messages may wait at once, for Nitro and without it.
const SCHEDULED: usize = 1;
const SCHEDULED_NITRO: usize = 25;

/// How many scheduled messages may wait (`ZG` in 566908).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduledLimit {
    pub max: usize,
    /// Nitro would raise it, which the client says with an upsell where
    /// Neucord only says the limit.
    pub upgradable: bool,
}

/// A message over the limit, which the client turns back with an alert
/// (710272). Without the perk it opens a Nitro upsell instead, which
/// Neucord doesn't copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooLong {
    pub length: usize,
    pub max: usize,
}

impl TooLong {
    pub fn heading(&self) -> &'static str {
        "Your message is too long."
    }

    /// The client says "of {length} characters shorter", with the whole
    /// length where the amount over would read right.
    pub fn body(&self) -> String {
        format!(
            "Please make your message of {} characters shorter. We've set the limit at {} characters to be courteous to others.",
            self.length, self.max
        )
    }
}

/// What the length indicator shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LengthCount {
    /// Characters left; below zero, over the limit.
    pub remaining: i64,
    pub max: usize,
    /// The number shows only near the limit.
    pub number: bool,
    /// The Nitro icon: Nitro is what lets the message past the usual length.
    pub powered: bool,
}

impl LengthCount {
    pub fn tooltip(&self) -> String {
        match self.remaining {
            0 => "Message is at max length".to_owned(),
            r if r > 0 => format!("{r} characters remaining"),
            _ => "Message is too long".to_owned(),
        }
    }

    pub fn powered_tooltip(&self) -> String {
        format!("Nitro is powering you with {} characters per message!", self.max)
    }
}

/// Why a custom emoji can't be sent where we are (`getEmojiUnavailableReason`,
/// 690521).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// Another server's emoji, in a channel without Use External Emojis for
    /// us (`DISALLOW_EXTERNAL`). The client leaves it out.
    NotHere,
    /// Its server no longer has the boosts it took (`available` is false;
    /// `GUILD_SUBSCRIPTION_UNAVAILABLE`). The client shows it locked.
    ServerBoost,
    /// It takes Nitro (`PREMIUM_LOCKED`): another server's emoji without
    /// Emojis Everywhere, or an animated one without Animated Emojis. The
    /// client shows it locked, and offers Nitro on a click, which Neucord
    /// doesn't copy.
    Nitro,
}

/// Why a sticker can't be sent where we are (`SENDABLE_WITH_PREMIUM`,
/// `SENDABLE_WITH_BOOSTED_GUILD` and `NONSENDABLE` of 361670's sendability).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StickerUnavailable {
    /// Another server's sticker, in a channel without Use External Stickers
    /// for us (`NONSENDABLE`).
    NotHere,
    /// Its server no longer has the boosts it took (`available` is false;
    /// `SENDABLE_WITH_BOOSTED_GUILD`).
    ServerBoost,
    /// Another server's sticker without Stickers Everywhere
    /// (`SENDABLE_WITH_PREMIUM`). The client opens a Nitro upsell on a
    /// click, which Neucord doesn't copy.
    Nitro,
}

/// What one message may carry (the client's message upload target, 313034).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Uploads {
    /// The largest file, in bytes.
    pub file_size: u64,
    pub files: usize,
    /// All files together, in bytes.
    pub total_size: u64,
}

/// How a stream is set up, in the go-live settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamPreset {
    /// Smoother video.
    Video,
    /// Sharper text, which alone may stream the source at 5 fps.
    Documents,
    Custom,
    /// Discord picks; offered only under an experiment.
    Auto,
}

/// Resolution (a height, 0 for the source's), frame rate, the only preset
/// it's for, and the perk or boost level that allows it; with neither,
/// anyone may.
type Stream = (u32, u32, Option<StreamPreset>, Option<Perk>, Option<u8>);

/// What we may stream: 753070's table, checked as `canStreamWithSettings`
/// (676030) does.
const STREAMS: &[Stream] = &[
    (0, 60, None, Some(Perk::StreamHighQuality), None),
    (0, 30, None, Some(Perk::StreamHighQuality), None),
    (0, 15, None, Some(Perk::StreamHighQuality), None),
    (0, 5, Some(StreamPreset::Documents), None, None),
    (1440, 60, None, Some(Perk::StreamMidQuality), Some(2)),
    (1440, 30, None, Some(Perk::StreamMidQuality), Some(2)),
    (1440, 15, None, Some(Perk::StreamMidQuality), Some(2)),
    (1080, 60, None, Some(Perk::StreamMidQuality), Some(2)),
    (1080, 30, None, Some(Perk::StreamMidQuality), Some(2)),
    (1080, 15, None, Some(Perk::StreamMidQuality), Some(2)),
    (720, 60, None, Some(Perk::StreamMidQuality), Some(1)),
    (720, 30, None, None, None),
    (720, 15, None, None, None),
    (720, 5, None, None, None),
    (480, 60, None, Some(Perk::StreamMidQuality), Some(1)),
    (480, 30, None, None, None),
    (480, 15, None, None, None),
    (480, 5, None, None, None),
];

/// The limits that apply to us: our Nitro, and the experiments Discord
/// rolls out to the account that change them. Server boosts come with the
/// server each check is for.
pub struct Limits<'a> {
    pub nitro: &'a Nitro,
    pub experiments: &'a Experiments,
    pub me: Snowflake,
}

impl Limits<'_> {
    /// The longest message we may send, or edit one to, as
    /// [`message_length`] counts it (135621).
    pub fn max_message_length(&self) -> usize {
        if self.nitro.has(Perk::IncreasedMessageLength) { LONG_MESSAGE } else { MESSAGE }
    }

    /// How many scheduled messages may wait. The client doesn't hold a new
    /// one back by it: Discord refuses the one over, with
    /// [`crate::scheduled::TOO_MANY`]. Only Nitro (Tier 2) raises it, not
    /// Basic or Classic, and not the `perks`.
    pub fn scheduled_messages(&self) -> ScheduledLimit {
        match self.nitro.premium_type {
            Some(PremiumType::Tier2) => ScheduledLimit { max: SCHEDULED_NITRO, upgradable: false },
            _ => ScheduledLimit { max: SCHEDULED, upgradable: true },
        }
    }

    /// Refuses `text` if it's over [`Self::max_message_length`]. A message
    /// is checked trimmed, an edit as the box holds it.
    pub fn check_length(&self, text: &str) -> Result<(), TooLong> {
        let (length, max) = (message_length(text), self.max_message_length());
        if length > max { Err(TooLong { length, max }) } else { Ok(()) }
    }

    /// What the message box's length indicator draws for a text of `length`,
    /// counted from the box as it is, untrimmed (263582, drawn in 246049).
    /// An edit has no Nitro icon.
    pub fn length_count(&self, length: usize, editing: bool) -> Option<LengthCount> {
        let max = self.max_message_length();
        let remaining = max as i64 - length as i64;
        let number = remaining <= (max / 10) as i64;
        let powered = !editing && self.nitro.has(Perk::IncreasedMessageLength) && length > MESSAGE && remaining >= 0;
        (number || powered).then_some(LengthCount { remaining, max, number, powered })
    }

    /// Whether we may send `emoji` in `place`, as the client's
    /// `getEmojiUnavailableReason` decides (for chat and reactions alike). An
    /// integration's emoji (`managed`) skips the Nitro check for other
    /// servers, an animated one doesn't. Emoji bought with a server's role
    /// subscription have a rule of their own, which needs those roles'
    /// purchase data Neucord has none of; they only show to members who
    /// hold the role (`GuildEmoji::usable_with`).
    pub fn emoji(&self, emoji: &GuildEmoji, place: &Place) -> Result<(), Unavailable> {
        let internal = place.guild == Some(emoji.guild_id);
        if place.guild.is_some() && !internal && !place.external {
            return Err(Unavailable::NotHere);
        }
        if !emoji.available {
            return Err(Unavailable::ServerBoost);
        }
        if !internal && !emoji.managed && !self.nitro.has(Perk::EmojisEverywhere) {
            return Err(Unavailable::Nitro);
        }
        if emoji.animated && !self.nitro.has(Perk::AnimatedEmojis) {
            return Err(Unavailable::Nitro);
        }
        Ok(())
    }

    /// Whether we may send a sticker from `origin` in `place`, as the client's
    /// sendability decides (`W$` in 361670). A standard pack's is open to
    /// everyone. A server's own is, unless its server lost its boosts; from
    /// another server it needs Use External Stickers (a DM needs none) and
    /// then Stickers Everywhere.
    pub fn sticker(&self, origin: &Origin, place: &StickerPlace) -> Result<(), StickerUnavailable> {
        let Origin::Guild { guild_id, available } = *origin else { return Ok(()) };
        if !available {
            return Err(StickerUnavailable::ServerBoost);
        }
        if place.guild == Some(guild_id) {
            return Ok(());
        }
        if !place.external_stickers {
            return Err(StickerUnavailable::NotHere);
        }
        match self.nitro.has(Perk::StickersEverywhere) {
            true => Ok(()),
            false => Err(StickerUnavailable::Nitro),
        }
    }

    /// What a message may carry in `guild`, or in DMs. The server's side
    /// comes from its features, not its boost level (453771's `o2`).
    pub fn uploads(&self, guild: Option<&Guild>) -> Uploads {
        let rollout = self.upload_rollout();
        let server = guild.map_or(DEFAULT_FILE_SIZE, |g| {
            let raised = |feature: &str, size| g.features.iter().any(|f| f == feature).then_some(size);
            [raised("MAX_FILE_SIZE_100_MB", 100 * MIB), raised("MAX_FILE_SIZE_50_MB", 50 * MIB)]
                .into_iter()
                .flatten()
                .fold(DEFAULT_FILE_SIZE, u64::max)
        });
        Uploads {
            file_size: MESSAGE_FILE_FLOOR.max(server).max(self.nitro.file_size(rollout)),
            files: 10,
            // For everyone, not only Nitro (292348's `jS`).
            total_size: if rollout { GIB } else { 500 * MIB },
        }
    }

    /// Whether we may stream with `preset` at `resolution` and `fps` in
    /// `guild`, or in a call.
    pub fn can_stream(&self, preset: StreamPreset, resolution: u32, fps: u32, guild: Option<&Guild>) -> bool {
        if preset == StreamPreset::Auto {
            return self.auto_stream_quality();
        }
        let tier = guild.map(|g| g.premium_tier);
        STREAMS.iter().any(|&(height, rate, only, perk, boost)| {
            height == resolution
                && rate == fps
                && only.is_none_or(|p| p == preset)
                && ((perk.is_none() && boost.is_none())
                    || perk.is_some_and(|p| self.nitro.has(p))
                    || boost.is_some_and(|b| tier.is_some_and(|t| t >= b)))
        })
    }

    /// The largest PNG we re-encode as WebP before uploading it, for the
    /// accounts that experiment reaches: variant 1 takes files up to 512 KiB,
    /// variant 2 up to 256 KiB, any other none (48325).
    pub fn webp_conversion(&self) -> Option<u64> {
        match self.experiments.apex_user(self.me, "2025-09-image-attachment-mezzanine-v2")?.variant {
            1 => Some(524_288),
            2 => Some(262_144),
            _ => None,
        }
    }

    /// The sizes images are asked for in, if the account has
    /// `2026-07-attachment-image-ladder` (502571): variant 1 carries its
    /// settings as JSON, and without them, or with ones out of range, the
    /// client has none.
    pub fn image_ladder(&self) -> Option<crate::cdn::Ladder> {
        let assignment = self.experiments.apex_user(self.me, "2026-07-attachment-image-ladder")?;
        if assignment.variant != 1 {
            return None;
        }
        crate::cdn::Ladder::from_config(assignment.config.as_ref()?.as_str()?)
    }

    /// Whether images wait to load until they are near the screen, which
    /// `2026-02-lazy-load-all-images` (619517) does for variant 1.
    pub fn lazy_images(&self) -> bool {
        self.experiments.apex_user(self.me, "2026-02-lazy-load-all-images").is_some_and(|a| a.variant == 1)
    }

    /// Whether a server is age-restricted by its `nsfw_level` rather than the
    /// level its owner set, which `2025-09-server-nsfw-level` (329972) does
    /// for variant 1.
    pub fn servers_by_nsfw_level(&self) -> bool {
        self.experiments.apex_user(self.me, "2025-09-server-nsfw-level").is_some_and(|a| a.variant == 1)
    }

    /// Variant 1 raises Nitro's file size to 1 GiB, and everyone's total
    /// per message to 1 GiB (219882).
    fn upload_rollout(&self) -> bool {
        self.experiments.apex_user(self.me, "2026-09-nitro-file-upload-rollout").is_some_and(|a| a.variant == 1)
    }

    /// Variants 1 and 2 offer the Auto stream preset (258585).
    fn auto_stream_quality(&self) -> bool {
        let variant = self.experiments.apex_user(self.me, "2025-10-go-live-auto-quality").map(|a| a.variant);
        matches!(variant, Some(1 | 2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::experiments::murmur3;
    use serde_json::json;

    const ME: Snowflake = Snowflake(10);

    fn nitro(user: Value) -> Nitro {
        Nitro::from_user(&user)
    }

    /// READY assigning us `variant` of each named Apex experiment.
    fn experiments(assigned: &[(&str, i64)]) -> Experiments {
        let assignments: Vec<Value> = assigned.iter().map(|(name, v)| json!([murmur3(name), v, 0, 1])).collect();
        Experiments::from_ready(&json!({
            "apex_experiments": { "assignments": { "1": { "10": { "assignments": assignments } } } }
        }))
    }

    fn guild(features: &[&str], premium_tier: u8) -> Guild {
        serde_json::from_value(json!({"id": "1", "name": "A", "features": features, "premium_tier": premium_tier}))
            .unwrap()
    }

    #[test]
    fn premium_types_and_their_perks() {
        let none = nitro(json!({"premium_type": 0}));
        let basic = nitro(json!({"premium_type": 3}));
        let classic = nitro(json!({"premium_type": 1}));
        let full = nitro(json!({"premium_type": 2}));
        assert_eq!(none.premium_type, None);
        assert_eq!(nitro(json!({"premium_type": 9})).premium_type, None, "an unknown type has no perks");
        assert!(!none.has(Perk::EmojisEverywhere));
        assert!(basic.has(Perk::EmojisEverywhere) && basic.has(Perk::StickersEverywhere));
        assert!(!basic.has(Perk::AnimatedAvatar) && !basic.has(Perk::IncreasedMessageLength));
        assert!(classic.has(Perk::AnimatedAvatar) && classic.has(Perk::StreamMidQuality));
        assert!(!classic.has(Perk::StickersEverywhere) && !classic.has(Perk::StreamHighQuality));
        assert!(full.has(Perk::StreamHighQuality) && full.has(Perk::IncreasedMessageLength));
    }

    #[test]
    fn perks_win_over_the_premium_type() {
        // Bits 2 and 15: emoji everywhere, longer messages.
        let perks = json!({"active_perks_bitmask": [(1u64 << 2) | (1 << 15)], "config_by_perk": {}});
        let granted = nitro(json!({"premium_type": 0, "perks": perks}));
        assert!(granted.has(Perk::EmojisEverywhere) && granted.has(Perk::IncreasedMessageLength));
        assert!(!granted.has(Perk::AnimatedEmojis));
        let withheld = nitro(json!({"premium_type": 2, "perks": {"active_perks_bitmask": [], "config_by_perk": {}}}));
        assert!(!withheld.has(Perk::AnimatedEmojis), "Nitro, but no perks");
        let as_text = nitro(json!({"perks": {"active_perks_bitmask": ["32"]}}));
        assert!(as_text.has(Perk::AnimatedAvatar));
    }

    #[test]
    fn message_length_counts_utf16_and_nitro_doubles_it() {
        assert_eq!(message_length("abc"), 3);
        assert_eq!(message_length("é"), 1);
        assert_eq!(message_length("🙂"), 2);
        let experiments = Experiments::default();
        let limit = |n: &Nitro| Limits { nitro: n, experiments: &experiments, me: ME }.max_message_length();
        assert_eq!(limit(&Nitro::default()), 2000);
        assert_eq!(limit(&nitro(json!({"premium_type": 3}))), 2000, "Basic doesn't lengthen messages");
        assert_eq!(limit(&nitro(json!({"premium_type": 2}))), 4000);
    }

    #[test]
    fn only_nitro_schedules_more_than_one_message() {
        let experiments = Experiments::default();
        let limit = |n: Nitro| Limits { nitro: &n, experiments: &experiments, me: ME }.scheduled_messages();
        let one = ScheduledLimit { max: 1, upgradable: true };
        assert_eq!(limit(Nitro::default()), one);
        assert_eq!(limit(nitro(json!({"premium_type": 3}))), one, "Basic");
        assert_eq!(limit(nitro(json!({"premium_type": 1}))), one, "Classic");
        assert_eq!(limit(nitro(json!({"premium_type": 2}))), ScheduledLimit { max: 25, upgradable: false });
        // The client asks the premium type, not the perks.
        let perks = json!({"active_perks_bitmask": [u64::MAX], "config_by_perk": {}});
        assert_eq!(limit(nitro(json!({"premium_type": 3, "perks": perks}))), one);
    }

    fn counted(n: &Nitro, length: usize, editing: bool) -> Option<LengthCount> {
        let experiments = Experiments::default();
        Limits { nitro: n, experiments: &experiments, me: ME }.length_count(length, editing)
    }

    #[test]
    fn too_long_messages_are_refused_by_utf16_length() {
        let experiments = Experiments::default();
        let free = Nitro::default();
        let full = nitro(json!({"premium_type": 2}));
        let check = |n: &Nitro, text: &str| Limits { nitro: n, experiments: &experiments, me: ME }.check_length(text);
        assert_eq!(check(&free, &"a".repeat(2000)), Ok(()));
        let over = check(&free, &"a".repeat(2001)).unwrap_err();
        assert_eq!((over.length, over.max), (2001, 2000));
        assert!(over.body().contains("message of 2001 characters shorter"));
        assert!(over.body().contains("limit at 2000 characters"));
        // 1001 emoji are 2002 units.
        assert!(check(&free, &"🙂".repeat(1000)).is_ok() && check(&free, &"🙂".repeat(1001)).is_err());
        assert_eq!(check(&full, &"a".repeat(4000)), Ok(()));
        assert_eq!(check(&full, &"a".repeat(4001)).unwrap_err().max, 4000);
    }

    #[test]
    fn the_indicator_shows_near_the_limit() {
        let free = Nitro::default();
        assert_eq!(counted(&free, 1799, false), None, "200 left is where it shows");
        assert_eq!(counted(&free, 1800, false).map(|c| (c.remaining, c.number, c.powered)), Some((200, true, false)));
        assert_eq!(counted(&free, 2000, false).unwrap().tooltip(), "Message is at max length");
        let over = counted(&free, 2010, true).unwrap();
        assert_eq!((over.remaining, over.tooltip()), (-10, "Message is too long".to_owned()));
        assert_eq!(counted(&free, 1900, false).unwrap().tooltip(), "100 characters remaining");
    }

    #[test]
    fn nitro_shows_the_indicator_past_the_usual_length() {
        let full = nitro(json!({"premium_type": 2}));
        assert_eq!(counted(&full, 2000, false), None, "not past it yet");
        let icon = counted(&full, 2001, false).unwrap();
        assert!(icon.powered && !icon.number, "the icon alone until near 4000");
        assert_eq!(icon.powered_tooltip(), "Nitro is powering you with 4000 characters per message!");
        assert_eq!(counted(&full, 3599, false).map(|c| c.number), Some(false));
        assert_eq!(counted(&full, 3600, false).map(|c| (c.number, c.powered)), Some((true, true)));
        assert_eq!(counted(&full, 4000, false).map(|c| (c.remaining, c.powered)), Some((0, true)));
        assert_eq!(counted(&full, 4001, false).map(|c| (c.remaining, c.number, c.powered)), Some((-1, true, false)));
        assert_eq!(counted(&full, 3000, true), None, "an edit has no icon");
        assert_eq!(counted(&full, 3700, true).map(|c| (c.number, c.powered)), Some((true, false)));
        assert_eq!(
            counted(&nitro(json!({"premium_type": 3})), 2001, false).map(|c| c.number),
            Some(true),
            "no perk, over"
        );
    }

    #[test]
    fn webp_conversion_follows_the_experiments_variant() {
        let webp = |e: &Experiments| Limits { nitro: &Nitro::default(), experiments: e, me: ME }.webp_conversion();
        let name = "2025-09-image-attachment-mezzanine-v2";
        assert_eq!(webp(&Experiments::default()), None, "not in it");
        assert_eq!(webp(&experiments(&[(name, 0)])), None, "control");
        assert_eq!(webp(&experiments(&[(name, 1)])), Some(512 * 1024));
        assert_eq!(webp(&experiments(&[(name, 2)])), Some(256 * 1024));
        assert_eq!(webp(&experiments(&[(name, 3)])), None, "a variant it doesn't define");
        assert_eq!(webp(&experiments(&[(name, -1)])), None, "not eligible");
    }

    #[test]
    fn images_follow_the_ladder_and_load_lazily_for_the_accounts_that_have_them() {
        let with = |ladder: Option<&str>, lazy: i64| {
            let ladder_hash = murmur3("2026-07-attachment-image-ladder");
            let mut entry = vec![json!(ladder_hash), json!(1), json!(0), json!(1)];
            if let Some(config) = ladder {
                entry.extend([Value::Null, json!(config)]);
            }
            Experiments::from_ready(&json!({
                "apex_experiments": { "assignments": { "1": { "10": { "assignments": [
                    entry, [murmur3("2026-02-lazy-load-all-images"), lazy, 0, 1]
                ] } } } }
            }))
        };
        let nitro = Nitro::default();
        let limits = |e| Limits { nitro: &nitro, experiments: e, me: ME };
        let none = Experiments::default();
        assert_eq!((limits(&none).image_ladder(), limits(&none).lazy_images()), (None, false));
        let on = with(Some(r#"{"ladder":[64,128]}"#), 1);
        assert!(limits(&on).image_ladder().is_some() && limits(&on).lazy_images());
        // The variant without its settings, or with bad ones, has no ladder.
        let (unset, bad, other) = (with(None, 0), with(Some(r#"{"ladder":[]}"#), 0), with(None, 2));
        assert_eq!(limits(&unset).image_ladder(), None);
        assert_eq!(limits(&bad).image_ladder(), None);
        assert!(!limits(&other).lazy_images(), "only variant 1");
    }

    #[test]
    fn servers_are_age_restricted_by_their_nsfw_level_in_variant_1() {
        let nitro = Nitro::default();
        let by_level = |e: &Experiments| Limits { nitro: &nitro, experiments: e, me: ME }.servers_by_nsfw_level();
        assert!(!by_level(&Experiments::default()));
        assert!(by_level(&experiments(&[("2025-09-server-nsfw-level", 1)])));
        assert!(!by_level(&experiments(&[("2025-09-server-nsfw-level", 0)])));
    }

    #[test]
    fn upload_sizes() {
        let none = Experiments::default();
        let rollout = experiments(&[("2026-09-nitro-file-upload-rollout", 1)]);
        let uploads =
            |n: &Nitro, e: &Experiments, g: Option<&Guild>| Limits { nitro: n, experiments: e, me: ME }.uploads(g);
        let free = Nitro::default();
        let basic = nitro(json!({"premium_type": 3}));
        let full = nitro(json!({"premium_type": 2}));
        let staff = nitro(json!({"flags": 1}));

        let dm = uploads(&free, &none, None);
        assert_eq!(dm, Uploads { file_size: 20 * MIB, files: 10, total_size: 500 * MIB });
        assert_eq!(uploads(&free, &none, Some(&guild(&[], 3))).file_size, 20 * MIB, "boosts alone raise nothing");
        assert_eq!(uploads(&free, &none, Some(&guild(&["MAX_FILE_SIZE_50_MB"], 2))).file_size, 50 * MIB);
        let both = guild(&["MAX_FILE_SIZE_50_MB", "MAX_FILE_SIZE_100_MB"], 3);
        assert_eq!(uploads(&free, &none, Some(&both)).file_size, 100 * MIB);
        assert_eq!(uploads(&basic, &none, None).file_size, 50 * MIB);
        assert_eq!(uploads(&basic, &none, Some(&both)).file_size, 100 * MIB, "the larger of ours and the server's");
        assert_eq!(uploads(&full, &none, Some(&both)).file_size, 500 * MIB);
        assert_eq!(uploads(&staff, &none, None).file_size, GIB);

        assert_eq!(uploads(&full, &rollout, None), Uploads { file_size: GIB, files: 10, total_size: GIB });
        assert_eq!(uploads(&free, &rollout, None).total_size, GIB, "the total grows for everyone");
        assert_eq!(uploads(&basic, &rollout, None).file_size, 50 * MIB);
        let control = experiments(&[("2026-09-nitro-file-upload-rollout", 0)]);
        assert_eq!(uploads(&full, &control, None).file_size, 500 * MIB);
    }

    #[test]
    fn stream_quality_takes_nitro_or_boosts() {
        let none = Experiments::default();
        let free = Nitro::default();
        let classic = nitro(json!({"premium_type": 1}));
        let full = nitro(json!({"premium_type": 2}));
        let can = |n: &Nitro, e: &Experiments, preset, res, fps, g: Option<&Guild>| {
            Limits { nitro: n, experiments: e, me: ME }.can_stream(preset, res, fps, g)
        };
        use StreamPreset::*;
        assert!(can(&free, &none, Video, 720, 30, None));
        assert!(!can(&free, &none, Video, 720, 60, None));
        assert!(can(&free, &none, Video, 720, 60, Some(&guild(&[], 1))));
        assert!(!can(&free, &none, Video, 1080, 30, Some(&guild(&[], 1))));
        assert!(can(&free, &none, Video, 1080, 30, Some(&guild(&[], 2))));
        assert!(can(&classic, &none, Video, 1440, 60, None));
        assert!(!can(&classic, &none, Video, 0, 30, Some(&guild(&[], 3))), "the source needs Nitro");
        assert!(can(&full, &none, Custom, 0, 60, None));
        assert!(can(&free, &none, Documents, 0, 5, None));
        assert!(!can(&free, &none, Video, 0, 5, None));
        assert!(!can(&full, &none, Video, 1080, 24, None), "only the listed rates");
        assert!(!can(&full, &none, Auto, 720, 30, None));
        let auto = experiments(&[("2025-10-go-live-auto-quality", 2)]);
        assert!(can(&free, &auto, Auto, 0, 0, None));
    }

    #[test]
    fn stickers_follow_the_clients_sendability() {
        // `W$` in 361670: a pack's is open; a server's own too unless it lost its boosts; another
        // server's needs Use External Stickers (not in a DM) and then Stickers Everywhere.
        let none = Experiments::default();
        let free = Nitro::default();
        let basic = nitro(json!({"premium_type": 3}));
        let classic = nitro(json!({"premium_type": 1}));
        let can = |n: &Nitro, origin: Origin, guild: Option<u64>, external: bool| {
            let place =
                StickerPlace { guild: guild.map(Snowflake), external_emojis: true, external_stickers: external };
            Limits { nitro: n, experiments: &none, me: ME }.sticker(&origin, &place)
        };
        let server = |id: u64, available| Origin::Guild { guild_id: Snowflake(id), available };
        let pack = Origin::Pack { pack_id: Snowflake(9) };
        assert_eq!(can(&free, pack, Some(1), false), Ok(()), "a pack, even without Use External Stickers");
        assert_eq!(
            can(&free, server(1, true), Some(1), false),
            Ok(()),
            "its own server's, with no Nitro or permission"
        );
        assert_eq!(can(&free, server(2, true), Some(1), true), Err(StickerUnavailable::Nitro));
        assert_eq!(can(&basic, server(2, true), Some(1), true), Ok(()), "Basic has Stickers Everywhere");
        assert_eq!(can(&classic, server(2, true), Some(1), true), Err(StickerUnavailable::Nitro), "Classic doesn't");
        assert_eq!(can(&basic, server(2, true), Some(1), false), Err(StickerUnavailable::NotHere));
        assert_eq!(
            can(&free, server(2, true), Some(1), false),
            Err(StickerUnavailable::NotHere),
            "the permission first"
        );
        assert_eq!(can(&basic, server(2, true), None, true), Ok(()), "a DM");
        assert_eq!(can(&free, server(2, true), None, true), Err(StickerUnavailable::Nitro));
        // Lost boosts win over everything, in its own server too.
        assert_eq!(can(&basic, server(1, false), Some(1), true), Err(StickerUnavailable::ServerBoost));
        assert_eq!(can(&basic, server(2, false), Some(1), false), Err(StickerUnavailable::ServerBoost));
    }
}
