//! What the sidebar says about a server channel besides its name, chosen as
//! the official client does (module 713654, the function holding
//! `AnnouncementsLockIcon`): the kind of channel, and whether it is the
//! server's rules, age-restricted, or hidden from `@everyone`.
//!
//! The client draws each combination as its own glyph, so a locked
//! announcement channel is one icon. Here the kind and the mark are apart,
//! for the UI to draw as it likes. Not copied: moderator report channels (a flag
//! icon ahead of the rules), text channels linked to a lobby, spoiler
//! channels, game invite forums, and the voice states (video, streams, people
//! connected).

use crate::{Channel, ChannelKind, Guild, permissions};

/// The kind of channel, or the rules channel, which the client draws apart
/// from the kind it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Glyph {
    Text,
    Announcement,
    Voice,
    Stage,
    Forum,
    Media,
    Rules,
}

/// What the client swaps a channel's glyph for, only one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mark {
    /// Age-restricted (`nsfw`).
    Nsfw,
    /// Hidden from `@everyone` ([`permissions::hidden_from_everyone`]).
    Locked,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Icon {
    pub glyph: Glyph,
    pub mark: Option<Mark>,
}

/// How the sidebar draws `channel`, one of `guild`'s. Categories, threads and
/// DMs have none: they are drawn some other way.
pub fn of(guild: &Guild, channel: &Channel) -> Option<Icon> {
    let glyph = match channel.kind {
        ChannelKind::Text => Glyph::Text,
        ChannelKind::Announcement => Glyph::Announcement,
        ChannelKind::Voice => Glyph::Voice,
        ChannelKind::Stage => Glyph::Stage,
        ChannelKind::Forum => Glyph::Forum,
        ChannelKind::Media => Glyph::Media,
        _ => return None,
    };
    if guild.rules_channel_id == Some(channel.id) {
        return Some(Icon { glyph: Glyph::Rules, mark: None });
    }
    // The client's stage channels take no age restriction.
    let nsfw = channel.nsfw && glyph != Glyph::Stage;
    let mark = if nsfw {
        Some(Mark::Nsfw)
    } else if permissions::hidden_from_everyone(guild, channel) {
        Some(Mark::Locked)
    } else {
        None
    };
    Some(Icon { glyph, mark })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::permissions::Permissions;

    const GUILD: u64 = 1;
    const RULES: u64 = 7;
    const VIEW: Permissions = Permissions::VIEW_CHANNEL;

    fn guild() -> Guild {
        let everyone = (VIEW | Permissions::CONNECT).0.to_string();
        serde_json::from_value(json!({
            "id": GUILD.to_string(), "name": "G", "rules_channel_id": RULES.to_string(),
            "roles": [{"id": GUILD.to_string(), "name": "@everyone", "permissions": everyone}],
        }))
        .unwrap()
    }

    fn channel(id: u64, kind: u8, extra: serde_json::Value) -> Channel {
        let mut channel = json!({"id": id.to_string(), "type": kind});
        channel.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        serde_json::from_value(channel).unwrap()
    }

    fn deny(id: u64, deny: Permissions) -> serde_json::Value {
        json!({"permission_overwrites": [{"id": id.to_string(), "type": 0, "deny": deny.0.to_string(), "allow": "0"}]})
    }

    fn icon(kind: u8, extra: serde_json::Value) -> Option<Icon> {
        of(&guild(), &channel(9, kind, extra))
    }

    fn plain(glyph: Glyph) -> Option<Icon> {
        Some(Icon { glyph, mark: None })
    }

    #[test]
    fn every_listed_kind_has_its_own_glyph() {
        let none = json!({});
        assert_eq!(icon(0, none.clone()), plain(Glyph::Text));
        assert_eq!(icon(5, none.clone()), plain(Glyph::Announcement));
        assert_eq!(icon(2, none.clone()), plain(Glyph::Voice));
        assert_eq!(icon(13, none.clone()), plain(Glyph::Stage));
        assert_eq!(icon(15, none.clone()), plain(Glyph::Forum));
        assert_eq!(icon(16, none.clone()), plain(Glyph::Media));
        for unlisted in [1, 3, 4, 10, 11, 12, 19, 99] {
            assert_eq!(icon(unlisted, none.clone()), None, "type {unlisted}");
        }
    }

    #[test]
    fn rules_channel_is_told_by_the_servers_id() {
        let guild = guild();
        assert_eq!(of(&guild, &channel(RULES, 0, json!({}))), plain(Glyph::Rules));
        // Before everything else: the client checks it ahead of the kind.
        let rules = channel(RULES, 5, json!({"nsfw": true}));
        assert_eq!(of(&guild, &rules), plain(Glyph::Rules));
        assert_eq!(icon(0, json!({})), plain(Glyph::Text), "another channel is no rules channel");
    }

    #[test]
    fn age_restricted_channels_are_marked() {
        for kind in [0, 2, 5, 15, 16] {
            assert_eq!(icon(kind, json!({"nsfw": true})).unwrap().mark, Some(Mark::Nsfw), "type {kind}");
            assert_eq!(icon(kind, json!({"nsfw": null})).unwrap().mark, None, "type {kind}: null is off");
        }
        assert_eq!(icon(13, json!({"nsfw": true})).unwrap().mark, None, "a stage takes none");
    }

    #[test]
    fn channels_everyone_cannot_view_are_locked() {
        let hidden = deny(GUILD, VIEW);
        for kind in [0, 2, 5, 13, 15, 16] {
            assert_eq!(icon(kind, hidden.clone()).unwrap().mark, Some(Mark::Locked), "type {kind}");
        }
        // Only `@everyone`'s overwrite counts, whoever else it hides the channel from.
        assert_eq!(icon(0, deny(55, VIEW)).unwrap().mark, None);
        assert_eq!(icon(0, deny(GUILD, Permissions::SEND_MESSAGES)).unwrap().mark, None);
    }

    #[test]
    fn voice_and_stage_channels_everyone_cannot_join_are_locked() {
        let no_connect = deny(GUILD, Permissions::CONNECT);
        assert_eq!(icon(2, no_connect.clone()).unwrap().mark, Some(Mark::Locked));
        assert_eq!(icon(13, no_connect.clone()).unwrap().mark, Some(Mark::Locked));
        assert_eq!(icon(0, no_connect).unwrap().mark, None, "a text channel has no Connect");
        // An allow for `@everyone` gives back what the role lacks.
        let mut guild: Guild = serde_json::from_value(json!({
            "id": GUILD.to_string(), "name": "G",
            "roles": [{"id": GUILD.to_string(), "name": "@everyone", "permissions": VIEW.0.to_string()}],
        }))
        .unwrap();
        let allowed = json!({"permission_overwrites": [
            {"id": GUILD.to_string(), "type": 0, "deny": "0", "allow": Permissions::CONNECT.0.to_string()}
        ]});
        assert_eq!(of(&guild, &channel(9, 2, json!({}))).unwrap().mark, Some(Mark::Locked), "the role lacks Connect");
        assert_eq!(of(&guild, &channel(9, 2, allowed)).unwrap().mark, None);
        guild.roles.clear();
        assert_eq!(of(&guild, &channel(9, 2, json!({}))).unwrap().mark, None, "no role: the default has it");
    }

    #[test]
    fn age_restriction_wins_over_the_lock() {
        let mut extra = deny(GUILD, VIEW);
        extra["nsfw"] = json!(true);
        assert_eq!(icon(0, extra).unwrap().mark, Some(Mark::Nsfw));
    }
}
