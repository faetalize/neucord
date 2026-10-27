//! Captchas, solved in a web view that hCaptcha takes for discord.com.
//!
//! Discord's site key is meant for its own domains. hCaptcha reads the domain
//! from the page's location, which is the base URI the page is loaded with,
//! so the answer counts as solved on discord.com. The web view's session is
//! ephemeral: nothing hCaptcha stores outlives the dialog.

use std::rc::Rc;

use adw::prelude::*;
use discord::{Captcha, CaptchaSolver};
use webkit::prelude::*;

const PAGE: &str = r#"<!doctype html>
<script>
const captcha = PARAMS;
function ready() {
  const id = hcaptcha.render("captcha", {
    sitekey: captcha.sitekey,
    theme: captcha.theme,
    size: captcha.invisible ? "invisible" : "normal",
    callback: key => webkit.messageHandlers.captcha.postMessage(key),
  });
  if (captcha.rqdata) hcaptcha.setData(id, { rqdata: captcha.rqdata });
  if (captcha.invisible) hcaptcha.execute(id);
}
</script>
<script src="https://js.hcaptcha.com/1/api.js?onload=ready&render=explicit&recaptchacompat=off" async defer></script>
<div id="captcha"></div>
"#;

/// Answers captchas in a dialog over `window`.
pub(super) fn solver(window: &adw::ApplicationWindow) -> CaptchaSolver {
    let window = window.downgrade();
    Rc::new(move |captcha| {
        let window = window.upgrade();
        Box::pin(async move { solve(&window?, &captcha).await })
    })
}

async fn solve(window: &adw::ApplicationWindow, captcha: &Captcha) -> Option<String> {
    if captcha.service != "hcaptcha" {
        eprintln!("captcha: {} is not supported", captcha.service);
        return None;
    }
    let (sender, answers) = async_channel::bounded(1);
    let content = webkit::UserContentManager::new();
    content.register_script_message_handler("captcha", None);
    content.connect_script_message_received(Some("captcha"), {
        let sender = sender.clone();
        move |_, key| {
            let _ = sender.try_send(key.to_str().to_string());
        }
    });
    let view = webkit::WebView::builder()
        .network_session(&webkit::NetworkSession::new_ephemeral())
        .user_content_manager(&content)
        .build();
    // The dialog's background, not the page's white.
    view.set_background_color(&gtk::gdk::RGBA::TRANSPARENT);
    // hCaptcha's Privacy and Terms links.
    view.connect_decide_policy(|_, decision, kind| {
        if kind != webkit::PolicyDecisionType::NewWindowAction {
            return false;
        }
        if let Some(uri) = decision
            .downcast_ref::<webkit::NavigationPolicyDecision>()
            .and_then(|d| d.navigation_action())
            .and_then(|action| action.request())
            .and_then(|request| request.uri())
        {
            gtk::UriLauncher::new(&uri).launch(None::<&gtk::Window>, gio::Cancellable::NONE, |_| {});
        }
        decision.ignore();
        true
    });
    view.load_html(&page(captcha), Some("https://discord.com/"));

    let toolbar = adw::ToolbarView::builder().content(&view).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    let dialog = adw::Dialog::builder().title("Captcha").content_width(420).content_height(640).child(&toolbar).build();
    dialog.connect_closed(move |_| {
        sender.close();
    });
    dialog.present(Some(window));
    let key = answers.recv().await.ok();
    dialog.force_close();
    key
}

fn page(captcha: &Captcha) -> String {
    let params = serde_json::json!({
        "sitekey": captcha.sitekey,
        "rqdata": captcha.rqdata,
        "invisible": captcha.invisible,
        "theme": if adw::StyleManager::default().is_dark() { "dark" } else { "light" },
    });
    // JSON is a JavaScript expression, once nothing in it can end the script.
    PAGE.replace("PARAMS", &params.to_string().replace("</", "<\\/"))
}
