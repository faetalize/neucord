//! Pinning a message: what the client says when Discord refuses it
//! (`pinMessage` in 687709) and what it asks first (145530).

/// How many messages a channel can have pinned (`maxPins` in 687709).
const MAX_PINS: u32 = 250;

/// An alert: its title and its text.
#[derive(Debug, PartialEq)]
pub struct Alert {
    pub title: String,
    pub body: String,
}

fn alert(title: &str, body: impl Into<String>) -> Alert {
    Alert { title: title.to_owned(), body: body.into() }
}

/// Why pinning a message was refused, by Discord's error `code`. `private`
/// says the channel is a DM, `channel` its name, and `said` what Discord's
/// own message was, which an unlisted code shows.
pub fn refused(code: Option<u64>, private: bool, channel: &str, said: Option<&str>) -> Alert {
    const UNKNOWN_MESSAGE: u64 = 10008;
    const TOO_MANY_PINS: u64 = 30003;
    const INVALID_ACCESS: u64 = 50001;
    const INVALID_CHANNEL: u64 = 50019;
    const SYSTEM_MESSAGE: u64 = 50021;
    const THREAD_ARCHIVED: u64 = 50083;
    match code {
        None => alert("Error", "An error occurred, please try again."),
        Some(TOO_MANY_PINS) if private => alert(
            "The Pin Broke",
            format!(
                "Discord is unable to pin that message. You may have hit the limit of {MAX_PINS} pins in this channel."
            ),
        ),
        Some(TOO_MANY_PINS) => alert(
            "The Pin Broke",
            format!(
                "Discord is unable to pin that message. You may have hit the limit of {MAX_PINS} pins in channel #{channel}."
            ),
        ),
        Some(INVALID_ACCESS) => alert("No Access", "You do not have permission to pin messages in this channel."),
        Some(INVALID_CHANNEL) => alert("Invalid Channel Type", "This message cannot be pinned in this channel."),
        Some(THREAD_ARCHIVED) => {
            alert("Invalid Thread Archive State", "This message cannot be pinned in an archived thread.")
        }
        Some(SYSTEM_MESSAGE) => alert("Invalid Message Type", "This message is a system message and cannot be pinned."),
        Some(UNKNOWN_MESSAGE) => alert("Unknown Message", "The message you are trying to pin does not exist."),
        Some(_) => alert("The Pin Broke", said.unwrap_or("An error occurred, please try again.")),
    }
}

/// What the client says when unpinning failed, with a way to try again.
pub const UNPIN_FAILED: (&str, &str) =
    ("The Pin Is Stuck!", "Discord failed to unpin that message for some reason. Sorry about that.");

/// The question before pinning, unless Shift skips it (`confirmPin`, 145530):
/// its heading, text and button.
pub fn pin_question(private: bool, channel: &str) -> (&'static str, String, &'static str) {
    let text = if private {
        "Hey, just double checking that you want to pin this message to the current channel for posterity and greatness?"
            .to_owned()
    } else {
        format!(
            "Hey, just double checking that you want to pin this message to #{channel} for posterity and greatness?"
        )
    };
    ("Pin It. Pin It Good.", text, "Oh yeah. Pin it")
}

/// The question before unpinning (`confirmUnpin`): heading, text, button and
/// the hint that Shift skips it.
pub const UNPIN_QUESTION: (&str, &str, &str, &str) = (
    "Unpin Message",
    "You sure you want to remove this pinned message?",
    "Remove it please!",
    "You can hold down Shift when clicking Unpin Message to skip this confirmation.",
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_says_what_the_clients_does() {
        assert_eq!(refused(None, false, "x", None).title, "Error");
        let full = refused(Some(30003), false, "general", None);
        assert_eq!(full.title, "The Pin Broke");
        assert!(full.body.ends_with("250 pins in channel #general."));
        assert!(refused(Some(30003), true, "ignored", None).body.ends_with("250 pins in this channel."));
        assert_eq!(refused(Some(50001), false, "", None).title, "No Access");
        assert_eq!(refused(Some(50083), false, "", None).title, "Invalid Thread Archive State");
        assert_eq!(refused(Some(99999), false, "", Some("Nope.")).body, "Nope.", "Discord's own words");
        assert_eq!(refused(Some(99999), false, "", None).body, "An error occurred, please try again.");
    }

    #[test]
    fn the_question_names_the_channel_unless_it_is_a_dm() {
        assert!(pin_question(false, "general").1.contains("#general"));
        assert!(pin_question(true, "general").1.contains("the current channel"));
    }
}
