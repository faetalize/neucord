//! What Discord holds against the account: a step it requires before the
//! account can be used again, and the account's standing.
//!
//! The official client covers itself with a verification screen for as long
//! as a step is required, so nothing it sends meanwhile can reach Discord.
//! A client that keeps going is acting where no official client can, which
//! is what gets accounts flagged (#51).

use serde::Deserialize;
use serde_json::Value;

use crate::endpoints::call;
use crate::{Error, Rest, Snowflake};

/// What a `required_action` (READY and `USER_REQUIRED_ACTION_UPDATE`) asks
/// for, as an instruction.
pub fn required_action_text(action: &str) -> &'static str {
    match action {
        "AGREEMENTS" => "Accept Discord's updated terms.",
        "REQUIRE_CAPTCHA" => "Solve a captcha.",
        "REQUIRE_VERIFIED_EMAIL" => "Verify your email address.",
        "REQUIRE_REVERIFIED_EMAIL" => "Verify your email address again.",
        "REQUIRE_VERIFIED_PHONE" => "Add a phone number and verify it.",
        "REQUIRE_REVERIFIED_PHONE" => "Verify your phone number again.",
        "REQUIRE_SAFETY_FLOWS" => "Complete Discord's safety check.",
        // Four combinations of (re)verifying either one.
        action if action.starts_with("REQUIRE_") && action.contains("_OR_") => {
            "Verify your email address or a phone number."
        }
        _ => "Complete the step Discord asks for.",
    }
}

/// The steps of Discord's standing scale, from its Account Standing page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    AllGood,
    Limited,
    VeryLimited,
    AtRisk,
    Suspended,
}

impl Level {
    fn from_state(state: u64) -> Option<Self> {
        Some(match state {
            100 => Level::AllGood,
            200 => Level::Limited,
            300 => Level::VeryLimited,
            400 => Level::AtRisk,
            500 => Level::Suspended,
            _ => return None,
        })
    }

    /// Discord's words for it: "Your account is {name}".
    pub fn name(self) -> &'static str {
        match self {
            Level::AllGood => "all good",
            Level::Limited => "limited",
            Level::VeryLimited => "very limited",
            Level::AtRisk => "at risk",
            Level::Suspended => "suspended",
        }
    }

    /// What it means, in Discord's words.
    pub fn description(self) -> &'static str {
        match self {
            Level::AllGood => "Thank you for upholding Discord's Terms of Service and Community Guidelines.",
            Level::Limited => "You may lose access to some parts of Discord if you break the rules again.",
            Level::VeryLimited => {
                "You can't use some parts of Discord. You may be suspended if you break the rules again."
            }
            Level::AtRisk => "You broke Discord's rules. You will be permanently suspended if you break them again.",
            Level::Suspended => {
                "Due to serious policy violations, your account is permanently suspended. You can no longer use Discord."
            }
        }
    }
}

/// A violation on the account, or on a server it owns.
#[derive(Clone, Debug, Deserialize)]
pub struct Violation {
    pub id: Snowflake,
    /// The rule broken, as Discord names it: "You broke the rules for {description}".
    pub description: String,
    /// When it stops counting against the account, in ISO 8601.
    pub max_expiration_time: String,
}

impl Violation {
    /// Still counting against the account at `now_unix`.
    pub fn active(&self, now_unix: i64) -> bool {
        glib::DateTime::from_iso8601(&self.max_expiration_time, Some(&glib::TimeZone::utc()))
            .map(|t| t.to_unix() > now_unix)
            // Unparseable: it was listed, so assume it counts.
            .unwrap_or(true)
    }
}

/// The account's standing and the violations behind it.
#[derive(Clone, Debug)]
pub struct Standing {
    /// `None` for a step this version doesn't know.
    pub level: Option<Level>,
    /// Newest first, expired ones included.
    pub violations: Vec<Violation>,
}

impl Standing {
    fn from_value(value: &Value) -> Self {
        let mut violations: Vec<Violation> = ["classifications", "guild_classifications"]
            .iter()
            .flat_map(|key| value[key].as_array().into_iter().flatten())
            .filter_map(|v| Violation::deserialize(v).ok())
            .collect();
        violations.sort_by_key(|v| std::cmp::Reverse(v.id));
        Standing { level: value["account_standing"]["state"].as_u64().and_then(Level::from_state), violations }
    }
}

impl Rest {
    /// What the official client's Account Standing page shows.
    pub async fn account_standing(&self) -> Result<Standing, Error> {
        let value: Value = self.request(call!(SAFETY_HUB), None).await?;
        Ok(Standing::from_value(&value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standing_from_the_safety_hub() {
        let standing = Standing::from_value(&serde_json::json!({
            "account_standing": {"state": 200},
            "classifications": [
                {"id": "10", "description": "spam", "max_expiration_time": "2026-11-01T00:00:00+00:00"},
                {"id": "30", "description": "spam", "max_expiration_time": "2026-01-01T00:00:00+00:00"},
                {"id": "20"}
            ],
            "guild_classifications": [
                {"id": "40", "description": "harassment", "max_expiration_time": "2027-01-01T00:00:00+00:00"}
            ]
        }));
        assert_eq!(standing.level, Some(Level::Limited));
        let ids: Vec<u64> = standing.violations.iter().map(|v| v.id.0).collect();
        assert_eq!(ids, [40, 30, 10], "newest first, unreadable ones dropped");
        // 2026-10-26
        let now = 1_792_972_800;
        let active: Vec<bool> = standing.violations.iter().map(|v| v.active(now)).collect();
        assert_eq!(active, [true, false, true]);
        assert_eq!(Standing::from_value(&serde_json::json!({"account_standing": {"state": 250}})).level, None);
    }

    #[test]
    fn required_actions_in_words() {
        assert_eq!(required_action_text("REQUIRE_VERIFIED_PHONE"), "Add a phone number and verify it.");
        assert_eq!(
            required_action_text("REQUIRE_REVERIFIED_EMAIL_OR_VERIFIED_PHONE"),
            "Verify your email address or a phone number."
        );
        assert_eq!(required_action_text("SOMETHING_NEW"), "Complete the step Discord asks for.");
    }
}
