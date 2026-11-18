//! The sensitive media filter: which attachments and embeds the client
//! covers until the reader reveals them. Discord scans media and flags it
//! (`flags`, `content_scan_version`); the account's settings decide, for
//! each kind of harm and each place a message is in, whether flagged media
//! is shown, blurred or blocked. The client's rules are `Jn` in 811602, the
//! defaults in 632119 (explicit) and 444802 (gore), and 282108 and 294520
//! for what covers what.
//!
//! Blur and block cover alike: the client draws no difference between them.
//! Not copied: media of message components, and the client's developer
//! switches.

use crate::Snowflake;
use crate::gates::Gates;
use crate::verification::Account;

/// A kind of harm Discord flags media for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Harm {
    Explicit,
    Gore,
    SelfHarm,
}

impl Harm {
    /// The order the client checks them in (`Jn`).
    const ALL: [Harm; 3] = [Harm::Explicit, Harm::Gore, Harm::SelfHarm];

    /// `CONTAINS_EXPLICIT_MEDIA`, `CONTAINS_GORE_CONTENT` and
    /// `CONTAINS_SELF_HARM_CONTENT`: the same bits on attachments and embeds.
    fn flag(self) -> u64 {
        match self {
            Harm::Explicit => 1 << 4,
            Harm::Gore => 1 << 6,
            Harm::SelfHarm => 1 << 7,
        }
    }

    /// The client's `bitmask`.
    fn bit(self) -> u8 {
        match self {
            Harm::Explicit => 1,
            Harm::Gore => 2,
            Harm::SelfHarm => 4,
        }
    }
}

/// `ExplicitContentRedaction`: what the account chose for a kind of harm in
/// a place.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Redaction {
    /// Never chosen: the defaults decide.
    #[default]
    Unset,
    Show,
    Blur,
    Block,
}

impl Redaction {
    pub fn from_raw(raw: u64) -> Self {
        match raw {
            1 => Redaction::Show,
            2 => Redaction::Blur,
            3 => Redaction::Block,
            _ => Redaction::Unset,
        }
    }

    /// `D` in 282108.
    fn covers(self) -> bool {
        matches!(self, Redaction::Blur | Redaction::Block)
    }
}

/// Where a message is, as far as the filter cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    Guild,
    FriendDm,
    NonFriendDm,
}

impl Place {
    /// `L` in 282108: `None` for our own messages, which are never covered,
    /// and for a channel we don't know. A DM's sender we can't name counts
    /// as not a friend.
    pub fn of(
        channel_known: bool,
        private: bool,
        author: Option<Snowflake>,
        me: Snowflake,
        friend: bool,
    ) -> Option<Place> {
        if !channel_known || author == Some(me) {
            return None;
        }
        Some(match (private, friend) {
            (false, _) => Place::Guild,
            (true, true) => Place::FriendDm,
            (true, false) => Place::NonFriendDm,
        })
    }
}

/// One kind of harm's three settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Redactions {
    pub guilds: Redaction,
    pub friend_dm: Redaction,
    pub non_friend_dm: Redaction,
}

impl Redactions {
    fn at(&self, place: Place) -> Redaction {
        match place {
            Place::Guild => self.guilds,
            Place::FriendDm => self.friend_dm,
            Place::NonFriendDm => self.non_friend_dm,
        }
    }
}

/// `explicitContentFilter`, the setting from before the per-place ones: it
/// decides explicit media's defaults in DMs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DmFilter {
    Disabled,
    #[default]
    NonFriends,
    FriendsAndNonFriends,
}

impl DmFilter {
    pub fn from_raw(raw: u64) -> Self {
        match raw {
            0 => DmFilter::Disabled,
            2 => DmFilter::FriendsAndNonFriends,
            _ => DmFilter::NonFriends,
        }
    }
}

/// The account's settings that decide (`text_and_images` in the settings
/// protobuf), as it set them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Preferences {
    pub explicit: Redactions,
    pub gore: Redactions,
    pub dm_filter: DmFilter,
}

/// The kinds of harm media is covered for in a place (`LO`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Harms(u8);

impl Harms {
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn contains(self, harm: Harm) -> bool {
        self.0 & harm.bit() != 0
    }

    /// The first harm `flags` flag that is among these (`b`, whose first
    /// reason the client shows).
    pub fn flagged(self, flags: u64) -> Option<Harm> {
        Harm::ALL.into_iter().find(|&h| self.contains(h) && flags & h.flag() != 0)
    }
}

/// What the filter reads of the account besides its settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rules {
    pub preferences: Preferences,
    /// The region sets sensitive content for teens by default, which is
    /// asked of the region, not the account's age (`isSettingTeenByDefault`).
    pub region_default: bool,
    /// Discord says the account is not an adult's (`nsfwAllowed === false`).
    pub underage: bool,
    /// The newest scan whose result is trusted ([`valid_scan_version`]).
    pub valid_scan: i64,
}

impl Rules {
    pub fn new(preferences: Preferences, gates: &Gates, account: &Account, self_harm_scans: bool) -> Rules {
        Rules {
            preferences,
            region_default: gates.sensitive_by_default,
            underage: account.nsfw_set && !account.nsfw_allowed,
            valid_scan: valid_scan_version(gates.explicit_content_scan_version, self_harm_scans),
        }
    }

    /// What `harm` is set to in `place`, defaults included. The client's
    /// self-harm setting is a constant: it ignores the account's.
    pub fn redaction(&self, harm: Harm, place: Place) -> Redaction {
        let set = match harm {
            Harm::Explicit => self.preferences.explicit.at(place),
            Harm::Gore => self.preferences.gore.at(place),
            Harm::SelfHarm => return Redaction::Unset,
        };
        if set != Redaction::Unset {
            return set;
        }
        if self.region_default {
            return if place == Place::NonFriendDm { Redaction::Block } else { Redaction::Blur };
        }
        match harm {
            Harm::Explicit => self.explicit_default(place),
            _ => self.gore_default(place),
        }
    }

    /// `E` in 632119, past the settings and the region.
    fn explicit_default(&self, place: Place) -> Redaction {
        use DmFilter::*;
        use Redaction::*;
        let filter = self.preferences.dm_filter;
        match (self.underage, place) {
            (_, Place::Guild) => {
                if self.underage {
                    Blur
                } else {
                    Show
                }
            }
            (false, Place::FriendDm) => {
                if filter == FriendsAndNonFriends {
                    Block
                } else {
                    Show
                }
            }
            (false, Place::NonFriendDm) => {
                if filter == Disabled {
                    Show
                } else {
                    Block
                }
            }
            (true, Place::FriendDm) => {
                if filter == FriendsAndNonFriends {
                    Block
                } else {
                    Blur
                }
            }
            (true, Place::NonFriendDm) => {
                if filter == Disabled {
                    Blur
                } else {
                    Block
                }
            }
        }
    }

    /// `c` in 444802, past the settings and the region.
    fn gore_default(&self, place: Place) -> Redaction {
        use Redaction::*;
        match (self.underage, place) {
            (_, Place::NonFriendDm) => Block,
            (true, _) => Blur,
            (false, _) => Show,
        }
    }

    /// The harms covered for messages in `place` (`m`); none for our own.
    pub fn harms(&self, place: Option<Place>) -> Harms {
        let Some(place) = place else { return Harms(0) };
        Harms(Harm::ALL.into_iter().filter(|&h| self.redaction(h, place).covers()).fold(0, |bits, h| bits | h.bit()))
    }

    /// Why media of `place` is covered, if it is: flagged for a harm that is
    /// covered there, or not scanned yet while some harm is (`b` then `$V`).
    /// `pending_ok` is false for what the client never covers for want of a
    /// scan: a bot's embeds, and ones not yet sized.
    pub fn cover(&self, harms: Harms, scan: Scan, pending_ok: bool) -> Option<Cover> {
        if let Some(harm) = harms.flagged(scan.flags) {
            return Some(Cover::Flagged(harm));
        }
        (pending_ok && self.awaits_scan(harms, scan)).then_some(Cover::Pending)
    }

    /// `$V`/`y` in 282108: some covered harm has no flag set, and the scan
    /// is missing or, for gore and self-harm, older than the one trusted.
    /// A version of -1 means the scan was skipped for good.
    fn awaits_scan(&self, harms: Harms, scan: Scan) -> bool {
        let unflagged = Harm::ALL.into_iter().any(|h| harms.contains(h) && scan.flags & h.flag() == 0);
        if !unflagged || scan.version == Some(-1) {
            return false;
        }
        let newer = harms.contains(Harm::Gore) || harms.contains(Harm::SelfHarm);
        match scan.version {
            None => true,
            Some(version) => newer && version < self.valid_scan,
        }
    }
}

/// What Discord says of a media item's scan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub flags: u64,
    /// `content_scan_version`; none if it was never scanned.
    pub version: Option<i64>,
}

/// Why a media item is covered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cover {
    /// Flagged for this harm: the cover says so and offers Mark as Not
    /// Sensitive.
    Flagged(Harm),
    /// Not scanned yet: covered without a word (`POTENTIAL_EXPLICIT_CONTENT`).
    Pending,
}

/// `validContentScanVersion` (705833): READY's version, 5 and up only for
/// accounts in the self-harm scan experiment, else at most 4.
pub fn valid_scan_version(ready: Option<i64>, self_harm_scans: bool) -> i64 {
    if self_harm_scans { ready.unwrap_or(5) } else { ready.unwrap_or(4).min(4) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPLICIT: u64 = 1 << 4;
    const GORE: u64 = 1 << 6;
    const SELF_HARM: u64 = 1 << 7;
    const ME: Snowflake = Snowflake(1);

    fn rules() -> Rules {
        Rules { valid_scan: 4, ..Rules::default() }
    }

    fn explicit(rules: &Rules, place: Place) -> Redaction {
        rules.redaction(Harm::Explicit, place)
    }

    fn gore(rules: &Rules, place: Place) -> Redaction {
        rules.redaction(Harm::Gore, place)
    }

    #[test]
    fn an_adult_sees_servers_and_is_covered_in_dms_by_the_old_filter() {
        use DmFilter::*;
        use Redaction::*;
        let with = |dm_filter| Rules { preferences: Preferences { dm_filter, ..Preferences::default() }, ..rules() };
        for (filter, friend, stranger) in
            [(Disabled, Show, Show), (NonFriends, Show, Block), (FriendsAndNonFriends, Block, Block)]
        {
            let rules = with(filter);
            assert_eq!(explicit(&rules, Place::Guild), Show, "{filter:?}");
            assert_eq!(explicit(&rules, Place::FriendDm), friend, "{filter:?}");
            assert_eq!(explicit(&rules, Place::NonFriendDm), stranger, "{filter:?}");
        }
    }

    #[test]
    fn the_old_filter_is_non_friends_until_the_account_chose() {
        assert_eq!(Preferences::default().dm_filter, DmFilter::NonFriends);
        assert_eq!(explicit(&rules(), Place::NonFriendDm), Redaction::Block);
        assert_eq!(explicit(&rules(), Place::FriendDm), Redaction::Show);
    }

    #[test]
    fn an_account_that_is_not_an_adults_is_blurred_where_an_adult_sees() {
        use DmFilter::*;
        use Redaction::*;
        let underage = |dm_filter| Rules {
            underage: true,
            preferences: Preferences { dm_filter, ..Preferences::default() },
            ..rules()
        };
        assert_eq!(explicit(&underage(NonFriends), Place::Guild), Blur);
        assert_eq!(explicit(&underage(Disabled), Place::FriendDm), Blur);
        assert_eq!(explicit(&underage(Disabled), Place::NonFriendDm), Blur);
        assert_eq!(explicit(&underage(NonFriends), Place::NonFriendDm), Block);
        assert_eq!(explicit(&underage(FriendsAndNonFriends), Place::FriendDm), Block);
        assert_eq!(gore(&underage(NonFriends), Place::Guild), Blur);
        assert_eq!(gore(&underage(NonFriends), Place::FriendDm), Blur);
        assert_eq!(gore(&underage(NonFriends), Place::NonFriendDm), Block);
    }

    #[test]
    fn gore_ignores_the_old_filter() {
        let adult =
            Rules { preferences: Preferences { dm_filter: DmFilter::Disabled, ..Default::default() }, ..rules() };
        assert_eq!(gore(&adult, Place::Guild), Redaction::Show);
        assert_eq!(gore(&adult, Place::FriendDm), Redaction::Show);
        assert_eq!(gore(&adult, Place::NonFriendDm), Redaction::Block);
    }

    #[test]
    fn a_region_that_defaults_teens_to_sensitive_content_overrides_age() {
        use Redaction::*;
        let region = Rules { region_default: true, ..rules() };
        for harm in [Harm::Explicit, Harm::Gore] {
            assert_eq!(region.redaction(harm, Place::Guild), Blur, "{harm:?}");
            assert_eq!(region.redaction(harm, Place::FriendDm), Blur, "{harm:?}");
            assert_eq!(region.redaction(harm, Place::NonFriendDm), Block, "{harm:?}");
        }
        let underage = Rules { underage: true, ..region };
        assert_eq!(explicit(&underage, Place::Guild), Blur);
    }

    #[test]
    fn what_the_account_chose_beats_every_default() {
        use Redaction::*;
        let chose = |explicit: Redaction, gore: Redaction| Rules {
            region_default: true,
            underage: true,
            preferences: Preferences {
                explicit: Redactions { guilds: explicit, friend_dm: explicit, non_friend_dm: explicit },
                gore: Redactions { guilds: gore, friend_dm: gore, non_friend_dm: gore },
                ..Preferences::default()
            },
            ..rules()
        };
        let shown = chose(Show, Show);
        assert_eq!(explicit(&shown, Place::NonFriendDm), Show);
        assert_eq!(gore(&shown, Place::NonFriendDm), Show);
        let adult = Rules { region_default: false, underage: false, ..chose(Blur, Block) };
        assert_eq!(explicit(&adult, Place::Guild), Blur);
        assert_eq!(gore(&adult, Place::Guild), Block);
    }

    #[test]
    fn the_settings_are_per_place() {
        use Redaction::*;
        let rules = Rules {
            preferences: Preferences {
                explicit: Redactions { guilds: Blur, friend_dm: Show, non_friend_dm: Unset },
                ..Preferences::default()
            },
            ..rules()
        };
        assert_eq!(explicit(&rules, Place::Guild), Blur);
        assert_eq!(explicit(&rules, Place::FriendDm), Show);
        assert_eq!(explicit(&rules, Place::NonFriendDm), Block, "unset: the default");
    }

    #[test]
    fn self_harm_follows_no_setting() {
        let teen = Rules { region_default: true, underage: true, ..rules() };
        for place in [Place::Guild, Place::FriendDm, Place::NonFriendDm] {
            assert_eq!(teen.redaction(Harm::SelfHarm, place), Redaction::Unset);
            assert!(!teen.harms(Some(place)).contains(Harm::SelfHarm));
        }
    }

    #[test]
    fn show_and_unset_cover_nothing_blur_and_block_cover() {
        assert!(!Redaction::Show.covers() && !Redaction::Unset.covers());
        assert!(Redaction::Blur.covers() && Redaction::Block.covers());
    }

    #[test]
    fn the_harms_covered_in_a_place() {
        let adult = rules();
        assert!(adult.harms(Some(Place::Guild)).is_empty());
        let stranger = adult.harms(Some(Place::NonFriendDm));
        assert!(stranger.contains(Harm::Explicit) && stranger.contains(Harm::Gore));
        assert!(!stranger.contains(Harm::SelfHarm));
        assert!(adult.harms(None).is_empty(), "our own messages");
    }

    #[test]
    fn a_place_is_the_server_or_a_dm_by_friendship_and_never_our_own() {
        let stranger = Snowflake(2);
        assert_eq!(Place::of(true, false, Some(stranger), ME, false), Some(Place::Guild));
        assert_eq!(Place::of(true, false, Some(stranger), ME, true), Some(Place::Guild));
        assert_eq!(Place::of(true, true, Some(stranger), ME, true), Some(Place::FriendDm));
        assert_eq!(Place::of(true, true, Some(stranger), ME, false), Some(Place::NonFriendDm));
        assert_eq!(Place::of(true, true, None, ME, false), Some(Place::NonFriendDm));
        assert_eq!(Place::of(true, true, Some(ME), ME, false), None);
        assert_eq!(Place::of(false, true, Some(stranger), ME, false), None);
    }

    #[test]
    fn media_flagged_for_a_covered_harm_is_covered_for_it() {
        let rules = rules();
        let harms = rules.harms(Some(Place::NonFriendDm));
        let scanned = |flags| Scan { flags, version: Some(4) };
        assert_eq!(rules.cover(harms, scanned(EXPLICIT), true), Some(Cover::Flagged(Harm::Explicit)));
        assert_eq!(rules.cover(harms, scanned(GORE), true), Some(Cover::Flagged(Harm::Gore)));
        assert_eq!(rules.cover(harms, scanned(GORE | EXPLICIT), true), Some(Cover::Flagged(Harm::Explicit)));
        assert_eq!(rules.cover(harms, scanned(0), true), None);
        assert_eq!(rules.cover(harms, scanned(1 << 1), true), None, "other flags");
        assert_eq!(rules.cover(harms, scanned(SELF_HARM), true), None, "self-harm is covered nowhere");
    }

    #[test]
    fn flagged_media_is_shown_where_its_harm_is() {
        let rules = rules();
        let servers = rules.harms(Some(Place::Guild));
        assert_eq!(rules.cover(servers, Scan { flags: EXPLICIT, version: Some(4) }, true), None);
        assert_eq!(rules.cover(Harms::default(), Scan { flags: EXPLICIT, version: None }, true), None);
    }

    #[test]
    fn media_not_scanned_yet_is_covered_while_some_harm_is() {
        let rules = rules();
        let harms = rules.harms(Some(Place::NonFriendDm));
        assert_eq!(rules.cover(harms, Scan::default(), true), Some(Cover::Pending));
        assert_eq!(rules.cover(harms, Scan::default(), false), None);
        assert_eq!(rules.cover(rules.harms(Some(Place::Guild)), Scan::default(), true), None);
    }

    #[test]
    fn a_scan_skipped_for_good_is_never_waited_for() {
        let rules = rules();
        let harms = rules.harms(Some(Place::NonFriendDm));
        assert_eq!(rules.cover(harms, Scan { flags: 0, version: Some(-1) }, true), None);
    }

    #[test]
    fn a_scan_older_than_the_trusted_one_is_waited_for_only_where_gore_is_covered() {
        let rules = Rules { valid_scan: 5, ..rules() };
        let (stale, current) = (Scan { flags: 0, version: Some(4) }, Scan { flags: 0, version: Some(5) });
        let both = rules.harms(Some(Place::NonFriendDm));
        assert_eq!(rules.cover(both, stale, true), Some(Cover::Pending));
        assert_eq!(rules.cover(both, current, true), None);
        let explicit_only = Harms(Harm::Explicit.bit());
        assert_eq!(rules.cover(explicit_only, stale, true), None, "only gore and self-harm are rescanned");
    }

    #[test]
    fn media_scanned_clean_of_one_harm_but_flagged_for_another_is_covered_for_it() {
        let rules = rules();
        let harms = rules.harms(Some(Place::NonFriendDm));
        let scan = Scan { flags: GORE, version: None };
        assert_eq!(rules.cover(harms, scan, true), Some(Cover::Flagged(Harm::Gore)));
    }

    #[test]
    fn the_trusted_scan_version() {
        assert_eq!(valid_scan_version(None, false), 4);
        assert_eq!(valid_scan_version(Some(5), false), 4);
        assert_eq!(valid_scan_version(Some(3), false), 3);
        assert_eq!(valid_scan_version(None, true), 5);
        assert_eq!(valid_scan_version(Some(6), true), 6);
    }

    #[test]
    fn the_rules_read_the_account_and_the_region() {
        let gates = Gates { sensitive_by_default: true, explicit_content_scan_version: Some(2), ..Gates::default() };
        let account = Account { nsfw_set: true, nsfw_allowed: false, ..Account::default() };
        let rules = Rules::new(Preferences::default(), &gates, &account, false);
        assert!(rules.region_default && rules.underage);
        assert_eq!(rules.valid_scan, 2);
        let unknown = Account { nsfw_set: false, nsfw_allowed: false, ..Account::default() };
        assert!(!Rules::new(Preferences::default(), &gates, &unknown, false).underage, "only an explicit false");
    }
}
