//! What a server asks of us before we can talk there: its verification level
//! (a verified email, then an older account, then time in the server, then a
//! verified phone), its verification role, and membership screening.
//!
//! The official client checks this itself (`GuildVerificationStore`) and
//! shows what's missing in place of the message box, so it never sends what
//! the server would refuse. [`gate`] is that check, in the order the client
//! tells the user about it.

use serde_json::Value;

use crate::{Guild, Snowflake};

/// How long an account must exist before a Medium server lets it talk.
pub const ACCOUNT_AGE_MINUTES: i64 = 5;
/// How long we must have been in a High server before it lets us talk.
pub const MEMBER_AGE_MINUTES: i64 = 10;

/// Member flag: a moderator let this member skip verification.
const BYPASSES_VERIFICATION: u64 = 1 << 2;

/// Members who joined before onboarding existed keep the role exemption in
/// servers that ever enabled it. The client reads this date in local time;
/// UTC is near enough.
const ONBOARDING_CUTOFF_MS: i64 = 1_669_939_200_000; // 2022-12-02

/// A server's `verification_level`, each step adding to the last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum VerificationLevel {
    #[default]
    None,
    /// A verified email.
    Low,
    /// Also an account older than [`ACCOUNT_AGE_MINUTES`].
    Medium,
    /// Also [`MEMBER_AGE_MINUTES`] in the server.
    High,
    /// A verified phone instead.
    VeryHigh,
}

impl VerificationLevel {
    fn from_raw(raw: u64) -> Self {
        match raw {
            0 => Self::None,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            // Anything newer is at least as strict.
            _ => Self::VeryHigh,
        }
    }
}

/// What a server asks before its members can talk, from its guild object.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rules {
    pub level: VerificationLevel,
    /// The owner skips all of it.
    pub owner: Option<Snowflake>,
    /// A role members need to talk, given out once they verify, and its name.
    pub role: Option<(Snowflake, String)>,
    /// Membership screening (`MEMBER_VERIFICATION_GATE_ENABLED`): new
    /// members are held as `pending` until they accept the server's rules.
    pub screening: bool,
    /// `GUILD_ONBOARDING_EVER_ENABLED`: roles picked during onboarding don't
    /// exempt members who joined since it existed.
    pub onboarding: bool,
    /// Roles not given by an integration, the only ones that exempt their
    /// members. A role created since we connected is missing, so it doesn't
    /// count: that errs on the side of not sending.
    pub plain_roles: Vec<Snowflake>,
}

impl Rules {
    /// From a guild object in either shape: settings at the top level, or
    /// inside `properties` (roles stay at the top level).
    pub fn from_guild(guild: &Value) -> Self {
        let props = if guild["properties"].is_object() { &guild["properties"] } else { guild };
        let roles = guild["roles"].as_array().map(Vec::as_slice).unwrap_or_default();
        let role_id = parse_id(&props["verification_role_id"]);
        let has_feature = |name: &str| {
            props["features"].as_array().is_some_and(|features| features.iter().any(|f| f.as_str() == Some(name)))
        };
        Rules {
            level: VerificationLevel::from_raw(props["verification_level"].as_u64().unwrap_or(0)),
            owner: parse_id(&props["owner_id"]),
            role: role_id.map(|id| {
                let name = roles.iter().find(|r| parse_id(&r["id"]) == Some(id)).and_then(|r| r["name"].as_str());
                (id, name.unwrap_or("verified").to_owned())
            }),
            screening: has_feature("MEMBER_VERIFICATION_GATE_ENABLED"),
            onboarding: has_feature("GUILD_ONBOARDING_EVER_ENABLED"),
            plain_roles: roles.iter().filter(|r| r["managed"] != true).filter_map(|r| parse_id(&r["id"])).collect(),
        }
    }
}

/// Our own membership of a server, as far as verification cares. Our roles
/// are [`Guild::my_roles`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Membership {
    /// When we joined, in milliseconds since 1970.
    pub joined_at: Option<i64>,
    /// Held by membership screening.
    pub pending: bool,
    pub flags: u64,
}

impl Membership {
    /// From our member object, which READY, GUILD_CREATE and
    /// GUILD_MEMBER_UPDATE carry.
    pub fn from_member(member: &Value) -> Self {
        Membership {
            joined_at: member["joined_at"].as_str().and_then(unix_ms),
            pending: member["pending"] == true,
            flags: member["flags"].as_u64().unwrap_or(0),
        }
    }
}

/// What a server's verification asks of the account. Only whether an email
/// and a phone are set is kept, never the address or the number.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Account {
    pub id: Snowflake,
    /// The email address is verified.
    pub verified: bool,
    pub email: bool,
    /// Discord only keeps a phone number once it's verified.
    pub phone: bool,
}

impl Account {
    /// From our own user object, in READY or USER_UPDATE.
    pub fn from_user(user: &Value) -> Option<Self> {
        Some(Account {
            id: parse_id(&user["id"])?,
            verified: user["verified"] == true,
            email: user["email"].is_string(),
            phone: user["phone"].is_string(),
        })
    }
}

/// Why we can't talk in a server yet, most pressing first.
#[derive(Clone, Debug, PartialEq)]
pub enum Gate {
    /// The account has neither an email nor a phone.
    Unclaimed,
    /// Membership screening holds us until we accept the server's rules.
    Screening,
    /// We lack the server's verification role (its name).
    Role(String),
    Phone,
    Email,
    /// We joined too recently; this clears at `until` (milliseconds since 1970).
    NewMember {
        until: i64,
    },
    /// The account is too new; this clears at `until`.
    NewAccount {
        until: i64,
    },
}

impl Gate {
    /// What the official client says, in place of the message box.
    pub fn text(&self) -> String {
        match self {
            Gate::Unclaimed => {
                "You must claim and verify your account before you can send messages in this server.".into()
            }
            Gate::Screening => "You must complete a few more steps before you can talk.".into(),
            Gate::Role(name) => format!("You must have the @{name} role before you can send messages in this server."),
            Gate::Phone => "You must verify your identity by phone before you can send messages in this server.".into(),
            Gate::Email => "You must verify your account before you can send messages in this server.".into(),
            Gate::NewMember { .. } => {
                format!(
                    "You must be a member for at least {MEMBER_AGE_MINUTES} minutes to send messages in this server."
                )
            }
            Gate::NewAccount { .. } => {
                format!(
                    "Your account must be at least {ACCOUNT_AGE_MINUTES} minutes old to send messages in this server."
                )
            }
        }
    }

    /// When this clears by itself, for the gates that only take time.
    pub fn until(&self) -> Option<i64> {
        match self {
            Gate::NewMember { until } | Gate::NewAccount { until } => Some(*until),
            _ => None,
        }
    }
}

/// Why `account` can't talk in `guild` at `now` (milliseconds since 1970),
/// or `None` if it can. Staff accounts skip most of this in the official
/// client; Neucord doesn't, which only ever holds back a message.
pub fn gate(guild: &Guild, account: &Account, now: i64) -> Option<Gate> {
    let rules = &guild.verification;
    let member = &guild.membership;
    let screening = rules.screening && member.pending;
    let exempt = rules.owner == Some(account.id) || member.flags & BYPASSES_VERIFICATION != 0;
    let leveled = rules.level != VerificationLevel::None && !exempt;

    let role = rules.role.as_ref().filter(|(id, _)| !exempt && !guild.my_roles.contains(id));
    // A verified phone passes every level, and so, unless the server gives
    // out a verification role or onboarding picked them, does any role.
    let checked = leveled
        && !account.phone
        && (rules.role.is_some()
            || (rules.onboarding && member.joined_at.is_some_and(|t| t >= ONBOARDING_CUTOFF_MS))
            || !guild.my_roles.iter().any(|r| rules.plain_roles.contains(r)));
    let at_least = |level| checked && rules.level >= level;
    let account_until = account.id.unix_ms() + ACCOUNT_AGE_MINUTES * 60_000;
    let member_until = member.joined_at.unwrap_or(0) + MEMBER_AGE_MINUTES * 60_000;

    if at_least(VerificationLevel::Low) && !account.email {
        Some(Gate::Unclaimed)
    } else if screening {
        Some(Gate::Screening)
    } else if let Some((_, name)) = role {
        Some(Gate::Role(name.clone()))
    } else if at_least(VerificationLevel::VeryHigh) {
        Some(Gate::Phone)
    } else if at_least(VerificationLevel::Low) && !account.verified {
        Some(Gate::Email)
    } else if at_least(VerificationLevel::High) && member_until > now {
        Some(Gate::NewMember { until: member_until })
    } else if at_least(VerificationLevel::Medium) && account_until > now {
        Some(Gate::NewAccount { until: account_until })
    } else {
        None
    }
}

fn parse_id(value: &Value) -> Option<Snowflake> {
    value.as_str()?.parse().ok().map(Snowflake)
}

pub(crate) fn unix_ms(timestamp: &str) -> Option<i64> {
    let time = glib::DateTime::from_iso8601(timestamp, Some(&glib::TimeZone::utc())).ok()?;
    Some(time.to_unix() * 1000 + i64::from(time.microsecond()) / 1000)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    const ME: Snowflake = Snowflake(1 << 32);
    /// An hour after ME was made.
    fn later() -> i64 {
        ME.unix_ms() + 3_600_000
    }

    fn account() -> Account {
        Account { id: ME, verified: true, email: true, phone: false }
    }

    fn guild(level: u64) -> Guild {
        let raw = json!({
            "id": "9", "name": "G", "verification_level": level, "owner_id": "2",
            "roles": [{"id": "9", "name": "@everyone"}, {"id": "40", "name": "Mod"},
                      {"id": "41", "name": "Booster", "managed": true}],
        });
        let mut guild = Guild::deserialize(&raw).unwrap();
        guild.membership.joined_at = Some(ME.unix_ms());
        guild
    }

    #[test]
    fn levels_add_up() {
        let unverified = Account { verified: false, ..account() };
        assert_eq!(gate(&guild(0), &unverified, later()), None);
        assert_eq!(gate(&guild(1), &unverified, later()), Some(Gate::Email));
        assert_eq!(gate(&guild(4), &account(), later()), Some(Gate::Phone));
        let unclaimed = Account { verified: false, email: false, ..account() };
        assert_eq!(gate(&guild(1), &unclaimed, later()), Some(Gate::Unclaimed));
        let phone = Account { phone: true, ..unclaimed };
        assert_eq!(gate(&guild(4), &phone, later()), None, "a verified phone passes every level");
    }

    #[test]
    fn time_gates_clear_by_themselves() {
        let made = ME.unix_ms();
        assert_eq!(gate(&guild(2), &account(), made + 60_000), Some(Gate::NewAccount { until: made + 300_000 }));
        assert_eq!(gate(&guild(2), &account(), made + 300_000), None);
        // At High, a newly joined member waits ten minutes from joining.
        let mut high = guild(3);
        high.membership.joined_at = Some(later());
        let until = later() + 600_000;
        assert_eq!(gate(&high, &account(), later()), Some(Gate::NewMember { until }));
        assert_eq!(gate(&high, &account(), later()).and_then(|g| g.until()), Some(until));
        assert_eq!(gate(&high, &account(), until), None);
    }

    #[test]
    fn who_skips_the_level() {
        let unverified = Account { verified: false, ..account() };
        let mut owned = guild(4);
        owned.verification.owner = Some(ME);
        assert_eq!(gate(&owned, &unverified, later()), None);

        let mut bypassed = guild(4);
        bypassed.membership.flags = BYPASSES_VERIFICATION;
        assert_eq!(gate(&bypassed, &unverified, later()), None);

        let mut role = guild(4);
        role.my_roles = vec![Snowflake(40)];
        assert_eq!(gate(&role, &unverified, later()), None, "any role exempts");
        role.my_roles = vec![Snowflake(41)];
        assert_eq!(gate(&role, &unverified, later()), Some(Gate::Phone), "an integration's role doesn't");
        role.my_roles = vec![Snowflake(40)];
        role.verification.onboarding = true;
        role.membership.joined_at = Some(ONBOARDING_CUTOFF_MS - 1);
        assert_eq!(gate(&role, &unverified, later()), None, "joined before onboarding");
        role.membership.joined_at = Some(ONBOARDING_CUTOFF_MS);
        assert_eq!(gate(&role, &unverified, later()), Some(Gate::Phone), "roles picked in onboarding");
    }

    #[test]
    fn verification_role() {
        let raw = json!({"id": "9", "name": "G", "verification_role_id": "42",
                         "roles": [{"id": "40", "name": "Mod"}, {"id": "42", "name": "Verified"}]});
        let mut guild = Guild::deserialize(&raw).unwrap();
        guild.my_roles = vec![Snowflake(40)];
        let unverified = Account { verified: false, ..account() };
        assert_eq!(gate(&guild, &account(), later()), Some(Gate::Role("Verified".into())));
        guild.verification.level = VerificationLevel::Low;
        assert_eq!(
            gate(&guild, &unverified, later()),
            Some(Gate::Role("Verified".into())),
            "said first, and other roles no longer exempt from the level"
        );
        guild.my_roles.push(Snowflake(42));
        assert_eq!(gate(&guild, &unverified, later()), Some(Gate::Email));
    }

    #[test]
    fn screening_holds_pending_members() {
        let raw = json!({"id": "9", "properties": {"name": "G", "owner_id": "2", "verification_level": 1,
                         "features": ["COMMUNITY", "MEMBER_VERIFICATION_GATE_ENABLED"]}});
        let mut guild = Guild::deserialize(&raw).unwrap();
        assert_eq!(guild.verification.level, VerificationLevel::Low, "read from properties");
        guild.membership = Membership::from_member(&json!({"pending": true, "joined_at": null}));
        assert_eq!(gate(&guild, &account(), later()), Some(Gate::Screening));
        guild.verification.owner = Some(ME);
        assert_eq!(gate(&guild, &account(), later()), Some(Gate::Screening), "even the owner is held");
        guild.membership.pending = false;
        assert_eq!(gate(&guild, &account(), later()), None);
    }

    #[test]
    fn parse_account_and_member() {
        let user = json!({"id": "5", "verified": true, "email": "someone@example.org", "phone": null});
        assert_eq!(
            Account::from_user(&user),
            Some(Account { id: Snowflake(5), verified: true, email: true, phone: false })
        );
        let member = Membership::from_member(&json!({"joined_at": "2022-12-02T00:00:00.250000+00:00", "flags": 4}));
        assert_eq!(member, Membership { joined_at: Some(ONBOARDING_CUTOFF_MS + 250), pending: false, flags: 4 });
    }
}
