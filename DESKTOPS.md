# Desktops

Neucord is built on niri. This is what to look at on the others, headless
first, then by hand where a screen or a person is the only judge.

## Headless

`tools/fake-discord/run.sh desktop <run>` runs one look at the app under the
compositor in `COMPOSITOR` (see its README): `mutter` (the default),
`gnome-shell` (GNOME's whole shell, headless), `niri` (nested, stock
configuration), or `host` for one that is running already, which is how sway
and KWin are reached (`HOST_DISPLAY=/path/to/its/socket`). The run leaves
shots of the window by the app and, where the compositor allows it, by the
compositor, the notification call the app made, and the names it put on the
bus. It sees layout at the size the compositor gives, popovers and menus, the
notification and tray registration. It can't see a frame a compositor draws,
what clicking a notification does, or a keyring: the harness's own sandbox
has no working Secret Service.

## By hand

What the app does is in the code; what the desktop does about it is for the
eyes.

- **Frames.** GTK 4 never asks for server-side decoration, so the window is
  always drawn client-side (`App::new`). Right is one header bar and no
  second title bar. A compositor that adds its own on top, or a window with
  no shadow or rounded corners where the desktop has them, is the thing to
  look for, most likely on sway and KWin.
- **Blur.** The bars blur what scrolls under them inside the app
  (`backdrop-filter`, set by Appearance's translucent switch), not through
  the compositor, so a compositor's own blur changes nothing. Look at it on a
  software renderer or a VM, where it costs frames.
- **Notifications.** The app speaks to `org.freedesktop.Notifications`
  itself (`desktop.rs`): clicking one opens the channel, and a reply typed
  into it sends, where the server offers `inline-reply`. Check both, and that
  a server without it still shows the notification.
- **Secret Service.** Tokens are kept only in the keyring (`session.rs`).
  Right is staying signed in across a restart. With no provider the app says
  `could not save the accounts` and holds the account for that run only,
  which is what a session without a provider gets (nothing on sway starts one).
- **Tray.** A StatusNotifierItem on the session bus (`tray/mod.rs`); GNOME
  shows none without the AppIndicator extension. Right is the icon with its
  unread state and menu where a panel hosts one, and the window never hiding
  where none does.
- **Portals.** Attaching, saving and opening a link go through
  `GtkFileDialog` and `GtkUriLauncher`, so the desktop's own dialog appears if
  its portal backend is there. Wherever a backend lacks the file chooser,
  look at which dialog you get and whether it works.
- **Scaling.** Images are asked for at the window's scale
  (`Images::set_scale`). At 125% and 150% look at avatars, emoji and pictures
  for blur or a doubled edge, and move the window between outputs of
  different scales.
- **Input methods.** The message box under ibus and fcitx5: dead keys, compose,
  the preedit's place by the caret, a candidate popup not hidden by the
  window.
- **Menus.** Right-click, the Menu key and Shift+F10 on a focused row open the
  menu where the pointer or the row is. Long-press on message text on a real
  touchscreen is the open question of #81: the selectable text may take the
  press, and an issue is due if it holds.
