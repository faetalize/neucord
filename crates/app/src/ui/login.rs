//! The login page: email and password with the two-factor step that may
//! follow, a QR code for the phone app, and a token field as a last resort.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use discord::auth::{Login, Mfa, MfaMethod};
use discord::remote_auth::{Event, RemoteAuth};
use discord::{Error, Rest, User};
use gtk::glib;
use qrcodegen::{QrCode, QrCodeEcc};

use super::{App, bare_page, widgets};

pub(super) struct Page {
    /// "password" or "mfa".
    form: gtk::Stack,
    email: adw::EntryRow,
    password: adw::PasswordEntryRow,
    mfa_hint: gtk::Label,
    mfa_code: adw::EntryRow,
    /// Switches to the account's other two-factor methods, and back out.
    mfa_others: gtk::Box,
    token: adw::PasswordEntryRow,
    error: gtk::Label,
    /// "waiting", "code", "scanned" or "failed".
    qr: gtk::Stack,
    qr_area: gtk::DrawingArea,
    qr_code: Rc<RefCell<Option<QrCode>>>,
    scanned: gtk::Box,
    qr_error: gtk::Label,
    remote: RefCell<Option<RemoteAuth>>,
    /// Counts QR sessions, so events from a replaced one are dropped.
    session: Cell<u64>,
    /// QR sessions that failed in a row, for backing off.
    failures: Cell<u32>,
    mfa: RefCell<Option<(Mfa, MfaMethod)>>,
}

impl Page {
    pub(super) fn new() -> Self {
        Page {
            form: gtk::Stack::builder()
                .transition_type(gtk::StackTransitionType::Crossfade)
                .vhomogeneous(false)
                .build(),
            email: adw::EntryRow::builder().title("Email or Phone Number").build(),
            password: adw::PasswordEntryRow::builder().title("Password").build(),
            mfa_hint: gtk::Label::builder().wrap(true).max_width_chars(40).xalign(0.0).build(),
            mfa_code: adw::EntryRow::builder().title("Code").build(),
            mfa_others: gtk::Box::new(gtk::Orientation::Vertical, 0),
            token: adw::PasswordEntryRow::builder().title("Token").build(),
            error: gtk::Label::builder()
                .wrap(true)
                .max_width_chars(40)
                .xalign(0.0)
                .visible(false)
                .css_classes(["error"])
                .build(),
            qr: gtk::Stack::builder().transition_type(gtk::StackTransitionType::Crossfade).build(),
            qr_area: gtk::DrawingArea::builder().content_width(192).content_height(192).build(),
            qr_code: Rc::default(),
            scanned: gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(12)
                .valign(gtk::Align::Center)
                .halign(gtk::Align::Center)
                .build(),
            qr_error: gtk::Label::builder()
                .wrap(true)
                .max_width_chars(24)
                .justify(gtk::Justification::Center)
                .css_classes(["error"])
                .build(),
            remote: RefCell::default(),
            session: Cell::new(0),
            failures: Cell::new(0),
            mfa: RefCell::default(),
        }
    }

    /// End the QR session, if any.
    pub(super) fn stop_qr(&self) {
        self.session.set(self.session.get() + 1);
        self.remote.take();
    }

    fn set_error(&self, error: Option<&str>) {
        self.error.set_label(error.unwrap_or_default());
        self.error.set_visible(error.is_some());
    }

    fn clear(&self) {
        for entry in [self.password.upcast_ref::<gtk::Editable>(), self.mfa_code.upcast_ref(), self.token.upcast_ref()]
        {
            entry.set_text("");
        }
        self.mfa.take();
        self.form.set_visible_child_name("password");
        self.form.set_sensitive(true);
    }
}

impl App {
    pub(super) fn build_login(self: &Rc<Self>) -> gtk::Widget {
        let page = &self.login;
        let columns = gtk::Box::builder().spacing(36).valign(gtk::Align::Center).halign(gtk::Align::Center).build();

        let left = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(18).width_request(300).build();
        left.append(&gtk::Label::builder().label("Neucord").xalign(0.0).css_classes(["title-1"]).build());

        let password_form = gtk::Box::new(gtk::Orientation::Vertical, 18);
        password_form.append(&boxed_list(&[page.email.upcast_ref(), page.password.upcast_ref()]));
        let log_in = gtk::Button::builder()
            .label("_Log In")
            .use_underline(true)
            .css_classes(["suggested-action", "pill"])
            .build();
        password_form.append(&log_in);
        page.form.add_named(&password_form, Some("password"));

        let mfa_form = gtk::Box::new(gtk::Orientation::Vertical, 12);
        mfa_form.append(&gtk::Label::builder().label("Two-Factor Login").xalign(0.0).css_classes(["title-4"]).build());
        mfa_form.append(&page.mfa_hint);
        mfa_form.append(&boxed_list(&[page.mfa_code.upcast_ref()]));
        let verify = gtk::Button::builder()
            .label("_Log In")
            .use_underline(true)
            .margin_top(6)
            .css_classes(["suggested-action", "pill"])
            .build();
        mfa_form.append(&verify);
        mfa_form.append(&page.mfa_others);
        page.form.add_named(&mfa_form, Some("mfa"));

        left.append(&page.form);
        left.append(&page.error);
        let token_box = boxed_list(&[page.token.upcast_ref()]);
        token_box.set_margin_top(12);
        left.append(&gtk::Expander::builder().label("Log in with a token").child(&token_box).build());
        columns.append(&left);

        let separator = gtk::Separator::new(gtk::Orientation::Vertical);
        columns.append(&separator);

        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .width_request(240)
            .build();
        page.qr.add_named(&adw::Spinner::builder().width_request(32).height_request(32).build(), Some("waiting"));
        let code =
            gtk::Box::builder().halign(gtk::Align::Center).valign(gtk::Align::Center).css_classes(["qr"]).build();
        code.append(&page.qr_area);
        page.qr.add_named(&code, Some("code"));
        page.qr.add_named(&page.scanned, Some("scanned"));
        let failed = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .build();
        failed.append(&page.qr_error);
        let retry = link_button("Try now");
        retry.set_halign(gtk::Align::Center);
        failed.append(&retry);
        page.qr.add_named(&failed, Some("failed"));
        right.append(&page.qr);
        right.append(&gtk::Label::builder().label("Log In With a QR Code").css_classes(["title-4"]).build());
        right.append(
            &gtk::Label::builder()
                .label("Scan it with the Discord app on your phone.")
                .wrap(true)
                .max_width_chars(28)
                .justify(gtk::Justification::Center)
                .css_classes(["dimmed"])
                .build(),
        );
        columns.append(&right);

        // One column on narrow windows, the QR code below the form.
        self.narrow.add_setter(&columns, "orientation", Some(&gtk::Orientation::Vertical.to_value()));
        self.narrow.add_setter(&separator, "visible", Some(&false.to_value()));
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().child(&columns).margin_top(24).margin_bottom(24).build())
            .build();

        let qr_code = page.qr_code.clone();
        page.qr_area.set_draw_func(move |_, cr, width, height| {
            let Some(code) = &*qr_code.borrow() else { return };
            // Whole pixels per module keep the edges sharp.
            let size = code.size();
            let scale = (width.min(height) / size).max(1);
            let (x0, y0) = ((width - scale * size) / 2, (height - scale * size) / 2);
            cr.set_source_rgb(0.0, 0.0, 0.0);
            for y in 0..size {
                for x in 0..size {
                    if code.get_module(x, y) {
                        cr.rectangle((x0 + x * scale).into(), (y0 + y * scale).into(), scale.into(), scale.into());
                    }
                }
            }
            let _ = cr.fill();
        });

        let on = |f: fn(&Rc<App>)| {
            let this = Rc::downgrade(self);
            move || {
                if let Some(this) = this.upgrade() {
                    f(&this);
                }
            }
        };
        let password_login = on(App::password_login);
        page.email.connect_entry_activated({
            let page = page.password.clone();
            move |_| {
                page.grab_focus();
            }
        });
        page.password.connect_entry_activated({
            let f = password_login.clone();
            move |_| f()
        });
        log_in.connect_clicked(move |_| password_login());
        let verify_mfa = on(App::verify_mfa);
        page.mfa_code.connect_entry_activated({
            let f = verify_mfa.clone();
            move |_| f()
        });
        verify.connect_clicked(move |_| verify_mfa());
        let token_login = on(App::token_login);
        page.token.connect_entry_activated(move |_| token_login());
        let start_qr = on(App::start_qr);
        retry.connect_clicked(move |_| start_qr());

        bare_page(&scroller)
    }

    pub(super) fn show_login(self: &Rc<Self>, error: Option<&str>) {
        self.set_content_title("Neucord");
        self.login.clear();
        self.login.set_error(error);
        self.stack.set_visible_child_name("login");
        self.login.email.grab_focus();
        if self.login.remote.borrow().is_none() {
            self.start_qr();
        }
    }

    /// Requests started on the login page answer late; by then the user may
    /// have logged in another way.
    fn on_login_page(&self) -> bool {
        self.stack.visible_child_name().as_deref() == Some("login")
    }

    fn logged_in(self: &Rc<Self>, token: String) {
        self.login.clear();
        {
            let mut state = self.state.borrow_mut();
            state.ready_seen = false;
            state.reconnect_attempts = 0;
        }
        self.connect(token, false);
    }

    fn login_failed(&self, error: &Error) {
        self.login.form.set_sensitive(true);
        self.login.set_error(Some(&error.to_string()));
    }

    /// For the login endpoints, before there is a token.
    fn anonymous_rest(&self) -> Rest {
        Rest::anonymous().solving_captchas(super::captcha::solver(&self.window))
    }

    fn token_login(self: &Rc<Self>) {
        // Tokens copied from devtools often come wrapped in quotes.
        let token = self.login.token.text().trim().trim_matches('"').to_owned();
        if !token.is_empty() {
            self.logged_in(token);
        }
    }

    fn password_login(self: &Rc<Self>) {
        let (login, password) = (self.login.email.text().trim().to_owned(), self.login.password.text());
        if login.is_empty() || password.is_empty() || !self.login.form.is_sensitive() {
            return;
        }
        self.login.form.set_sensitive(false);
        self.login.set_error(None);
        let (this, rest) = (self.clone(), self.anonymous_rest());
        glib::spawn_future_local(async move {
            let result = rest.login(&login, &password).await;
            if !this.on_login_page() {
                return;
            }
            match result {
                Ok(Login::Token(token)) => this.logged_in(token),
                Ok(Login::Mfa(mfa)) => match mfa.preferred() {
                    Some(method) => this.choose_mfa(mfa, method),
                    None => this.login_failed(&Error::Rejected(
                        "This account logs in with a security key, which Neucord cannot use yet. \
                         Log in with the QR code instead."
                            .into(),
                    )),
                },
                Err(e) => this.login_failed(&e),
            }
        });
    }

    fn choose_mfa(self: &Rc<Self>, mfa: Mfa, method: MfaMethod) {
        let page = &self.login;
        page.form.set_sensitive(true);
        page.set_error(None);
        page.mfa_code.set_text("");
        page.mfa_hint.set_label(match method {
            MfaMethod::Totp => "Enter the code from your authenticator app.",
            MfaMethod::Sms => "Sending a code to your phone…",
            MfaMethod::Backup => "Enter one of your backup codes.",
        });
        if method == MfaMethod::Sms {
            let (this, rest) = (Rc::downgrade(self), self.anonymous_rest());
            let mfa = mfa.clone();
            glib::spawn_future_local(async move {
                let sent = rest.send_mfa_sms(&mfa).await;
                let Some(this) = this.upgrade() else { return };
                // Still waiting for this code?
                if this.login.mfa.borrow().as_ref().map(|(_, m)| *m) != Some(MfaMethod::Sms) {
                    return;
                }
                match sent {
                    Ok(phone) => this.login.mfa_hint.set_label(&format!("Enter the code sent to {phone}.")),
                    Err(e) => this.login_failed(&e),
                }
            });
        }

        while let Some(child) = page.mfa_others.first_child() {
            page.mfa_others.remove(&child);
        }
        for other in mfa.methods().into_iter().filter(|&m| m != method) {
            let label = match other {
                MfaMethod::Totp => "Use your authenticator app",
                MfaMethod::Sms => "Text me a code",
                MfaMethod::Backup => "Use a backup code",
            };
            let button = link_button(label);
            let (this, mfa) = (Rc::downgrade(self), mfa.clone());
            button.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    this.choose_mfa(mfa.clone(), other);
                }
            });
            page.mfa_others.append(&button);
        }
        let back = link_button("Back");
        let this = Rc::downgrade(self);
        back.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.login.mfa.take();
                this.login.set_error(None);
                this.login.form.set_visible_child_name("password");
            }
        });
        page.mfa_others.append(&back);

        page.mfa.replace(Some((mfa, method)));
        page.form.set_visible_child_name("mfa");
        page.mfa_code.grab_focus();
    }

    fn verify_mfa(self: &Rc<Self>) {
        let Some((mfa, method)) = self.login.mfa.borrow().clone() else { return };
        // Codes are often shown grouped, as `123 456` or `abcd-efgh`.
        let code: String = self.login.mfa_code.text().chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
        if code.is_empty() || !self.login.form.is_sensitive() {
            return;
        }
        self.login.form.set_sensitive(false);
        self.login.set_error(None);
        let (this, rest) = (self.clone(), self.anonymous_rest());
        glib::spawn_future_local(async move {
            let result = rest.verify_mfa(&mfa, method, &code).await;
            if !this.on_login_page() {
                return;
            }
            match result {
                Ok(token) => this.logged_in(token),
                Err(e) => this.login_failed(&e),
            }
        });
    }

    /// Show a fresh QR code, replacing the current one.
    fn start_qr(self: &Rc<Self>) {
        self.login.stop_qr();
        let session = self.login.session.get();
        self.login.qr.set_visible_child_name("waiting");
        let this = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = RemoteAuth::connect().await;
            let current = || this.upgrade().filter(|this| this.login.session.get() == session);
            let Some(app) = current() else { return };
            let events = match result {
                Ok((remote, events)) => {
                    app.login.remote.replace(Some(remote));
                    events
                }
                Err(e) => return app.qr_failed(&e),
            };
            drop(app);
            while let Ok(event) = events.recv().await {
                let Some(app) = current() else { return };
                app.on_qr(event);
            }
        });
    }

    fn on_qr(self: &Rc<Self>, event: Event) {
        let page = &self.login;
        match event {
            Event::Code(url) => {
                page.failures.set(0);
                page.qr_code.replace(QrCode::encode_text(&url, QrCodeEcc::Medium).ok());
                page.qr_area.queue_draw();
                page.qr.set_visible_child_name("code");
            }
            Event::Scanned(user) => {
                self.show_scanned(&user);
                page.qr.set_visible_child_name("scanned");
            }
            Event::Token(token) => self.logged_in(token),
            // Discord's own client just shows a new code.
            Event::Cancelled | Event::Expired => self.start_qr(),
            Event::Failed(e) => self.qr_failed(&e),
        }
    }

    fn show_scanned(self: &Rc<Self>, user: &User) {
        let scanned = &self.login.scanned;
        while let Some(child) = scanned.first_child() {
            scanned.remove(&child);
        }
        let avatar = widgets::avatar_widget(user, 40, Some(&self.images));
        avatar.set_halign(gtk::Align::Center);
        scanned.append(&avatar);
        scanned.append(
            &gtk::Label::builder()
                .label(format!("Confirm the login as {} on your phone.", user.username))
                .wrap(true)
                .max_width_chars(24)
                .justify(gtk::Justification::Center)
                .build(),
        );
        let other = link_button("Show a new code");
        other.set_halign(gtk::Align::Center);
        let this = Rc::downgrade(self);
        other.connect_clicked(move |_| {
            if let Some(this) = this.upgrade() {
                this.start_qr();
            }
        });
        scanned.append(&other);
    }

    /// Show why, and try again by itself after a while, as Discord's client
    /// does: 1.5 s, doubling up to 30 s.
    fn qr_failed(self: &Rc<Self>, error: &Error) {
        self.login.stop_qr();
        let session = self.login.session.get();
        let failures = self.login.failures.get() + 1;
        self.login.failures.set(failures);
        let delay = (Duration::from_millis(1500) * 2u32.pow(failures.min(6) - 1)).min(Duration::from_secs(30));
        self.login.qr_error.set_label(&format!("QR login failed: {error}. Trying again in {}s.", delay.as_secs_f64()));
        self.login.qr.set_visible_child_name("failed");
        let this = Rc::downgrade(self);
        glib::timeout_add_local_once(delay, move || {
            if let Some(this) =
                this.upgrade().filter(|this| this.login.session.get() == session && this.on_login_page())
            {
                this.start_qr();
            }
        });
    }
}

fn boxed_list(rows: &[&gtk::Widget]) -> gtk::ListBox {
    let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
    for row in rows {
        list.append(*row);
    }
    list
}

fn link_button(label: &str) -> gtk::Button {
    gtk::Button::builder().label(label).halign(gtk::Align::Start).css_classes(["link"]).build()
}
