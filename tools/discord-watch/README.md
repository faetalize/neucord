# discord-watch

Discord documents nothing a user client relies on, and changes it without
notice. The official client is the only reference that stays current, so
this follows it: a weekly workflow snapshots the client and opens an issue
when anything Neucord uses changed.

The client is the web build at discord.com/app; the desktop apps load that
same build, so their downloads hold none of it. Every chunk is found through
webpack's chunk map, parsed with acorn and never run.

A snapshot keeps what a third-party client depends on, stripped of minified
names so two builds differ only where Discord changed something:

- REST endpoint paths, and for every request the client makes, the body,
  query and header fields it sends.
- The API version the client speaks, from its page's `API_VERSION`. Neucord
  speaks the same one, since any other sets every request apart (#73).
- The headers its HTTP layer adds to every request, by name. Neucord sends
  `X-Super-Properties` because login needs it (#47); a new header there may
  be the next one Discord insists on.
- Gateway events the client registers handlers for, with the payload fields
  those handlers read. Handlers often pass the payload on whole, so a missing
  field proves nothing.
- The enums anchored in `watch.json`, matched by member names: compiled
  TypeScript enums, and flag tables written as objects, such as the
  permission bits.
- The source of the methods named in `watch.json`, with minified names
  replaced. These are rules Neucord copies rather than data it reads:
  `isSystemDM` is where the client shows no message box, and posting there
  anyway got an account flagged (#51).
- The modules of the stores named in `watch.json`, reduced the same way. A
  store names itself (`static displayName`), the functions holding its
  rules don't: `GuildVerificationStore` decides where the client shows a
  server's verification steps instead of the message box (#53).
- The values named in `watch.json` (a literal, or a list or set of them),
  wherever the client defines or exports them: how many emoji a message
  can be and still show them large, which message types people write.

What Neucord uses is read from `crates/discord`: the endpoint table in
`endpoints.rs`, which every REST request goes through, the event names
`gateway.rs` matches on, and the API version in `identity.rs`. Only `watch.json` is kept by hand.

What can't be snapshotted is in [MANUAL.md](MANUAL.md): another weekly
workflow opens an issue to check it by hand.

Snapshots live on the `discord-snapshots` branch, one commit per build, so
its history is the history of the client. An issue links the full diff.

## Reading the client

`node cli.mjs update <dir>` fetches the current build into a git repo, one
formatted file per webpack module. Module ids survive across builds, so
`git diff` between two commits shows what changed in each module. Run
`npm install` first; formatting needs Prettier.

The client is Discord's code: keep unpacked builds local, never commit them
here.
