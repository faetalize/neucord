//! Mark As Not Sensitive in a message's menu on media the filter covers
//! (761304, `report-image-false-positive`; the confirmation is 544135 over
//! 961997): it asks, then forwards the media to Discord to improve its
//! scans.

use std::rc::Rc;

use adw::prelude::*;
use discord::sensitive::Report;
use discord::{Message, Snowflake};
use gtk::glib;

use super::App;
use super::chat::MessageTarget;
use super::embeds::{MAX_H, MAX_W, fitted_picture};
use super::friends::confirm;
use super::menus::{Entry, Item, Kind};

/// The client's words (`TPpVkI`, `z4du/I`, `gFsTKu`, `R0RpRX`).
const HEADING: &str = "Mark as not sensitive";
const QUESTION: &str = "Help improve our sensitive content filters by forwarding this media to Discord.";
const THANKS: &str = "Thanks for letting us know!";
const FAILED: &str = "Oops, something went wrong...";

pub(super) const MARK_NOT_SENSITIVE: Entry<MessageTarget> = Entry::Item(Item {
    name: "mark-not-sensitive",
    label: "Mark As Not _Sensitive",
    shows: |app, target| {
        target.media.as_ref().is_some_and(|media| app.sensitive_report(target.shown(), media).is_some())
    },
    kind: Kind::Action(|app, target| {
        let Some(report) = target.media.as_ref().and_then(|media| app.sensitive_report(target.shown(), media)) else {
            return;
        };
        app.confirm_not_sensitive(target.shown(), report);
    }),
});

impl App {
    /// Ask, with the picture when it is the only thing reported, then send
    /// the report.
    pub(super) fn confirm_not_sensitive(self: &Rc<Self>, message: &Message, report: Report) {
        let dialog = confirm(HEADING, QUESTION, "_Confirm", adw::ResponseAppearance::Suggested);
        if let [only] = report.attachments[..]
            && report.embeds.is_empty()
        {
            dialog.set_extra_child(self.reported_picture(message, only).as_ref());
        }
        let (this, message) = (self.clone(), message.clone());
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&this.window)).await != "confirm" {
                return;
            }
            let Some(rest) = this.account.state.borrow().rest.clone() else { return };
            match rest.report_false_positive(report.body(&message)).await {
                Ok(()) => this.toast(THANKS),
                Err(e) => {
                    eprintln!("report false positive: {e}");
                    this.toast(FAILED);
                }
            }
        });
    }

    /// The attachment being reported, as a picture.
    fn reported_picture(&self, message: &Message, id: Snowflake) -> Option<gtk::Widget> {
        let attachment = message.attachments.iter().find(|a| a.id == id).filter(|a| a.is_image())?;
        let fitted = fitted_picture(attachment.width, attachment.height, MAX_W, MAX_H);
        let url = attachment.proxy_url.as_deref().unwrap_or(&attachment.url);
        self.images.set_sized(&fitted.picture, url, fitted.size, attachment.width.zip(attachment.height));
        Some(fitted.frame.upcast())
    }
}
