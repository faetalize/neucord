//! What READY says holds privacy settings back for the account: a parent
//! linked through Family Center, and the age rules of the account's region.
//! The settings window offers the same switches as the client does, and
//! leaves alone those the client disables or refuses.

use serde_json::Value;

/// `ageVerificationStatus` (155718): the teen and adult ones, verified or
/// inferred.
fn is_teen(status: u64) -> bool {
    matches!(status, 2 | 5)
}

fn is_adult(status: u64) -> bool {
    matches!(status, 3 | 4)
}

/// `MESSAGE_REQUEST_RESTRICTIONS` among the settings a region sets for teens
/// by default (923457).
const TEEN_MESSAGE_REQUESTS: u64 = 4;
/// `GUILD_ACTIVITY_STATUS` among them: activity is shared in large servers
/// until the account chooses (923457). The client asks only the region, not
/// the account's age (`isSettingTeenByDefault`, 454409).
const TEEN_ACTIVITY_STATUS: u64 = 8;
/// `DM_PRIVACY_SETTINGS` among the features a region age-gates (687123).
const AGE_GATED_DM_PRIVACY: u64 = 4;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gates {
    /// A parent is linked and active: who may message us and send friend
    /// requests is theirs to change (`useIsParentLinked`, 834981).
    pub parent_linked: bool,
    /// A teen where the region turns message requests on by default: they
    /// stay on (`116774`).
    pub message_requests_forced: bool,
    /// Turning message requests off needs age verification first
    /// (`953298`): the client then opens its verification flow, which Neucord
    /// has none of.
    pub message_requests_age_gated: bool,
    /// Where an account that never chose shares its activity: large servers,
    /// not none.
    pub activity_status_by_default: bool,
}

impl Gates {
    pub fn from_ready(ready: &Value) -> Gates {
        let linked = ready["linked_users"].as_array().into_iter().flatten();
        // `link_status` ACTIVE is 2 and `link_type` PARENT is 1 (191627).
        let parent_linked = linked.into_iter().any(|l| l["link_status"] == 2 && l["link_type"] == 1);
        let age = ready["user"]["age_verification_status"].as_u64().unwrap_or(0);
        let region = &ready["regional_feature_config"];
        let teen_defaults = region["teen_by_default_settings"].as_u64().unwrap_or(0);
        let age_gated = region["age_gated_features"].as_u64().unwrap_or(0);
        Gates {
            parent_linked,
            message_requests_forced: is_teen(age) && teen_defaults & TEEN_MESSAGE_REQUESTS != 0,
            message_requests_age_gated: !is_adult(age) && age_gated & AGE_GATED_DM_PRIVACY != 0,
            activity_status_by_default: teen_defaults & TEEN_ACTIVITY_STATUS != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_plain_account_has_no_gates() {
        assert_eq!(Gates::from_ready(&json!({"user": {}})), Gates::default());
        assert_eq!(Gates::from_ready(&Value::Null), Gates::default());
    }

    #[test]
    fn activity_is_shared_in_large_servers_by_default_where_the_region_says_whatever_the_age() {
        let ready = |age, teen| {
            json!({"user": {"age_verification_status": age},
                                       "regional_feature_config": {"teen_by_default_settings": teen}})
        };
        assert!(Gates::from_ready(&ready(3, 8)).activity_status_by_default, "an adult too");
        assert!(Gates::from_ready(&ready(2, 12)).activity_status_by_default);
        assert!(!Gates::from_ready(&ready(2, 4)).activity_status_by_default);
    }

    #[test]
    fn a_linked_parent_is_an_active_one() {
        let linked = |status, kind| json!({"linked_users": [{"link_status": status, "link_type": kind}]});
        assert!(Gates::from_ready(&linked(2, 1)).parent_linked);
        assert!(!Gates::from_ready(&linked(1, 1)).parent_linked, "a request is not a link");
        assert!(!Gates::from_ready(&linked(2, 2)).parent_linked, "a child's side");
    }

    #[test]
    fn a_teen_keeps_message_requests_where_the_region_says() {
        let ready = |age, teen, gated| {
            json!({"user": {"age_verification_status": age},
                   "regional_feature_config": {"teen_by_default_settings": teen, "age_gated_features": gated}})
        };
        assert!(Gates::from_ready(&ready(2, 4, 0)).message_requests_forced);
        assert!(Gates::from_ready(&ready(5, 5, 0)).message_requests_forced, "an inferred teen too");
        assert!(!Gates::from_ready(&ready(3, 4, 0)).message_requests_forced, "an adult");
        assert!(!Gates::from_ready(&ready(2, 3, 0)).message_requests_forced, "the region leaves them off");

        assert!(Gates::from_ready(&ready(1, 0, 4)).message_requests_age_gated, "unverified");
        assert!(Gates::from_ready(&ready(2, 0, 7)).message_requests_age_gated);
        assert!(!Gates::from_ready(&ready(3, 0, 4)).message_requests_age_gated, "a verified adult");
        assert!(!Gates::from_ready(&ready(4, 0, 4)).message_requests_age_gated, "an inferred adult");
        assert!(!Gates::from_ready(&ready(1, 0, 3)).message_requests_age_gated, "not a gated feature");
    }
}
