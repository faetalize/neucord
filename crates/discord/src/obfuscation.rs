//! Discord hides the names of server channels the account may not see. What
//! the client does with it:
//!
//! - It asks for the hiding in IDENTIFY, always: `capabilities` has bit 15
//!   (1767421 against 1734653 without), in `_doIdentify` in 868 and in the
//!   fast-connect identify (785564) alike.
//! - A channel hidden this way carries the flag [`FLAG`] and is called "No
//!   Access" wherever it is named (`isObfuscated` in 95701, `m1` in 47167).
//!   Nothing else of it is special: whether it shows follows the permissions
//!   it comes with, a link to it goes nowhere (232492), and search does not
//!   offer to look in it (586944). When Discord gives the account access
//!   it sends the channel again, whole.

/// Channel flag `OBFUSCATED` (`ChannelFlags`, 746080).
pub const FLAG: u64 = 1 << 17;

/// What the client calls a hidden channel.
pub const NO_ACCESS: &str = "No Access";

/// The capabilities IDENTIFY sends, as `_doIdentify` has them: the client's
/// bits, bit 15 for hidden channels among them.
pub const CAPABILITIES: u64 = 1_767_421;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_capabilities_ask_for_hidden_channels() {
        assert_eq!(CAPABILITIES, 1_734_653 | 1 << 15);
    }
}
