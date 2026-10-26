# Neucord

Another GTK Discord client? Daring today, are we...?

> [!WARNING]
> Neucord is a third-party client. Use it at your own risk.

## What doesn't work yet

Reactions, editing or deleting your own messages, uploads, emoji and
sticker pickers, threads and forums, the member list, voice and video.
Custom emoji show as `:name:` and video attachments as links. Logins that
Discord answers with a captcha, and two-factor with security keys. Planned
work is in the issue tracker.

## Requirements

- GTK 4.24 or newer
- libadwaita 1.7 or newer
- libsoup 3
- libsecret, and a Secret Service provider such as GNOME Keyring or KWallet
- OpenSSL
- Rust 1.92 or newer, to build

On Arch Linux:

```sh
sudo pacman -S --needed rust gtk4 libadwaita libsoup3 libsecret openssl
```

## Building

```sh
git clone https://github.com/faetalize/neucord
cd neucord
cargo run --release
```

## Logging in

Scan the QR code with the Discord app on your phone, or use your email and
password. With the QR code, your password never passes through Neucord.
Discord sometimes answers a password login with a captcha, which Neucord
can't show yet; use the QR code then.

You can still log in with a token. Guard it: anyone who has it controls your
account SO NEVER SHARE IT ANYWHERE!

## Privacy

Neucord connects only to Discord's API, gateway and CDN. There are no
analytics, no crash reporting, and none of the tracking calls the official
client makes. It identifies itself to Discord as Neucord instead of
impersonating the official client.

Link previews and GIFs load through Discord's media proxy, never from the
site they link to, so those sites don't see your IP address.

Your token is stored in the system keyring. Images are cached in
`~/.cache/neucord/images/`, and embedded videos you play in
`~/.cache/neucord/videos/` (up to 256 MiB).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
