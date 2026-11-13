# Contributing to Neucord

If you plan something large, comment on its issue first so work isn't
duplicated.

## Layout

`crates/discord` is the Discord protocol: model, REST, gateway, read states,
notification rules, settings, markup parsing. It runs on the GLib main loop
but never depends on GTK, so all of it is testable without a display.

`crates/app` is the GTK application. Its UI is split into modules by area of
the window under `src/ui/`.

Anything that can be decided without GTK (parsing, rules, ordering, merging)
belongs in `crates/discord` or another plain-Rust module, with unit tests.
Keep UI code thin.

## Threads

The GLib main loop does everything that touches GTK or shared state, and
never blocks: a frame is 8 ms. Work that can (disk, parsing a payload of
megabytes, converting an image) runs off it and comes back as plain owned
data, applied on the main loop in one go. Nothing that is a GObject or an
`Rc` crosses. A task with a result is `gio::spawn_blocking`, called from the
main loop and awaited there; a thread of its own is for what has to stay in
order, as the gateway's frames do (`gateway.rs`). Decoding a texture stays on
the main loop for now: gtk4-rs only lets `gdk::Texture::from_bytes` run
there.

## Checks

CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` on every pull request. Run them before you push.

Keep extra build directories (a second toolchain, a scratch copy) under
`target/`, which git ignores, and look at `git status` before committing: a
build directory once got committed to `master` by a merge.

## Testing

A bug gets a regression test first: run it alone and see it fail on the wrong
behaviour (a compile error or a skip proves nothing), fix, see it pass, then
run the whole suite. If the logic has to move to be testable, move it without
changing what it does, so the test fails against the real code. Behaviour
only a compositor or GPU shows gets no fake test: reproduce it by hand and say
so in the pull request.

To see a change work, or read exactly what Neucord sends, run it against
[`tools/fake-discord`](tools/fake-discord), never a real account. Any headless
run of the app goes inside `dbus-run-session`, or its notifications land on
your desktop.

## Code

Comments and docs say what is, not what changed (that's the commit
message's job), densely: no line that restates the code. New widgets start on
GTK and libadwaita defaults: no CSS until there's a reason for it.

## Debugging

- `NEUCORD_DEBUG_SHAPES=1` prints the structure of every gateway event
  (field names and value types, never values), for checking parsers against
  what Discord actually sends, and what IDENTIFY sent: its capabilities and
  the names of its properties.
- `NEUCORD_DEBUG_DROP=<seconds>` drops the gateway connection that long after
  each connect, to exercise reconnecting and resuming.
- `NEUCORD_DEBUG_EXPERIMENTS=<dir>` writes, on READY, the account's
  experiment assignments and server rollouts, and READY's shape, into
  `<dir>`.
- `NEUCORD_DEBUG_UNKNOWN_MESSAGES=1` puts a line in the chat for every system
  message type Neucord has no line for, with its number, including the ones
  it hides. If the official client draws nothing there either, ignore it.

## Privacy

Neucord handles other people's messages and a credential with full account
access. Never log tokens, message contents or user data, and never commit
real IDs, names or payloads: use made-up test data.

## What every issue and pull request looks at

Neucord follows the official client, so each issue and pull request checks
it for five things:

- **Account safety.** Neucord sends nothing the official client wouldn't,
  and holds back where it does: a request no official client makes is how
  an account gets flagged. Look at how the client sends anything Neucord
  hasn't sent before.
- **Drift.** If Neucord comes to rely on something new in the official
  client (a field, an enum value, a rule such as which channels take
  messages), extend `tools/discord-watch` in the same pull request, so the
  weekly check notices when Discord changes it. If it can't be snapshotted,
  such as a rule in an unnamed function, add it to
  `tools/discord-watch/MANUAL.md`, which gets checked by hand every week.
  Important: the existence of MANUAL.md is not an invitation to bloat it 
  everytime needlessly. Only add an entry in MANUAL.md if it is necessary.
  LLMs tend to be trigger happy and always find a reason to add something to
  a file. Make sure any addition to MANUAL.md has to fight for its right to
  exist.
- **Identity.** Anything that changes what Neucord says about itself (the
  properties, a header, a field of IDENTIFY, a gateway op tied to who the
  client is) updates [IDENTITY.md](IDENTITY.md) in the same pull request.
- **Limits.** Nitro, a server's boost level and Discord's experiments decide
  much of what an account may do: how long a message may be, how large an
  upload, which emoji and stickers work outside their server, how well it
  may stream. `discord::limits` knows them, read from the client. Anything
  Neucord sends that the client gates checks there first and holds back as
  the client does, before any request, and never works around one. A
  feature Discord rolls out through an experiment shows only where Discord
  assigned it (`discord::experiments`). If the client gates something
  `limits.rs` doesn't know yet, the same pull request adds it there, with
  its tracking.
- **Context menus.** Say whether the official client offers the feature in
  a context menu: on which surface, which items, under what conditions. If
  Neucord has that surface, the pull request adds the entries to its table
  in `crates/app/src/ui/menus.rs`; if not, the issue notes them for when it
  does.

If you're not comfortable doing so, just mention it in the PR body and I'll
handle it.

## Pull requests

One feature or fix per pull request, against `master`, linked to its issue.
A fix found while testing an open pull request goes into that pull request.
Don't start an issue while one blocking it is open.

The body says what it does, then the five checks above as **Account
safety.**, **Drift.**, **Identity.** (if it applies), **Limits.** and **Context
menus.**, then **Testing.**: what you ran, and what you couldn't verify. Show
anything visible with screenshots from `tools/fake-discord` (made-up data
only), committed to the `pr-screenshots` branch under your branch's name and
linked from the body, never to the feature branch.

A pull request closes its issue (`Closes #N`) only if it finishes it. If it
leaves part of it, or a question for me (something left out, matched one
way, a call to make), it says `Refs #N`, lists what's left, and the question
goes on the issue as a comment, so it isn't lost when the pull request
merges. An open-ended issue (performance, polish) takes passes: each is
`Refs #N` and says what it found.

Work that depends on another open pull request is based on that pull
request's branch and says so. When the base merges, retarget it to `master`
before the base's branch is deleted, or GitHub closes it; `Closes #N` only
links once it targets `master`. To bring a branch up to date, merge `master`
into it rather than rebasing, and run the checks again after every merge.

Name the branch `<type>/<issue>-<what it does>`, for example
`feature/21-qr-login`. The type is one of `feature`, `fix`, `docs`,
`refactor` or `ci`. Pick the name before opening the pull request: renaming
a branch that has one closes it.
