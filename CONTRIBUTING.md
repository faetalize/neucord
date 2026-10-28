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

## Checks

CI runs `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and
`cargo test` on every pull request. Run them before you push.

## Debugging

- `NEUCORD_DEBUG_SHAPES=1` prints the structure of every gateway event
  (field names and value types, never values), for checking parsers against
  what Discord actually sends.
- `NEUCORD_DEBUG_DROP=<seconds>` drops the gateway connection that long after
  each connect, to exercise reconnecting and resuming.

## Privacy

Neucord handles other people's messages and a credential with full account
access. Never log tokens, message contents or user data, and never commit
real IDs, names or payloads: use made-up test data.

## Pull requests

One feature or fix per pull request, against `master`, linked to its issue.

If it makes Neucord rely on something new in the official client (a field,
an enum value, a rule such as which channels take messages), extend
`tools/discord-watch` in the same pull request, so the weekly check notices
when Discord changes it. If it can't be snapshotted, such as a rule in an
unnamed function, add it to `tools/discord-watch/MANUAL.md`, which gets
checked by hand every week. If you're not comfortable doing so, just mention
it in the PR body and I'll handle it.

Name the branch `<type>/<issue>-<what it does>`, for example
`feature/21-qr-login`. The type is one of `feature`, `fix`, `docs`,
`refactor` or `ci`. Pick the name before opening the pull request: renaming
a branch that has one closes it.
