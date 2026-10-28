# Neucord

Another GTK Discord client? Daring today, are we...?

> [!WARNING]
> Neucord is a third-party client. Use it at your own risk.

## What doesn't work yet

Reactions, editing or deleting your own messages, uploads, emoji and
sticker pickers, threads you haven't joined, forum post lists and new
posts, the member list, voice and video.
Polls and bots' buttons show but can't be used, Lottie stickers show only
their name, and `ansi` code blocks lose their colours. Two-factor with
security keys. Steps Discord sometimes requires, such as verifying a phone
number, accepting new terms or changing your password: Neucord waits while
you take them in the official app or at discord.com. Planned work is in the
issue tracker.

## Requirements

- GTK 4.24 or newer
- libadwaita 1.7 or newer
- libsoup 3
- WebKitGTK 6.0, for captchas
- GtkSourceView 5, to highlight code
- libsecret, and a Secret Service provider such as GNOME Keyring or KWallet
- OpenSSL
- Rust 1.92 or newer, to build

On Arch Linux:

```sh
sudo pacman -S --needed rust gtk4 libadwaita libsoup3 webkitgtk-6.0 gtksourceview5 libsecret openssl
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
Discord sometimes asks for a captcha first, which Neucord shows in a
window.

You can still log in with a token. Guard it: anyone who has it controls your
account SO NEVER SHARE IT ANYWHERE!

## Privacy

Neucord connects only to Discord's API, gateway and CDN, and to hCaptcha
when Discord asks for a captcha. Captchas run in a private web view that
keeps nothing once it closes. There are no analytics, no crash reporting,
and none of the tracking calls the official client makes. It identifies
itself to Discord as Neucord instead of impersonating the official client.

Link previews and GIFs load through Discord's media proxy, never from the
site they link to, so those sites don't see your IP address.

Your token is stored in the system keyring. Images are cached in
`~/.cache/neucord/images/`, and the videos and sound you play in
`~/.cache/neucord/videos/` (up to 256 MiB).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).
If you open a PR using an AI agent, be upfront about it; I won't bite.

## LLM Disclosure

This project is NOT vibecoded NOR mainly LLM written, however a lot of the tooling surrounding the project itself is LLM assisted!
Documentation, like the README you're currently reading, is written by me.
However, issue creation and feature discovery might be LLM spear-headed, because fuzzing through a large project such as Discord suuuuucks. I don't wanna write an issue for every single lil Discord quirk I discover.
I have an LLM doing the decomp work and diffing with the real client, alongside my own automated API diffing tools. Trust me, it helps. It'll help you avoid account bans due to missed fields or unsent headers, etc.
Finally, when it comes to actually writing the code, I personally do it.
I do press tab sometimes when VSCode's completions are good.... sometimes...

## License

GPL-3.0-or-later. See [LICENSE](LICENSE).
