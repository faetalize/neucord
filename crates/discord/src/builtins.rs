//! The official client's own commands, which sit in the `/` suggestions
//! beside the apps': the list is `x` in 12115 (with 743361's /gif, /leave,
//! /schedule and /sticker first). The client runs them itself, never as an
//! interaction.

/// A built-in command, in the client's order (`x`). `/roll-dice` (-18,
/// behind `2026-05-dice-roll-slash-command`) is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BuiltIn {
    Gif,
    Leave,
    Schedule,
    Sticker,
    Shrug,
    Tableflip,
    Unflip,
    Tts,
    Me,
    Spoiler,
    Nick,
    Thread,
    Kick,
    Ban,
    Timeout,
    Msg,
}

/// How a built-in takes what is typed (`y$` in 392054).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    /// `BUILT_IN`: it does something.
    Action,
    /// `BUILT_IN_TEXT`: it writes the message.
    Text,
    /// `BUILT_IN_INTEGRATION`: it searches, and the pick is sent.
    Integration(Integration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Integration {
    Gif,
    Sticker,
}

impl BuiltIn {
    pub const ALL: [BuiltIn; 16] = [
        BuiltIn::Gif,
        BuiltIn::Leave,
        BuiltIn::Schedule,
        BuiltIn::Sticker,
        BuiltIn::Shrug,
        BuiltIn::Tableflip,
        BuiltIn::Unflip,
        BuiltIn::Tts,
        BuiltIn::Me,
        BuiltIn::Spoiler,
        BuiltIn::Nick,
        BuiltIn::Thread,
        BuiltIn::Kick,
        BuiltIn::Ban,
        BuiltIn::Timeout,
        BuiltIn::Msg,
    ];

    /// The client's id for it, negative so that no app's can be it; the gaps
    /// (-8, -9) are commands it dropped.
    pub fn id(self) -> &'static str {
        match self {
            BuiltIn::Gif => "-16",
            BuiltIn::Leave => "-15",
            BuiltIn::Schedule => "-19",
            BuiltIn::Sticker => "-17",
            BuiltIn::Shrug => "-1",
            BuiltIn::Tableflip => "-2",
            BuiltIn::Unflip => "-3",
            BuiltIn::Tts => "-4",
            BuiltIn::Me => "-5",
            BuiltIn::Spoiler => "-6",
            BuiltIn::Nick => "-7",
            BuiltIn::Thread => "-10",
            BuiltIn::Kick => "-11",
            BuiltIn::Ban => "-12",
            BuiltIn::Timeout => "-13",
            BuiltIn::Msg => "-14",
        }
    }

    pub fn from_id(id: &str) -> Option<BuiltIn> {
        BuiltIn::ALL.into_iter().find(|b| b.id() == id)
    }

    /// Its name after `/`, never translated.
    pub fn name(self) -> &'static str {
        match self {
            BuiltIn::Gif => "gif",
            BuiltIn::Leave => "leave",
            BuiltIn::Schedule => "schedule",
            BuiltIn::Sticker => "sticker",
            BuiltIn::Shrug => "shrug",
            BuiltIn::Tableflip => "tableflip",
            BuiltIn::Unflip => "unflip",
            BuiltIn::Tts => "tts",
            BuiltIn::Me => "me",
            BuiltIn::Spoiler => "spoiler",
            BuiltIn::Nick => "nick",
            BuiltIn::Thread => "thread",
            BuiltIn::Kick => "kick",
            BuiltIn::Ban => "ban",
            BuiltIn::Timeout => "timeout",
            BuiltIn::Msg => "msg",
        }
    }

    pub fn input(self) -> Input {
        match self {
            BuiltIn::Gif => Input::Integration(Integration::Gif),
            BuiltIn::Sticker => Input::Integration(Integration::Sticker),
            BuiltIn::Shrug | BuiltIn::Tableflip | BuiltIn::Unflip | BuiltIn::Tts | BuiltIn::Me | BuiltIn::Spoiler => {
                Input::Text
            }
            BuiltIn::Leave
            | BuiltIn::Schedule
            | BuiltIn::Nick
            | BuiltIn::Thread
            | BuiltIn::Kick
            | BuiltIn::Ban
            | BuiltIn::Timeout
            | BuiltIn::Msg => Input::Action,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_built_in_has_its_own_id() {
        for builtin in BuiltIn::ALL {
            assert_eq!(BuiltIn::from_id(builtin.id()), Some(builtin));
            assert!(builtin.id().starts_with('-'));
        }
        assert_eq!(BuiltIn::from_id("-8"), None, "dropped");
        assert_eq!(BuiltIn::from_id("910"), None);
    }
}
