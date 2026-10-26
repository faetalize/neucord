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
- Gateway events the client registers handlers for, with the payload fields
  those handlers read. Handlers often pass the payload on whole, so a missing
  field proves nothing.
- The enums anchored in `watch.json`, matched by member names.

What Neucord uses is read from `crates/discord`: `(method, path)` pairs and
the event names `gateway.rs` matches on. Only `watch.json` is kept by hand.
Calls are recognised by how they are written, so an HTTP method the tool
cannot tie to a path fails the job rather than going unwatched.

Snapshots live on the `discord-snapshots` branch, one commit per build, so
its history is the history of the client. An issue links the full diff.

## Reading the client

`node cli.mjs update <dir>` fetches the current build into a git repo, one
formatted file per webpack module. Module ids survive across builds, so
`git diff` between two commits shows what changed in each module. Run
`npm install` first; formatting needs Prettier.

The client is Discord's code: keep unpacked builds local, never commit them
here.
