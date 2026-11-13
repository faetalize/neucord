//! Which buttons the message box's bar shows (`em` in 4025,
//! `ChannelTextAreaButtons`), and the character count beside them.

/// What the open channel's message box is doing.
#[derive(Clone, Copy, Debug, Default)]
pub struct Bar {
    /// The box shows: the channel takes our messages. Where it doesn't, the
    /// client's buttons draw nothing (`disabled`, `nS` in 989161) and
    /// Neucord's notice bar stands in for the box.
    pub open: bool,
    /// The box edits a message. The client's edit box (`EDIT` in 355622)
    /// has the emoji button and none of the others.
    pub editing: bool,
    /// A voice recording waits in the channel, which the client draws in
    /// place of the box and the whole bar (`tG` in 989161).
    pub recording: bool,
    /// A slash command is active (`getActiveCommand`): the client draws no
    /// GIF or sticker button beside it (`null == K`).
    pub command: bool,
}

/// What shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub gif: bool,
    pub sticker: bool,
    pub emoji: bool,
    /// The character count, which Neucord draws in the row a recording takes.
    pub count: bool,
}

pub fn shown(bar: Bar) -> Shown {
    let message = bar.open && !bar.editing && !bar.recording && !bar.command;
    Shown { gif: message, sticker: message, emoji: !bar.recording, count: !bar.recording }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: Bar = Bar { open: true, editing: false, recording: false, command: false };

    #[test]
    fn a_recording_takes_the_gif_button_with_the_box() {
        let shown = shown(Bar { recording: true, ..OPEN });
        assert!(!shown.gif);
        assert_eq!(shown, Shown { gif: false, sticker: false, emoji: false, count: false });
    }

    #[test]
    fn an_open_box_shows_every_button() {
        assert_eq!(shown(OPEN), Shown { gif: true, sticker: true, emoji: true, count: true });
    }

    #[test]
    fn an_edit_keeps_only_the_emoji_button() {
        assert_eq!(
            shown(Bar { editing: true, ..OPEN }),
            Shown { gif: false, sticker: false, emoji: true, count: true }
        );
    }

    #[test]
    fn a_box_that_doesnt_show_has_no_gif_or_sticker_button() {
        let shown = shown(Bar { open: false, ..OPEN });
        assert!(!shown.gif && !shown.sticker);
    }

    #[test]
    fn an_active_command_takes_the_gif_and_sticker_buttons() {
        assert_eq!(
            shown(Bar { command: true, ..OPEN }),
            Shown { gif: false, sticker: false, emoji: true, count: true }
        );
    }
}
