//! What Clyde says in a channel when Discord refuses something we sent
//! there with a code and no message of its own (`sendClydeError` and its
//! table in 138399): the words for each code, and for any other the
//! undeliverable-message text.

use crate::{Channel, ChannelKind};

/// Where it went wrong, as far as the words depend on it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Where {
    /// A forum or media channel, or a post in one.
    pub forum: bool,
    /// A DM with exactly one other person, who is a bot.
    pub bot_dm: bool,
    /// The channel's slowmode, in seconds.
    pub slowmode: u32,
}

impl Where {
    /// What the words read of `channel`, with `forum` whether it is a forum
    /// post or a forum.
    pub fn of(channel: &Channel, forum: bool) -> Self {
        Where {
            forum: forum || channel.kind.is_forum_like(),
            bot_dm: channel.kind == ChannelKind::Dm && channel.recipients.len() == 1 && channel.recipients[0].bot,
            slowmode: channel.rate_limit_per_user,
        }
    }
}

/// The help article the undeliverable texts point to (`DM_COULD_NOT_BE_DELIVERED`).
const NOT_DELIVERED: &str = "https://support.discord.com/hc/en-us/articles/360060145013";
/// Where harmful links are explained.
const HARMFUL_LINKS: &str = "https://dis.gd/phish";
/// The total size of a message's files, in megabytes, the client's text names.
const MAX_TOTAL_MB: u32 = 500;

fn undeliverable() -> String {
    format!("Your message could not be delivered.\n\nYou can see the full list of reasons here: {NOT_DELIVERED}")
}

/// Clyde's words for Discord's refusal `code`, from the client's table, else
/// for the undeliverable message.
pub fn send_failure(code: u64, here: &Where) -> String {
    match code {
        20016 => {
            let s = here.slowmode;
            let unit = if s == 1 { "second" } else { "seconds" };
            format!("Slowmode is enabled. Members can send one message every {s} {unit}.")
        }
        50278 if here.bot_dm => undeliverable(),
        50278 => format!(
            "Your message could not be delivered. Send a friend request first and start the conversation once they've accepted.\n\nHere is the full list of reasons why your message could not be delivered: {NOT_DELIVERED}"
        ),
        50007 => undeliverable(),
        160006 if here.forum => {
            "Your message could not be sent because this server has reached the maximum number of posts. Free up space by archiving a post.".into()
        }
        160006 => {
            "Your message could not be sent because this server has reached the maximum number of threads. Free up space by archiving a thread.".into()
        }
        160007 => {
            "Your message could not be sent because this server has reached the maximum number of announcement threads. Free up space by archiving a thread.".into()
        }
        240000 => format!(
            "Your message could not be delivered because this message contains a link blocked by Discord. You can learn more here: {HARMFUL_LINKS}."
        ),
        240001 => "Your message could not be delivered because this message contains a link blocked by Discord.".into(),
        40002 => "You need to verify your email or phone before you can send messages here.".into(),
        400005 => "Server is too busy, please try again later.".into(),
        40003 => "You are sending too many new direct messages. Hold off for a few minutes before trying to message this person again.".into(),
        40004 => "Sending messages to this channel has been temporarily disabled. Try again in a bit.".into(),
        50251 => "This user does not allow messages from Game Friends in Discord, try sending them a message in-game instead!".into(),
        50252 => "This user cannot receive messages while offline.".into(),
        30053 => format!("Your attachment(s) are too large. Maximum total size is {MAX_TOTAL_MB} MB."),
        50146 => "We couldn't find your file upload. Try removing the attachment and adding it again.".into(),
        50013 => "You don't have permission to do that here.".into(),
        _ => undeliverable(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clyde_says_what_the_clients_table_does() {
        let any = Where::default();
        assert!(
            send_failure(50007, &any).starts_with("Your message could not be delivered.\n\nYou can see the full list")
        );
        assert_eq!(send_failure(9999, &any), send_failure(50007, &any), "an unknown code gets the generic text");
        assert_eq!(
            send_failure(20016, &Where { slowmode: 1, ..any }),
            "Slowmode is enabled. Members can send one message every 1 second."
        );
        assert!(send_failure(20016, &Where { slowmode: 30, ..any }).ends_with("every 30 seconds."));
        assert!(send_failure(50278, &any).contains("Send a friend request first"));
        assert_eq!(
            send_failure(50278, &Where { bot_dm: true, ..any }),
            send_failure(50007, &any),
            "a bot isn't a friend"
        );
        assert!(send_failure(160006, &Where { forum: true, ..any }).contains("number of posts"));
        assert!(send_failure(160006, &any).contains("number of threads"));
        assert!(send_failure(240000, &any).ends_with("https://dis.gd/phish."));
        assert_eq!(send_failure(50013, &any), "You don't have permission to do that here.");
        assert!(send_failure(30053, &any).contains("500 MB"));
    }
}
