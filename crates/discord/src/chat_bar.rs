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
    /// The box holds nothing to send ([`crate::rest::nothing_to_send`]).
    pub empty: bool,
}

/// The buttons the user chose on this device (the client's
/// `AccessibilityStore`, 775602, which Discord never sees).
#[derive(Clone, Copy, Debug)]
pub struct Prefs {
    /// The send button (`submitButtonEnabled`, off by default). Not behind
    /// `2026-06-chat-bar-customization`, unlike the other buttons' choices.
    pub send: bool,
}

/// What shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub gif: bool,
    pub sticker: bool,
    pub emoji: bool,
    /// The send button, last in the row after a separator (`eu`).
    pub send: bool,
    /// The send button can be pressed: `G` in 4025 greys it with nothing to
    /// send. An active command is something to send: the client's box then
    /// holds the command's text.
    pub send_enabled: bool,
    /// The character count, which Neucord draws in the row a recording takes.
    pub count: bool,
}

pub fn shown(bar: Bar, prefs: Prefs) -> Shown {
    let message = bar.open && !bar.editing && !bar.recording;
    Shown {
        gif: message && !bar.command,
        sticker: message && !bar.command,
        emoji: !bar.recording,
        // The client's edit box has no `submit.button`; Neucord's box
        // doubles as it.
        send: message && prefs.send,
        send_enabled: !bar.empty || bar.command,
        count: !bar.recording,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPEN: Bar = Bar { open: true, editing: false, recording: false, command: false, empty: false };
    const SEND: Prefs = Prefs { send: true };

    #[test]
    fn a_recording_takes_the_gif_button_with_the_box() {
        let shown = shown(Bar { recording: true, ..OPEN }, SEND);
        assert!(!shown.gif);
        assert!(!shown.sticker && !shown.emoji && !shown.send && !shown.count, "the whole bar goes");
    }

    #[test]
    fn an_open_box_shows_every_button() {
        let shown = shown(OPEN, SEND);
        assert!(shown.gif && shown.sticker && shown.emoji && shown.send && shown.send_enabled && shown.count);
    }

    #[test]
    fn an_edit_keeps_only_the_emoji_button() {
        let shown = shown(Bar { editing: true, ..OPEN }, SEND);
        assert_eq!((shown.gif, shown.sticker, shown.emoji, shown.send), (false, false, true, false));
    }

    #[test]
    fn a_box_that_doesnt_show_has_no_gif_sticker_or_send_button() {
        let shown = shown(Bar { open: false, ..OPEN }, SEND);
        assert!(!shown.gif && !shown.sticker && !shown.send);
    }

    #[test]
    fn an_active_command_takes_the_gif_and_sticker_buttons() {
        let shown = shown(Bar { command: true, ..OPEN }, SEND);
        assert_eq!((shown.gif, shown.sticker, shown.emoji), (false, false, true));
        assert!(shown.send, "the send button stays");
    }

    #[test]
    fn the_send_button_shows_only_when_chosen() {
        assert!(!shown(OPEN, Prefs { send: false }).send);
    }

    #[test]
    fn the_send_button_greys_with_nothing_to_send_but_a_command() {
        assert!(!shown(Bar { empty: true, ..OPEN }, SEND).send_enabled);
        assert!(shown(Bar { empty: true, command: true, ..OPEN }, SEND).send_enabled);
    }
}
