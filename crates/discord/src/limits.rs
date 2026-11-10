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

use crate::experiments::Experiments;
use crate::model::{Guild, Snowflake};

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
}
