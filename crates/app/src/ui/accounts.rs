//! The accounts on this device, as the official client's switcher has them:
//! the account menu's Switch Accounts submenu (998034), the Manage Accounts
//! dialog (623161, 970573) and the list the login page shows while accounts
//! are stored but none is logged in (275941).
//!
//! Like the client, one account is connected at a time: switching closes the
//! shown account's gateway, forgets what the window held of it, and connects
//! the other's token. Nothing is sent to Discord for that. Opening a list
//! asks Discord who each stored token belongs to, and logging an account out
//! sends `POST /auth/logout` with its own token.

use std::rc::Rc;

use adw::prelude::*;
use discord::accounts::{Answer, Entry as Listed, MAX_ACCOUNTS, Status};
use discord::{Rest, Snowflake, User, session};
use gtk::{gdk, gio, glib};

use super::menus::{self, At, Entry, Item, Kind};
use super::{App, bare_page, widgets};

/// Where the login page was opened from, which decides how its requests go
/// out and what Back does.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Origin {
    /// No account to go back to.
    #[default]
    Start,
    /// Adding an account while another stays logged in.
    Beside,
    /// From the list of stored accounts, which Back returns to.
    List,
}

/// Which list of accounts: the dialog, or the page shown when none is
/// logged in. They differ in what their buttons say.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Role {
    Manage,
    Choose,
}

pub(super) struct AccountTarget(pub(super) Snowflake);

/// The menu of an account's row in the Manage Accounts dialog
/// (`manage-multi-account`, 970573): Log out, and nothing else.
pub(super) const LOG_OUT: Entry<AccountTarget> = Entry::Item(Item {
    name: "log-out",
    label: "Log _Out",
    shows: |_, _| true,
    kind: Kind::Action(|app, target| app.logout_account(target.0)),
});

impl App {
    /// Open the account that was shown last, or wherever the stored ones
    /// leave the user.
    pub(super) async fn resume_session(self: &Rc<Self>) {
        match session::load_roster().await {
            Ok(roster) => *self.roster.borrow_mut() = roster,
            Err(e) => return self.show_login(Some(&e.to_string())),
        }
        // Earlier versions kept one token apart; it is the account they
        // were last in, and READY files it under its user id.
        match session::load_legacy_token().await {
            Ok(Some(token)) => {
                self.account.state.borrow_mut().migrating = true;
                return self.connect(token, true);
            }
            Ok(None) => {}
            Err(e) => return self.show_login(Some(&e.to_string())),
        }
        let current = self.roster.borrow().current();
        if let Some(id) = current {
            match session::load_token(id).await {
                Ok(Some(token)) => {
                    self.account.state.borrow_mut().target = Some(id);
                    return self.connect(token, true);
                }
                Ok(None) => {}
                Err(e) => return self.show_login(Some(&e.to_string())),
            }
        }
        self.show_signed_out(None);
    }

    /// An account connected: put it on the list, and its token in the
    /// keyring if it isn't. `migrated` is a token that came from the entry of
    /// earlier versions, which goes once it is filed under its user id.
    pub(super) fn remember_account(self: &Rc<Self>, user: &User, token: Option<String>, migrated: bool) {
        let before = self.roster.borrow().to_json();
        let dropped = self.roster.borrow_mut().opened(user);
        let changed = self.roster.borrow().to_json() != before;
        self.refresh_account_views();
        // Past the limit, the client forgets the account it has just added.
        let kept = !dropped.contains(&user.id);
        let (this, id, label) = (self.clone(), user.id, format!("Neucord ({})", user.username));
        glib::spawn_future_local(async move {
            if kept
                && let Some(token) = token
                && let Err(e) = session::store_token(id, &token, &label).await
            {
                eprintln!("could not save token: {e}");
            }
            if changed {
                this.save_roster().await;
            }
            for id in dropped {
                let _ = session::clear_token(id).await;
            }
            if migrated && kept {
                let _ = session::clear_legacy_token().await;
            }
        });
    }

    pub(super) async fn save_roster(&self) {
        let roster = self.roster.borrow().clone();
        if let Err(e) = session::store_roster(&roster).await {
            eprintln!("could not save the accounts: {e}");
        }
    }

    /// Nobody is logged in: the stored accounts to choose from, or the
    /// login page if there are none. `error` says why we got here.
    pub(super) fn show_signed_out(self: &Rc<Self>, error: Option<&str>) {
        if self.roster.borrow().is_empty() {
            return self.show_login(error);
        }
        self.set_content_title("Neucord");
        self.accounts_error.set_label(error.unwrap_or_default());
        self.accounts_error.set_visible(error.is_some());
        self.refresh_account_views();
        self.stack.set_visible_child_name("accounts");
        self.validate_accounts();
    }

    /// The page of stored accounts, for when none is logged in. The client
    /// asks "Choose an account" and offers to add one (275941).
    pub(super) fn build_choose_account(self: &Rc<Self>) -> gtk::Widget {
        let column = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(18)
            .valign(gtk::Align::Center)
            .margin_top(24)
            .margin_bottom(24)
            .build();
        column.append(&gtk::Label::builder().label("Choose an account").xalign(0.0).css_classes(["title-1"]).build());
        column.append(
            &gtk::Label::builder()
                .label("Select an account to log in with or add a new one.")
                .wrap(true)
                .xalign(0.0)
                .build(),
        );
        column.append(&self.accounts_error);
        column.append(&self.account_list(Role::Choose));
        let add = gtk::Button::builder().label("_Add an Account").use_underline(true).css_classes(["pill"]).build();
        add.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.add_account(Origin::List);
                }
            }
        });
        column.append(&add);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&adw::Clamp::builder().maximum_size(420).child(&column).build())
            .build();
        bare_page(&scroller)
    }

    /// A list of the stored accounts, kept current as they change.
    fn account_list(self: &Rc<Self>, role: Role) -> gtk::ListBox {
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).css_classes(["boxed-list"]).build();
        self.account_lists.borrow_mut().push((list.downgrade(), role));
        self.fill_account_list(&list, role);
        list
    }

    fn fill_account_list(self: &Rc<Self>, list: &gtk::ListBox, role: Role) {
        list.remove_all();
        let (entries, validating) = {
            let roster = self.roster.borrow();
            (roster.entries().to_vec(), roster.is_validating())
        };
        // The client shows a spinner instead of the list while it asks.
        if validating {
            let spinner = adw::Spinner::builder().width_request(32).height_request(32).margin_top(12).margin_bottom(12);
            list.append(&spinner.build());
            return;
        }
        let current = self.account.state.borrow().me();
        for entry in entries {
            list.append(&self.account_row(&entry, role, current));
        }
    }

    fn account_row(self: &Rc<Self>, entry: &Listed, role: Role, current: Option<Snowflake>) -> adw::ActionRow {
        let id = entry.id;
        let invalid = entry.status == Status::Invalid;
        let row = adw::ActionRow::builder().title(entry.tag()).use_markup(false).build();
        row.add_prefix(&widgets::avatar_widget(&entry.user(), 40, Some(&self.images)));
        let active = current == Some(id);
        if active {
            row.set_subtitle("Active account");
        } else if invalid {
            row.set_subtitle("Please log in again.");
        }
        let weak = Rc::downgrade(self);
        if !active {
            let label = match (invalid, role) {
                (true, _) | (_, Role::Choose) => "Log In",
                _ => "Switch",
            };
            let button = gtk::Button::builder().label(label).valign(gtk::Align::Center).build();
            button.connect_clicked({
                let weak = weak.clone();
                move |_| {
                    let Some(this) = weak.upgrade() else { return };
                    this.close_manage();
                    if invalid {
                        this.add_account(if role == Role::Manage { Origin::Beside } else { Origin::List });
                    } else {
                        this.switch_to(id);
                    }
                }
            });
            row.add_suffix(&button);
        }
        let more = gtk::Button::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("More Options")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .build();
        more.connect_clicked(move |button| {
            let Some(this) = weak.upgrade() else { return };
            let rect = gdk::Rectangle::new(0, 0, button.width(), button.height());
            menus::popup(&this, menus::ACCOUNT, AccountTarget(id), button, At::Below(rect));
        });
        row.add_suffix(&more);
        row
    }

    /// Redraw everything that lists the accounts: the lists, and the menu.
    pub(super) fn refresh_account_views(self: &Rc<Self>) {
        let lists: Vec<_> = {
            let mut lists = self.account_lists.borrow_mut();
            lists.retain(|(list, _)| list.upgrade().is_some());
            lists.iter().filter_map(|(list, role)| Some((list.upgrade()?, *role))).collect()
        };
        for (list, role) in lists {
            self.fill_account_list(&list, role);
        }

        let (entries, current) = (self.roster.borrow().entries().to_vec(), self.account.state.borrow().me());
        self.switch_items.remove_all();
        for entry in &entries {
            let tag = entry.tag().replace('_', "__");
            let label = if entry.status == Status::Invalid { format!("{tag} (log in again)") } else { tag };
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("win.switch-account"), Some(&entry.id.to_string().to_variant()));
            self.switch_items.append_item(&item);
        }
        if let Some(action) = self.window.lookup_action("switch-account").and_downcast::<gio::SimpleAction>() {
            action.set_state(&current.map(|id| id.to_string()).unwrap_or_default().to_variant());
        }
    }

    /// Ask Discord whom each stored token belongs to, as the client does
    /// whenever a list of the accounts opens (`validate` in 429707): an
    /// account without a token is invalid without asking, one Discord
    /// rejects is too, and any other failure is no reason to doubt the
    /// token. They are asked at once, and not again while still being asked.
    pub(super) fn validate_accounts(self: &Rc<Self>) {
        if self.roster.borrow().is_validating() {
            return;
        }
        let ids: Vec<Snowflake> = self.roster.borrow().entries().iter().map(|e| e.id).collect();
        for &id in &ids {
            self.roster.borrow_mut().validating(id);
        }
        self.refresh_account_views();
        for id in ids {
            let this = self.clone();
            glib::spawn_future_local(async move {
                let answer = match session::load_token(id).await {
                    Ok(Some(token)) if !token.is_empty() => Answer::of(Rest::new(token).me().await),
                    _ => Answer::NoToken,
                };
                let refreshed = matches!(answer, Answer::User(_));
                this.roster.borrow_mut().validated(id, answer);
                this.refresh_account_views();
                if refreshed {
                    this.save_roster().await;
                }
            });
        }
    }

    /// Log in as `id`, leaving the shown account: no request goes out, and
    /// the client does just that (`switchAccountToken` in 830215).
    pub(super) fn switch_to(self: &Rc<Self>, id: Snowflake) {
        if self.account.state.borrow().me() == Some(id) {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            match session::load_token(id).await {
                Ok(Some(token)) if !token.is_empty() => {
                    drop(this.leave_account());
                    this.login_origin.set(Origin::Start);
                    this.account.state.borrow_mut().target = Some(id);
                    this.connect(token, true);
                }
                // The client marks the account and stays where it is.
                Ok(_) => {
                    this.roster.borrow_mut().validated(id, Answer::NoToken);
                    this.refresh_account_views();
                }
                Err(e) => this.toast(&e.to_string()),
            }
        });
    }

    /// Log an account out from the list: its token ends at Discord, and it
    /// leaves the list and the keyring.
    pub(super) fn logout_account(self: &Rc<Self>, id: Snowflake) {
        if self.account.state.borrow().me() == Some(id) {
            self.close_manage();
            return self.logout();
        }
        let name = self.roster.borrow().get(id).map(|e| e.username.clone()).unwrap_or_default();
        let this = self.clone();
        let hold = self.window.application().map(|app| app.hold());
        glib::spawn_future_local(async move {
            let _hold = hold;
            let token = session::load_token(id).await.ok().flatten();
            // The token is forgotten before Discord is told, as when logging
            // out the shown account.
            this.forget_account(Some(id), false).await;
            if let Some(token) = token.filter(|t| !t.is_empty())
                && let Err(e) = Rest::new(token).solving_captchas(super::captcha::solver(&this.window)).logout().await
            {
                eprintln!("logout: {e}");
            }
            this.set_manage_note(&format!("{name} logged out"), false);
        });
    }

    /// The login page, to add an account to the list. At the limit the
    /// client says so instead.
    pub(super) fn add_account(self: &Rc<Self>, origin: Origin) {
        if self.roster.borrow().is_full() {
            return self.toast(&full_message());
        }
        self.show_login_as(origin, None);
    }

    pub(super) fn login_back(self: &Rc<Self>) {
        let origin = self.login_origin.replace(Origin::Start);
        self.login.stop_qr();
        self.login.clear();
        match origin {
            Origin::Beside => self.stack.set_visible_child_name("main"),
            Origin::List => self.show_signed_out(None),
            Origin::Start => {}
        }
    }

    /// The dialog behind the account menu's Manage Accounts (623161).
    pub(super) fn show_manage_accounts(self: &Rc<Self>) {
        let column = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(12).margin_start(18).build();
        column.set_margin_end(18);
        column.set_margin_bottom(18);
        column.append(&gtk::Label::builder().label("Switch accounts, sign in, sign out, go wild.").wrap(true).build());
        let note = gtk::Label::builder().wrap(true).visible(false).build();
        column.append(&note);
        column.append(&self.account_list(Role::Manage));
        let add =
            gtk::Button::builder().label("_Add an Account").use_underline(true).halign(gtk::Align::Center).build();
        column.append(&add);

        let toolbar = adw::ToolbarView::builder().content(&column).build();
        toolbar.add_top_bar(&adw::HeaderBar::new());
        let dialog = adw::Dialog::builder()
            .title("Manage Accounts")
            .content_width(440)
            .follows_content_size(true)
            .child(&toolbar)
            .build();
        add.connect_clicked({
            let this = Rc::downgrade(self);
            move |_| {
                let Some(this) = this.upgrade() else { return };
                if this.roster.borrow().is_full() {
                    this.set_manage_note(&full_message(), true);
                } else {
                    this.close_manage();
                    this.add_account(Origin::Beside);
                }
            }
        });
        dialog.connect_closed({
            let this = Rc::downgrade(self);
            move |_| {
                if let Some(this) = this.upgrade() {
                    this.manage.take();
                }
            }
        });
        self.manage.replace(Some((dialog.clone(), note)));
        dialog.present(Some(&self.window));
        self.validate_accounts();
    }

    pub(super) fn close_manage(&self) {
        let open = self.manage.borrow().as_ref().map(|(dialog, _)| dialog.clone());
        if let Some(dialog) = open {
            dialog.close();
        }
    }

    /// Say something above the dialog's list, while it is open.
    fn set_manage_note(&self, text: &str, error: bool) {
        if let Some((_, note)) = &*self.manage.borrow() {
            note.set_label(text);
            note.set_visible(true);
            note.remove_css_class("error");
            note.remove_css_class("success");
            note.add_css_class(if error { "error" } else { "success" });
        }
    }

    /// The submenu of the account menu: the accounts, ticking the shown one,
    /// then Manage Accounts (998034).
    pub(super) fn switch_menu(&self) -> gio::Menu {
        let submenu = gio::Menu::new();
        submenu.append_section(None, &self.switch_items);
        let manage = gio::Menu::new();
        manage.append(Some("_Manage Accounts"), Some("win.manage-accounts"));
        submenu.append_section(None, &manage);
        let menu = gio::Menu::new();
        menu.append_submenu(Some("_Switch Accounts"), &submenu);
        menu
    }

    /// The window's actions for the menu above.
    pub(super) fn account_actions(self: &Rc<Self>) -> [gio::SimpleAction; 2] {
        let switch = gio::SimpleAction::new_stateful("switch-account", Some(glib::VariantTy::STRING), &"".to_variant());
        switch.connect_activate({
            let this = Rc::downgrade(self);
            move |_, target| {
                let (Some(this), Some(id)) = (this.upgrade(), target.and_then(|t| t.str()?.parse().ok())) else {
                    return;
                };
                let id = Snowflake(id);
                let invalid = this.roster.borrow().get(id).is_some_and(|e| e.status == Status::Invalid);
                if invalid {
                    this.add_account(Origin::Beside);
                } else {
                    this.switch_to(id);
                }
            }
        });
        let manage = gio::SimpleAction::new("manage-accounts", None);
        manage.connect_activate({
            let this = Rc::downgrade(self);
            move |_, _| {
                if let Some(this) = this.upgrade() {
                    this.show_manage_accounts();
                }
            }
        });
        [switch, manage]
    }
}

fn full_message() -> String {
    format!("Sorry, you can only add {MAX_ACCOUNTS} accounts.")
}
