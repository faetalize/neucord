//! Which buttons the message box's bar shows (`em` in 4025,
//! `ChannelTextAreaButtons`), and the character count beside them. The
//! device's choice of apps, emoji, GIF and sticker buttons only counts for
//! accounts in `2026-06-chat-bar-customization` (951260); everyone else gets
//! every button.

use crate::Snowflake;
use crate::experiments::Experiments;

/// The bar follows the device's button choices: variant 1 of the Apex user
/// experiment (its `enabled`), which also shows their switches (`tP` in
/// 960111).
pub fn customizable(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-06-chat-bar-customization").is_some_and(|a| a.variant == 1)
}

/// A server announcement channel gets the announcement composer (`e1` in
/// 699344, variant 1), whose box takes no commands
/// (`CREATE_ANNOUNCEMENT_POST` in 355622).
pub fn announcement_composer(experiments: &Experiments, me: Snowflake) -> bool {
    experiments.apex_user(me, "2026-02-announcement-composer").is_some_and(|a| a.variant == 1)
}

/// The attach menu offers App Launcher in place of Use Apps (`eR` in
/// 316576): where the Apps button was turned off and that choice counts.
pub fn attach_launcher(customizable: bool, prefs: Prefs) -> bool {
    customizable && !prefs.apps
}

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
    /// [`customizable`] for the account.
    pub customizable: bool,
    /// A server announcement channel under [`announcement_composer`].
    pub announcement: bool,
}

/// The buttons the user chose on this device (the client's
/// `AccessibilityStore`, 775602, which Discord never sees).
#[derive(Clone, Copy, Debug)]
pub struct Prefs {
    /// The send button (`submitButtonEnabled`, off by default). Not behind
    /// `2026-06-chat-bar-customization`, unlike the other buttons' choices.
    pub send: bool,
    /// `appsButtonEnabled`, on by default, as are the three below.
    pub apps: bool,
    pub emoji: bool,
    /// `gifButtonEnabled` (#297).
    pub gif: bool,
    pub sticker: bool,
}

/// What shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub gif: bool,
    pub sticker: bool,
    pub emoji: bool,
    /// The Apps button, after emoji, which opens the app launcher.
    pub apps: bool,
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
    let chosen = |pref: bool| !bar.customizable || pref;
    Shown {
        gif: message && !bar.command && chosen(prefs.gif),
        sticker: message && !bar.command && chosen(prefs.sticker),
        emoji: !bar.recording && chosen(prefs.emoji),
        // `Z`: a box that takes commands (not the edit or announcement
        // box), with none active. Replying doesn't matter here, only to
        // the attach menu's Use Apps.
        apps: message && !bar.command && !bar.announcement && chosen(prefs.apps),
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

    use serde_json::json;

    const OPEN: Bar = Bar {
        open: true,
        editing: false,
        recording: false,
        command: false,
        empty: false,
        customizable: false,
        announcement: false,
    };
    const SEND: Prefs = Prefs { send: true, apps: true, emoji: true, gif: true, sticker: true };
    const NONE: Prefs = Prefs { send: false, apps: false, emoji: false, gif: false, sticker: false };

    #[test]
    fn a_recording_takes_the_gif_button_with_the_box() {
        let shown = shown(Bar { recording: true, ..OPEN }, SEND);
        assert!(!shown.gif);
        assert!(!shown.sticker && !shown.emoji && !shown.apps && !shown.send && !shown.count, "the whole bar goes");
    }

    #[test]
    fn an_open_box_shows_every_button() {
        let shown = shown(OPEN, SEND);
        assert!(
            shown.gif && shown.sticker && shown.emoji && shown.apps && shown.send && shown.send_enabled && shown.count
        );
    }

    #[test]
    fn an_edit_keeps_only_the_emoji_button() {
        let shown = shown(Bar { editing: true, ..OPEN }, SEND);
        assert_eq!((shown.gif, shown.sticker, shown.emoji, shown.apps, shown.send), (false, false, true, false, false));
    }

    #[test]
    fn a_box_that_doesnt_show_has_no_gif_sticker_or_send_button() {
        let shown = shown(Bar { open: false, ..OPEN }, SEND);
        assert!(!shown.gif && !shown.sticker && !shown.apps && !shown.send);
    }

    #[test]
    fn an_active_command_takes_the_gif_and_sticker_buttons() {
        let shown = shown(Bar { command: true, ..OPEN }, SEND);
        assert_eq!((shown.gif, shown.sticker, shown.emoji, shown.apps), (false, false, true, false));
        assert!(shown.send, "the send button stays");
    }

    #[test]
    fn the_send_button_shows_only_when_chosen() {
        assert!(!shown(OPEN, Prefs { send: false, ..SEND }).send);
    }

    #[test]
    fn the_send_button_greys_with_nothing_to_send_but_a_command() {
        assert!(!shown(Bar { empty: true, ..OPEN }, SEND).send_enabled);
        assert!(shown(Bar { empty: true, command: true, ..OPEN }, SEND).send_enabled);
    }

    #[test]
    fn the_button_choices_count_only_under_the_experiment() {
        let everything = shown(OPEN, NONE);
        assert!(everything.apps && everything.emoji && everything.gif && everything.sticker, "outside it: all");
        let none = shown(Bar { customizable: true, ..OPEN }, NONE);
        assert!(!none.apps && !none.emoji && !none.gif && !none.sticker);
        assert!(none.count && none.send_enabled);
        let all = shown(Bar { customizable: true, ..OPEN }, SEND);
        assert!(all.apps && all.emoji && all.gif && all.sticker);
    }

    #[test]
    fn the_announcement_composer_has_no_apps_button() {
        let shown = shown(Bar { announcement: true, ..OPEN }, SEND);
        assert!(!shown.apps);
        assert!(shown.gif && shown.sticker && shown.emoji, "its box keeps the rest");
    }

    #[test]
    fn the_attach_menu_offers_the_launcher_where_the_apps_button_is_off() {
        assert!(attach_launcher(true, NONE));
        assert!(!attach_launcher(false, NONE), "the choice doesn't count");
        assert!(!attach_launcher(true, SEND));
    }

    #[test]
    fn the_experiments_are_variant_one() {
        let assigned = |name: &str, variant: i64| {
            Experiments::from_ready(&json!({"apex_experiments": {"assignments": {"1": {"7": {"assignments": [
                [crate::experiments::murmur3(name), variant, 0, 1]
            ]}}}}}))
        };
        let me = Snowflake(7);
        for (name, gate) in [
            ("2026-06-chat-bar-customization", customizable as fn(&Experiments, Snowflake) -> bool),
            ("2026-02-announcement-composer", announcement_composer),
        ] {
            assert!(!gate(&Experiments::default(), me), "{name}: not in it");
            assert!(!gate(&assigned(name, 0), me), "{name}: control");
            assert!(gate(&assigned(name, 1), me), "{name}");
            assert!(!gate(&assigned(name, 2), me), "{name}: a variant it doesn't define");
            assert!(!gate(&assigned(name, 1), Snowflake(8)), "{name}: someone else's");
        }
    }
}
