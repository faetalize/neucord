//! Confirming an age group from the age gate, as the client's expressive
//! modal does (12247): the methods Discord offers, the one picked started,
//! and the vendor's page in a web view over discord.com, as the client
//! shows it in an iframe (662558). What the requests and the page are is
//! [`discord::age_verification`].

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use adw::prelude::*;
use discord::age_verification::{self, Method, Methods};
use gtk::glib;
use webkit::prelude::*;

use super::App;

/// Where the help article on confirming an age is (`AGE_VERIFICATION_LEARN_MORE`, 652215).
const LEARN_MORE: &str = "https://support.discord.com/hc/en-us/articles/30326565624343";
/// The vendors, methods and what happens to the data (`zS`, 40449).
const TRUSTED_PROVIDERS: &str = "https://discord.com/safety/age-assurance-on-discord-vendors-methods-and-your-data";
/// The experiment that shows the second link in the subtitle
/// (`2026-08-show-expressive-modal-subtitle-alt`, 12247): variation 1.
const SUBTITLE_ALT: &str = "2026-08-show-expressive-modal-subtitle-alt";

impl App {
    /// Whether the client would show this account its expressive modal (and
    /// not Incode's session), which is the flow built here.
    pub(super) fn age_verification_offered(&self) -> bool {
        let state = self.account.state.borrow();
        let Some(me) = state.me() else { return false };
        age_verification::offered(state.experiments.apex_user(me, age_verification::INCODE).map(|a| a.variant))
    }

    /// The methods dialog, over the window.
    pub(super) fn show_age_verification(self: &Rc<Self>) {
        let toasts = adw::ToastOverlay::new();
        let body = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).build();
        let scroller = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&body).build();
        let clamp = adw::Clamp::builder()
            .maximum_size(480)
            .margin_start(12)
            .margin_end(12)
            .margin_bottom(12)
            .child(&scroller)
            .build();
        let toolbar = adw::ToolbarView::builder().content(&clamp).build();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toasts.set_child(Some(&toolbar));
        let dialog = adw::Dialog::builder()
            .title("Age verification")
            .content_width(520)
            .content_height(640)
            .child(&toasts)
            .build();
        let view = Rc::new(MethodsView {
            body,
            toasts,
            dialog: dialog.clone(),
            busy: Cell::new(false),
            frame: RefCell::default(),
        });
        self.load_age_methods(&view);
        dialog.present(Some(&self.window));
    }

    /// Ask for the methods and show them, or what went wrong.
    fn load_age_methods(self: &Rc<Self>, view: &Rc<MethodsView>) {
        clear(&view.body);
        view.body.append(&gtk::Spinner::builder().spinning(true).height_request(48).margin_top(48).build());
        let rest = self.account.state.borrow().rest.clone();
        let (this, view) = (self.clone(), view.clone());
        glib::spawn_future_local(async move {
            let Some(rest) = rest else { return };
            let answer = rest.age_verification_methods().await;
            if view.dialog.parent().is_none() {
                return;
            }
            this.show_age_methods(&view, answer.ok());
        });
    }

    fn show_age_methods(self: &Rc<Self>, view: &Rc<MethodsView>, methods: Option<Methods>) {
        clear(&view.body);
        let alt = {
            let state = self.account.state.borrow();
            state.me().and_then(|me| state.experiments.apex_user(me, SUBTITLE_ALT)).is_some_and(|a| a.variant == 1)
        };
        let title = gtk::Label::builder().label("Confirm your age group").css_classes(["title-2"]).wrap(true).build();
        let subtitle = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .css_classes(["dimmed"])
            .label(subtitle(alt))
            .build();
        subtitle.connect_activate_link(|label, uri| {
            gtk::UriLauncher::new(uri).launch(
                label.root().and_downcast_ref::<gtk::Window>(),
                gtk::gio::Cancellable::NONE,
                |_| {},
            );
            glib::Propagation::Stop
        });
        view.body.append(&title);
        view.body.append(&subtitle);
        let retry = |text: &str| {
            let status = adw::StatusPage::builder().title(text).build();
            let reload =
                gtk::Button::builder().label("Reload").halign(gtk::Align::Center).css_classes(["pill"]).build();
            let (this, view) = (self.clone(), view.clone());
            reload.connect_clicked(move |_| this.load_age_methods(&view));
            status.set_child(Some(&reload));
            status
        };
        let Some(methods) = methods else {
            return view.body.append(&retry("Something went wrong loading verification methods. Please try again."));
        };
        if methods.methods.is_empty() {
            return view.body.append(&retry("No age verification methods available. Please check again shortly."));
        }
        if let Some(outage) = &methods.outage_banner_message {
            view.body.append(&adw::Banner::builder().title(outage).revealed(true).build());
        }
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        for method in &methods.methods {
            list.append(&self.age_method_row(view, method));
        }
        view.body.append(&list);
        if let Some(footer) = &methods.footer_message {
            view.body.append(
                &gtk::Label::builder().label(footer).wrap(true).css_classes(["caption", "dimmed"]).xalign(0.0).build(),
            );
        }
    }

    fn age_method_row(self: &Rc<Self>, view: &Rc<MethodsView>, method: &Method) -> adw::ActionRow {
        let subtitle = match &method.provided_by {
            Some(by) => format!("{}\n{by}", glib::markup_escape_text(&method.description)),
            None => glib::markup_escape_text(&method.description).to_string(),
        };
        let row = adw::ActionRow::builder()
            .title(glib::markup_escape_text(&method.title))
            .subtitle(subtitle)
            .subtitle_lines(0)
            .activatable(true)
            .build();
        let spinner = gtk::Spinner::new();
        row.add_suffix(&spinner);
        row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
        let (this, view, method) = (self.clone(), view.clone(), method.clone());
        row.connect_activated(move |_| {
            if !view.busy.replace(true) {
                spinner.start();
                this.start_age_verification(&view, &method, &spinner);
            }
        });
        row
    }

    /// `verify/v2` for the method picked, then the vendor's page.
    fn start_age_verification(self: &Rc<Self>, view: &Rc<MethodsView>, method: &Method, spinner: &gtk::Spinner) {
        let rest = self.account.state.borrow().rest.clone();
        let (this, view, method, spinner) = (self.clone(), view.clone(), method.clone(), spinner.clone());
        glib::spawn_future_local(async move {
            let Some(rest) = rest else { return };
            let started = rest.start_age_verification(&method).await;
            spinner.stop();
            view.busy.set(false);
            match started {
                Ok(started) => match started.page() {
                    // Incode's session is the vendor's own code, which the client runs and Neucord can't.
                    Some(url) if !started.incode => this.show_age_page(&view, url),
                    _ => view.toast("Oops, something went wrong"),
                },
                Err(discord::Error::Http { body, .. })
                    if serde_json::from_str::<serde_json::Value>(&body)
                        .is_ok_and(|b| b["code"].as_u64() == Some(age_verification::METHOD_UNAVAILABLE)) =>
                {
                    view.toast("This method is no longer available.");
                    this.load_age_methods(&view);
                }
                Err(_) => view.toast("Oops, something went wrong"),
            }
        });
    }

    /// The vendor's page, in its own dialog over the methods: Result closes
    /// the methods, Exit Review closes this one (662558, `isExpressiveModalV2`).
    fn show_age_page(self: &Rc<Self>, methods: &Rc<MethodsView>, url: &str) {
        let (sender, messages) = async_channel::unbounded::<String>();
        let content = webkit::UserContentManager::new();
        content.register_script_message_handler("age", None);
        content.connect_script_message_received(Some("age"), move |_, value| {
            let _ = sender.try_send(value.to_str().to_string());
        });
        let page = frame_view(&content, &self.window);
        page.load_html(&age_verification::frame_page(url), Some("https://discord.com/"));

        let back = gtk::Button::builder().label("Choose another method").hexpand(true).build();
        let reload = gtk::Button::builder().label("Reload").hexpand(true).build();
        let bar =
            gtk::Box::builder().spacing(12).margin_top(12).margin_bottom(12).margin_start(12).margin_end(12).build();
        bar.append(&back);
        bar.append(&reload);
        let toolbar = adw::ToolbarView::builder().content(&page).build();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        toolbar.add_bottom_bar(&bar);
        let dialog = adw::Dialog::builder()
            .title("Age verification")
            .content_width(520)
            .content_height(720)
            .child(&toolbar)
            .build();
        back.connect_clicked({
            let dialog = dialog.clone();
            move |_| {
                dialog.close();
            }
        });
        reload.connect_clicked({
            let (page, url) = (page.clone(), url.to_owned());
            move |_| page.load_html(&age_verification::frame_page(&url), Some("https://discord.com/"))
        });
        glib::spawn_future_local({
            let (dialog, methods) = (dialog.clone(), methods.clone());
            async move {
                while let Ok(kind) = messages.recv().await {
                    match kind.as_str() {
                        age_verification::RESULT => {
                            methods.dialog.close();
                        }
                        age_verification::EXIT_REVIEW => {
                            dialog.close();
                        }
                        _ => {}
                    }
                }
            }
        });
        *methods.frame.borrow_mut() = Some(dialog.clone());
        dialog.present(Some(&self.window));
    }
}

/// The methods dialog's parts.
pub(super) struct MethodsView {
    body: gtk::Box,
    toasts: adw::ToastOverlay,
    dialog: adw::Dialog,
    /// A method is being started.
    busy: Cell<bool>,
    /// The vendor's page, while it is open.
    frame: RefCell<Option<adw::Dialog>>,
}

impl MethodsView {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }
}

fn clear(body: &gtk::Box) {
    while let Some(child) = body.first_child() {
        body.remove(&child);
    }
}

/// What the dialog says under its title (`mK` in 36149): one link, or with the
/// subtitle experiment a second.
fn subtitle(alt: bool) -> String {
    let help = format!("<a href=\"{LEARN_MORE}\">how to confirm</a>");
    if alt {
        format!(
            "If confirmed as an adult (18+), you will have access to age-restricted content and settings. Need help? See {help} or learn about our <a href=\"{TRUSTED_PROVIDERS}\">trusted providers</a>."
        )
    } else {
        format!(
            "If confirmed as an adult (18+), you will have access to age-restricted content and settings. Need help? See {help}."
        )
    }
}

/// A web view for the vendor's page: an ephemeral session, camera and
/// microphone asked for before they are given, links out to the browser.
fn frame_view(content: &webkit::UserContentManager, window: &adw::ApplicationWindow) -> webkit::WebView {
    let settings = webkit::Settings::new();
    settings.set_enable_media_stream(true);
    let view = webkit::WebView::builder()
        .network_session(&webkit::NetworkSession::new_ephemeral())
        .user_content_manager(content)
        .settings(&settings)
        .vexpand(true)
        .build();
    let window = window.downgrade();
    view.connect_permission_request(move |_, request| {
        let (Some(request), Some(window)) =
            (request.downcast_ref::<webkit::UserMediaPermissionRequest>(), window.upgrade())
        else {
            return false;
        };
        let alert = adw::AlertDialog::new(
            Some("Allow Camera and Microphone?"),
            Some("The page you are verifying on asks to use them."),
        );
        alert.add_responses(&[("deny", "Don't Allow"), ("allow", "Allow")]);
        alert.set_default_response(Some("deny"));
        alert.set_close_response("deny");
        let request = request.clone();
        alert.choose(Some(&window), gtk::gio::Cancellable::NONE, move |response| {
            if response == "allow" {
                request.allow();
            } else {
                request.deny();
            }
        });
        true
    });
    view
}
