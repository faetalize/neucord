//! Restricted hours: a schedule a parent sets through Family Center for a
//! teen's account (`restricted_schedule` on the user object, class `kX` in
//! 244284). While one of its rules is active the client holds every desktop
//! notification and notification sound (`tE` and the message handler in
//! 534765) and covers the app with a "come back later" screen.

use serde::Deserialize;
use serde_json::Value;

/// One rule: a window that starts on each of its days at `start` and runs to
/// `end`, into the next day when `end` is not after `start`.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    /// Minutes after midnight.
    start: u32,
    end: u32,
    /// The days it starts on, 1 Monday to 7 Sunday (the client's day enum).
    days: Vec<u8>,
    enabled: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RestrictedSchedule {
    rules: Vec<Rule>,
}

/// A `TimeOfDay`; protobuf JSON leaves out what is zero.
#[derive(Default, Deserialize)]
struct Time {
    #[serde(default)]
    hours: u32,
    #[serde(default)]
    minutes: u32,
}

impl Time {
    fn minutes(&self) -> u32 {
        60 * self.hours + self.minutes
    }
}

#[derive(Deserialize)]
struct RawRule {
    start_time: Option<Time>,
    end_time: Option<Time>,
    #[serde(default)]
    days: Vec<u8>,
    #[serde(default)]
    enabled: bool,
}

impl Rule {
    /// `isActiveAt` (244284); `day` is 1 Monday to 7 Sunday, `minute` the
    /// minutes since midnight. A rule with no times, no days or switched off
    /// is never active.
    fn active_at(&self, day: u8, minute: u32) -> bool {
        if self.days.is_empty() || !self.enabled {
            return false;
        }
        let overnight = self.start > self.end;
        if self.days.contains(&day) && (minute >= self.start && (overnight || minute < self.end)) {
            return true;
        }
        // The night that began the day before runs on until `end`.
        let yesterday = if day == 1 { 7 } else { day - 1 };
        overnight && self.days.contains(&yesterday) && minute < self.end
    }
}

impl RestrictedSchedule {
    /// From the user object's `restricted_schedule` (`fromServer`, 244284):
    /// `{rules: [{start_time, end_time, days, enabled}]}`. `None` when there
    /// is none, which is the case for nearly every account.
    pub fn from_user(user: &Value) -> Option<Self> {
        let rules = user["restricted_schedule"]["rules"].as_array()?;
        Some(RestrictedSchedule {
            rules: rules
                .iter()
                .filter_map(|raw| RawRule::deserialize(raw).ok())
                .filter_map(|raw| {
                    // A rule without both times never applies (`isActiveAt`).
                    Some(Rule {
                        start: raw.start_time?.minutes(),
                        end: raw.end_time?.minutes(),
                        days: raw.days,
                        enabled: raw.enabled,
                    })
                })
                .collect(),
        })
    }

    /// `isInRestrictedHours` (244284) at a local time: `day` 1 Monday to 7
    /// Sunday and `minute` since local midnight.
    pub fn is_active(&self, day: u8, minute: u32) -> bool {
        self.rules.iter().any(|rule| rule.active_at(day, minute))
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn at(hours: u32, minutes: u32) -> u32 {
        60 * hours + minutes
    }

    /// The shape the client reads: times as `TimeOfDay`, days as the enum.
    fn schedule(rules: Value) -> RestrictedSchedule {
        RestrictedSchedule::from_user(&json!({"restricted_schedule": {"rules": rules}})).unwrap()
    }

    #[test]
    fn a_user_without_a_schedule_has_none() {
        assert_eq!(RestrictedSchedule::from_user(&json!({"id": "1"})), None);
        assert_eq!(RestrictedSchedule::from_user(&json!({"restricted_schedule": null})), None);
        let empty = schedule(json!([]));
        assert!(!empty.is_active(1, at(12, 0)), "no rules, no hours");
    }

    #[test]
    fn a_same_day_rule_holds_from_its_start_up_to_its_end() {
        let s = schedule(json!([{
            "start_time": {"hours": 9, "minutes": 30, "seconds": 0, "nanos": 0},
            "end_time": {"hours": 15},
            "days": [2, 4], "enabled": true,
        }]));
        assert!(!s.is_active(2, at(9, 29)));
        assert!(s.is_active(2, at(9, 30)));
        assert!(s.is_active(2, at(14, 59)));
        assert!(!s.is_active(2, at(15, 0)), "the end is outside");
        assert!(!s.is_active(3, at(10, 0)), "not a listed day");
        assert!(s.is_active(4, at(10, 0)));
    }

    #[test]
    fn an_overnight_rule_runs_into_the_next_day() {
        // Fridays and Sundays, 22:00 to 07:00.
        let s =
            schedule(json!([{"start_time": {"hours": 22}, "end_time": {"hours": 7}, "days": [5, 7], "enabled": true}]));
        assert!(!s.is_active(5, at(21, 59)));
        assert!(s.is_active(5, at(22, 0)));
        assert!(s.is_active(5, at(23, 59)));
        assert!(s.is_active(6, at(0, 0)), "Saturday's early hours belong to Friday's night");
        assert!(s.is_active(6, at(6, 59)));
        assert!(!s.is_active(6, at(7, 0)));
        assert!(!s.is_active(6, at(22, 0)), "Saturday night wasn't listed");
        // Sunday's night ends Monday: the week wraps.
        assert!(s.is_active(1, at(3, 0)));
        assert!(!s.is_active(5, at(3, 0)), "Thursday night wasn't listed");
    }

    #[test]
    fn a_rule_that_is_off_or_incomplete_never_holds() {
        let s = schedule(json!([
            {"start_time": {"hours": 0}, "end_time": {"hours": 23}, "days": [1, 2, 3, 4, 5, 6, 7], "enabled": false},
            {"start_time": {"hours": 0}, "end_time": {"hours": 23}, "days": [], "enabled": true},
            {"end_time": {"hours": 23}, "days": [1], "enabled": true},
            {"start_time": {"hours": 0}, "days": [1], "enabled": true},
        ]));
        assert!(!s.is_active(1, at(12, 0)));
    }

    #[test]
    fn any_rule_is_enough() {
        let s = schedule(json!([
            {"start_time": {"hours": 8}, "end_time": {"hours": 9}, "days": [1], "enabled": true},
            {"start_time": {"hours": 20}, "end_time": {"hours": 21}, "days": [1], "enabled": true},
        ]));
        assert!(s.is_active(1, at(8, 30)) && s.is_active(1, at(20, 30)) && !s.is_active(1, at(12, 0)));
    }
}
