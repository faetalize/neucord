# Neucord

Another GTK + Rust Discord client? Daring today, are we...?

> [!WARNING]
> Neucord is a third-party client. Use it at your own risk.

## What doesn't work yet

Replies, reactions, embeds, editing or deleting your own messages, uploads,
emoji and sticker pickers, threads and forums, the member list, voice and
video. Custom emoji show as `:name:` and videos as links. Logging in only
works with a token. Planned work is in the issue tracker.

## Requirements

- GTK 4.24 or newer
- libsoup 3
- libsecret, and a Secret Service provider such as GNOME Keyring or KWallet
- Rust 1.92 or newer, to build

On Arch Linux:

```sh
sudo pacman -S --needed rust gtk4 libsoup3 libsecret
```

## Building

```sh
git clone https://github.com/faetalize/neucord
cd neucord
cargo run --release
```

## Logging in

Until proper login is in place, Neucord needs a Discord token, and I don't
recommend logging in at all. If you want to beta test anyway, guard your
token: anyone who has it controls your account SO NEVER SHARE IT ANYWHERE!

## Privacy

Neucord connects only to Discord's API, gateway and CDN. There are no
analytics, no crash reporting, and none of the tracking calls the official
client makes. It identifies itself to Discord as Neucord instead of
impersonating the official client.

Your token is stored in the system keyring. Images are cached in
`~/.cache/neucord/images/`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
