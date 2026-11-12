//! `2026-02-private-channel-hiding`: Discord hides the names of server
//! channels the account may not see. What the client does with it:
//!
//! - It asks for the hiding in IDENTIFY: `capabilities` has bit 15 more
//!   (1767421 against 1734653) when the experiment's config says
//!   `enableObfuscation`, which variants 1 and 2 do (variant 2 also sets
//!   `enableIntegrityCheck`, which nothing in the client reads; 3 is off).
//!   It decides from its stored assignments, which it keeps between launches,
//!   so a first launch asks for none (`_doIdentify` in 868, over `RK` in
//!   697277). [`crate::identity::Cache`] keeps them for each account, and
//!   Neucord has no fast-connect path, whose inline script reads a local flag
//!   (`private_channel_obfuscation`, 375389) instead of the store.
//! - A channel hidden this way carries the flag [`FLAG`] and is called "No
//!   Access" wherever it is named (`isObfuscated` in 95701, `m1` in 47167).
//!   Nothing else of it is special: whether it shows follows the permissions
//!   it comes with, a link to it goes nowhere (232492), and search does not
//!   offer to look in it (586944). When Discord gives the account access
//!   it sends the channel again, whole.

use serde_json::Value;

use crate::experiments::murmur3;

const EXPERIMENT: &str = "2026-02-private-channel-hiding";

/// Channel flag `OBFUSCATED` (`ChannelFlags`, 375389).
pub const FLAG: u64 = 1 << 17;

/// What the client calls a hidden channel.
pub const NO_ACCESS: &str = "No Access";

/// The capabilities IDENTIFY sends, as `_doIdentify` computes them: the
/// client's bits, and bit 15 for hidden channels.
pub fn capabilities(obfuscation: bool) -> u64 {
    if obfuscation { 1_767_421 } else { 1_734_653 }
}

/// Whether the experiment's variant hides channels.
fn hides(variant: i64) -> bool {
    matches!(variant, 1 | 2)
}

/// Whether the stored assignments (`[hash, variant, flags, …]`, as
/// [`crate::experiments::Experiments::user_cache`] keeps them) put the
/// account in the experiment, as `getConfig` decides: an assignment that only
/// makes the account eligible has no variant of its own.
pub fn assigned(assignments: &[Value]) -> bool {
    const USE_AS_ELIGIBILITY: u64 = 8;
    let hash = u64::from(murmur3(EXPERIMENT));
    assignments
        .iter()
        .find(|a| a[0].as_u64() == Some(hash))
        .filter(|a| a[2].as_u64().unwrap_or(0) & USE_AS_ELIGIBILITY == 0)
        .and_then(|a| a[1].as_i64())
        .is_some_and(hides)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn stored(variant: i64, flags: u64) -> Vec<Value> {
        vec![json!([murmur3("2099-01-other"), 1, 0, 1]), json!([murmur3(EXPERIMENT), variant, flags, 1])]
    }

    #[test]
    fn variants_one_and_two_hide_channels_and_nothing_else_does() {
        assert!(assigned(&stored(1, 0)));
        assert!(assigned(&stored(2, 0)));
        assert!(!assigned(&stored(3, 0)));
        assert!(!assigned(&stored(0, 0)));
        assert!(!assigned(&stored(1, 8)), "eligibility is not a variant");
        assert!(!assigned(&[]), "a first launch has nothing stored");
        assert!(!assigned(&[json!([murmur3("2099-01-other"), 1, 0, 1])]));
    }

    #[test]
    fn the_clients_capabilities_differ_by_bit_15() {
        assert_eq!(capabilities(false), 1_734_653);
        assert_eq!(capabilities(true), 1_734_653 | 1 << 15);
    }
}
