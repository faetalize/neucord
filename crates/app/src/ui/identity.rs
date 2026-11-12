//! What Neucord keeps of its identity for the account that is shown (see
//! [`discord::identity`]): the activity heartbeat that keeps the session
//! `client_heartbeat_session_id` names, and the file that carries it and the
//! account's experiment assignments from one launch to the next.
//!
//! The client's heartbeat (`SessionHeartbeatScheduler`, 903063) runs while
//! the app is in use, a token and the window in focus: at once when it
//! starts, then every fifteen minutes from the last run, and not at all while
//! unfocused. Each run opens a session if there is none or it lapsed, and
//! counts it as used. Neucord reports nothing with it.

use std::cell::{Cell, RefCell};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use discord::Snowflake;
use discord::experiments::Experiments;
use discord::identity::{self, Cache};
use gtk::glib;

thread_local! {
    static TIMER: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    /// When the heartbeat last ran.
    static LAST_BEAT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// One file for each account, so no account's session is another's. Private
/// to the user, like what usage keeps.
fn file_of(id: Snowflake) -> PathBuf {
    glib::user_data_dir().join("neucord").join(format!("identity-{}.json", id.0))
}

/// Forget an account's identity for good, as logging it out does.
pub(super) fn forget(id: Snowflake) {
    let _ = std::fs::remove_file(file_of(id));
}

fn load(id: Snowflake) -> Cache {
    std::fs::read_to_string(file_of(id)).map(|json| Cache::from_json(&json)).unwrap_or_default()
}

fn save() {
    let Some(id) = identity::account() else { return };
    let file = file_of(id);
    let cache = identity::cache();
    if cache.is_empty() {
        let _ = std::fs::remove_file(file);
        return;
    }
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(file.parent().unwrap_or(&file))?;
        let mut out = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&file)?;
        out.write_all(cache.to_json().as_bytes())
    };
    if let Err(e) = write() {
        eprintln!("identity: {e}");
    }
}

/// A token is about to be used for `account` (known unless it is a first
/// login): its cached session and assignments are what IDENTIFY goes by.
pub(super) fn begin(account: Option<Snowflake>) {
    let changed = identity::account() != account || account.is_none();
    identity::sign_in(account, account.map(load).unwrap_or_default());
    if changed {
        LAST_BEAT.set(None);
    }
    rearm();
}

/// The account is left or logged out.
pub(super) fn end() {
    identity::sign_out();
    LAST_BEAT.set(None);
    rearm();
}

/// READY named the account and its assignments. The client opens its session
/// on `CONNECTION_OPEN` too (`q`, 903063).
pub(super) fn ready(me: Snowflake, experiments: &Experiments) {
    identity::signed_in_as(me);
    identity::learn_assignments(experiments.user_cache(me));
    identity::heartbeat();
    save();
    rearm();
}

/// The window gained or lost focus.
pub(super) fn focus_changed(focused: bool) {
    identity::set_focused(focused);
    rearm();
}

fn beat() {
    identity::heartbeat();
    LAST_BEAT.set(Some(Instant::now()));
    save();
    rearm();
}

/// Run the heartbeat when it is due, if the app is in use; stop it if not
/// (`X` in 903063).
fn rearm() {
    if let Some(timer) = TIMER.take() {
        timer.remove();
    }
    if !identity::is_active() {
        return;
    }
    let since = LAST_BEAT.get().map(|at| at.elapsed().as_millis() as i64);
    let wait = Duration::from_millis(identity::beat_delay_ms(since) as u64);
    let timer = glib::timeout_add_local_once(wait, || {
        TIMER.set(None);
        beat();
    });
    TIMER.set(Some(timer));
}
