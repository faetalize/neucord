//! What we know of other people's presence, kept for everyone the gateway
//! tells us of, as the client's `PresenceStore` does (290863): friends' and
//! the people of the member lists we subscribed to, whoever else they are.
//! One person can come with several scopes (the account's own, and every
//! server they are in); the one heard from last is what shows.
//!
//! Offline isn't kept: the client drops a person once every scope says
//! offline, and a missing one reads as offline.

use std::collections::HashMap;

use crate::Snowflake;
use crate::relationships::Presence;

#[derive(Clone, Debug)]
struct Scoped {
    /// `None` is the account's own scope: friends, and group DMs.
    guild: Option<Snowflake>,
    presence: Presence,
    /// Which update it was, newest highest, for picking the one that shows.
    heard: u64,
}

#[derive(Clone, Debug, Default)]
pub struct Presences {
    /// Online in at least one scope. A person is in a few scopes at most, so
    /// a short list per person is leaner than a map.
    by_user: HashMap<Snowflake, Vec<Scoped>>,
    heard: u64,
}

impl Presences {
    /// Record what `guild` (`None`: the account) says of `user`. Whether what
    /// shows of them changed.
    pub fn set(&mut self, guild: Option<Snowflake>, user: Snowflake, presence: Presence) -> bool {
        let before = self.get(user).cloned();
        let scopes = self.by_user.entry(user).or_default();
        scopes.retain(|s| s.guild != guild);
        if presence.status != crate::relationships::Status::Offline {
            self.heard += 1;
            // Most people have one scope: don't let the list round up to four.
            scopes.reserve_exact(1);
            scopes.push(Scoped { guild, presence, heard: self.heard });
        }
        if scopes.is_empty() {
            self.by_user.remove(&user);
        }
        self.get(user) != before.as_ref()
    }

    /// What shows of `user`: the newest presence of theirs that isn't offline.
    pub fn get(&self, user: Snowflake) -> Option<&Presence> {
        self.by_user.get(&user)?.iter().max_by_key(|s| s.heard).map(|s| &s.presence)
    }

    /// A server was left or lost: everything it said goes.
    pub fn forget_guild(&mut self, guild: Snowflake) {
        self.by_user.retain(|_, scopes| {
            scopes.retain(|s| s.guild != Some(guild));
            !scopes.is_empty()
        });
    }

    /// A member left a server (`GUILD_MEMBER_REMOVE`). Whether what shows of
    /// them changed.
    pub fn forget_member(&mut self, guild: Snowflake, user: Snowflake) -> bool {
        self.set(Some(guild), user, Presence::default())
    }

    /// Everyone not offline anywhere.
    pub fn len(&self) -> usize {
        self.by_user.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_user.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relationships::Status;

    fn status(status: Status) -> Presence {
        Presence { status, text: None, mobile: false }
    }

    const SERVER: Option<Snowflake> = Some(Snowflake(10));
    const OTHER: Option<Snowflake> = Some(Snowflake(11));
    const USER: Snowflake = Snowflake(1);

    #[test]
    fn anyone_is_kept_and_offline_is_nobody() {
        let mut presences = Presences::default();
        assert!(presences.set(SERVER, USER, status(Status::Idle)), "a stranger's is kept");
        assert_eq!(presences.get(USER).map(|p| p.status), Some(Status::Idle));
        assert!(!presences.set(SERVER, USER, status(Status::Idle)), "unchanged");
        assert!(presences.set(SERVER, USER, Presence::default()));
        assert!(presences.get(USER).is_none() && presences.is_empty(), "offline takes up nothing");
        assert!(!presences.set(SERVER, USER, Presence::default()), "offline of the unknown changes nothing");
    }

    #[test]
    fn the_newest_scope_shows_and_a_scope_going_offline_leaves_the_others() {
        let mut presences = Presences::default();
        presences.set(None, USER, status(Status::Online));
        presences.set(SERVER, USER, status(Status::DoNotDisturb));
        assert_eq!(presences.get(USER).map(|p| p.status), Some(Status::DoNotDisturb), "heard last");
        presences.set(OTHER, USER, status(Status::Idle));
        assert_eq!(presences.get(USER).map(|p| p.status), Some(Status::Idle));
        assert!(presences.set(OTHER, USER, Presence::default()));
        assert_eq!(presences.get(USER).map(|p| p.status), Some(Status::DoNotDisturb), "back to the one before");
        presences.set(SERVER, USER, Presence::default());
        assert_eq!(presences.get(USER).map(|p| p.status), Some(Status::Online), "the account's own is left");
        assert_eq!(presences.len(), 1);
    }

    #[test]
    fn leaving_a_server_takes_its_presences_away() {
        let mut presences = Presences::default();
        presences.set(SERVER, USER, status(Status::Online));
        presences.set(SERVER, Snowflake(2), status(Status::Online));
        presences.set(OTHER, Snowflake(2), status(Status::Idle));
        presences.forget_guild(Snowflake(10));
        assert!(presences.get(USER).is_none());
        assert_eq!(presences.get(Snowflake(2)).map(|p| p.status), Some(Status::Idle));
        assert!(presences.forget_member(Snowflake(11), Snowflake(2)));
        assert!(presences.is_empty());
    }
}
