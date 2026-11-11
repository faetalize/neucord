//! Starting with the session, and being allowed to run with no window.
//!
//! On the host that is an autostart entry in `$XDG_CONFIG_HOME/autostart`.
//! Under Flatpak the sandbox can't write one, so the Background portal does
//! it, and it also tells the system the app means to keep running when its
//! window is gone (GNOME lists such apps, and may end one that never asked).
//! Nothing here touches Discord.
//!
//! An autostarted Neucord is launched with `--background`: it starts
//! hidden, behind its tray icon, instead of putting a window up at login.

use std::path::{Path, PathBuf};

use gtk::gio::{self, DBusCallFlags, DBusSignalFlags};
use gtk::glib::prelude::*;
use gtk::glib::{self, Variant, VariantDict};

use crate::APP_ID;

/// The command-line flag of an autostarted launch.
pub const FLAG: &str = "background";

const PORTAL: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const BACKGROUND: &str = "org.freedesktop.portal.Background";
const REQUEST: &str = "org.freedesktop.portal.Request";

/// Whether the app runs in a Flatpak sandbox, which the runtime marks with
/// this file.
pub fn sandboxed() -> bool {
    Path::new("/.flatpak-info").exists()
}

fn autostart_file() -> PathBuf {
    glib::user_config_dir().join("autostart").join(format!("{APP_ID}.desktop"))
}

/// An argument of an `Exec` line. Quoting is needed for anything the format
/// reserves, and inside quotes `"`, `` ` ``, `$` and `\` take a backslash,
/// which the file's own string escaping then doubles. `%` starts a field
/// code, so a literal one is doubled.
fn exec_argument(argument: &str) -> String {
    const RESERVED: &str = " \t\n\"'\\><~|&;$*?#()`";
    let argument = argument.replace('%', "%%");
    if !argument.contains(|c| RESERVED.contains(c)) {
        return argument;
    }
    let mut quoted = String::from("\"");
    for c in argument.chars() {
        match c {
            '\\' => quoted.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                quoted.push_str("\\\\");
                quoted.push(c);
            }
            // The one that file-level string escaping reads as a line break.
            '\n' => quoted.push_str("\\n"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// The autostart entry that runs `executable` in the background.
pub fn desktop_entry(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Neucord\nExec={} --{FLAG}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        exec_argument(&executable.to_string_lossy())
    )
}

/// Make the host start Neucord with the session, or stop doing so. The entry
/// is rewritten on every call, so it follows the binary when it moves.
pub fn set_autostart(enabled: bool) -> std::io::Result<()> {
    let file = autostart_file();
    if !enabled {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let entry = desktop_entry(&std::env::current_exe()?);
    if std::fs::read_to_string(&file).is_ok_and(|current| current == entry) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, entry)
}

/// What the portal answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    /// The app may keep running with no window.
    pub background: bool,
    /// It will be started at login.
    pub autostart: bool,
}

/// The path the portal answers on: the caller's unique bus name without its
/// colon and with dots as underscores, then the token we chose. Knowing it
/// lets us listen before asking, so a quick answer can't be missed.
fn request_path(unique_name: &str, token: &str) -> String {
    format!("{PORTAL_PATH}/request/{}/{token}", unique_name.trim_start_matches(':').replace('.', "_"))
}

fn options(token: &str, autostart: bool, executable: &Path) -> Variant {
    let options = VariantDict::new(None);
    options.insert_value("handle_token", &token.to_variant());
    options.insert_value("reason", &"Keep Neucord running in the background for notifications".to_variant());
    options.insert_value("autostart", &autostart.to_variant());
    options.insert_value("commandline", &[executable.to_string_lossy().into_owned(), format!("--{FLAG}")].to_variant());
    options.end()
}

/// The portal's `Response(u, a{sv})`: 0 is a yes; 1 the user said no; 2
/// anything else.
fn grant(response: &Variant) -> Option<Grant> {
    let (code, results) = response.get::<(u32, VariantDict)>()?;
    if code != 0 {
        return Some(Grant { background: false, autostart: false });
    }
    let flag = |name| results.lookup::<bool>(name).ok().flatten();
    Some(Grant { background: flag("background")?, autostart: flag("autostart").unwrap_or(false) })
}

/// Ask the Background portal to let the app run with no window, and to
/// start it at login if `autostart`.
pub async fn request(autostart: bool) -> Result<Grant, String> {
    let connection = gio::bus_get_future(gio::BusType::Session).await.map_err(|e| e.to_string())?;
    let unique = connection.unique_name().ok_or("no bus name")?;
    let token = format!("neucord{}", std::process::id());
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;

    let (sender, answers) = async_channel::bounded(1);
    let _subscription = connection.subscribe_to_signal(
        Some(PORTAL),
        Some(REQUEST),
        Some("Response"),
        Some(&request_path(&unique, &token)),
        None,
        DBusSignalFlags::NONE,
        move |signal| {
            let _ = sender.try_send(grant(signal.parameters));
        },
    );
    connection
        .call_future(
            Some(PORTAL),
            PORTAL_PATH,
            BACKGROUND,
            "RequestBackground",
            Some(&Variant::tuple_from_iter(["".to_variant(), options(&token, autostart, &executable)])),
            None,
            DBusCallFlags::NONE,
            -1,
        )
        .await
        .map_err(|e| e.to_string())?;
    answers.recv().await.map_err(|e| e.to_string())?.ok_or_else(|| "unreadable answer".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_entry_starts_the_binary_in_the_background() {
        let entry = desktop_entry(Path::new("/opt/neucord/bin/neucord"));
        assert!(entry.starts_with("[Desktop Entry]\nType=Application\n"));
        assert!(entry.contains("\nExec=/opt/neucord/bin/neucord --background\n"), "{entry}");
        assert!(entry.contains("\nTerminal=false\n"));
    }

    #[test]
    fn a_path_the_format_reserves_is_quoted_and_escaped() {
        assert_eq!(exec_argument("/usr/bin/neucord"), "/usr/bin/neucord");
        assert_eq!(exec_argument("/home/a b/neucord"), "\"/home/a b/neucord\"");
        assert_eq!(exec_argument("/100%/neucord"), "/100%%/neucord");
        assert_eq!(exec_argument("/a\\b c"), "\"/a\\\\\\\\b c\"", "a backslash is escaped twice over");
        assert_eq!(exec_argument("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(exec_argument("/a\"b"), "\"/a\\\\\"b\"");
    }

    #[test]
    fn the_portal_answers_on_a_path_made_from_our_name_and_token() {
        assert_eq!(
            request_path(":1.42", "tok"),
            "/org/freedesktop/portal/desktop/request/1_42/tok",
            "the colon goes and the dot becomes an underscore"
        );
    }

    #[test]
    fn the_request_carries_what_the_portal_documents() {
        let options = VariantDict::new(Some(&options("tok", true, Path::new("/app/bin/neucord"))));
        assert_eq!(options.lookup::<String>("handle_token").unwrap().as_deref(), Some("tok"));
        assert_eq!(options.lookup::<bool>("autostart").unwrap(), Some(true));
        assert_eq!(
            options.lookup::<Vec<String>>("commandline").unwrap(),
            Some(vec!["/app/bin/neucord".to_owned(), "--background".to_owned()])
        );
        assert!(options.lookup::<String>("reason").unwrap().is_some());
    }

    #[test]
    fn the_answer_is_a_yes_only_for_response_zero() {
        let answer = |code: u32, background: Option<bool>, autostart: Option<bool>| {
            let results = VariantDict::new(None);
            if let Some(b) = background {
                results.insert_value("background", &b.to_variant());
            }
            if let Some(a) = autostart {
                results.insert_value("autostart", &a.to_variant());
            }
            grant(&Variant::tuple_from_iter([code.to_variant(), results.end()]))
        };
        assert_eq!(answer(0, Some(true), Some(true)), Some(Grant { background: true, autostart: true }));
        assert_eq!(answer(0, Some(true), None), Some(Grant { background: true, autostart: false }));
        assert_eq!(answer(0, Some(false), Some(false)), Some(Grant { background: false, autostart: false }));
        assert_eq!(answer(1, Some(true), Some(true)), Some(Grant { background: false, autostart: false }));
        assert_eq!(answer(0, None, None), None, "a yes that doesn't say what to");
    }
}
