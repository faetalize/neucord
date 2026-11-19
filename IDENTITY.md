# Identity

What Neucord tells Discord about itself, and why. The values live in
`crates/discord/src/identity.rs`; this records the decisions behind them, so
a change is a choice and not drift. Whatever changes here changes this file in
the same pull request.

Neucord presents as **Discord's web app in a browser named Neucord**. That is
a choice, not a promise: the first section holds whatever it presents as, the
second only holds because of that choice, and is what to revisit if it ever
presents as something else.

## Always

- It names itself. `browser` is `Neucord`, and so are the user agent and the
  version. It never says Chrome, Firefox or the desktop app, and never copies
  a binary's fingerprint (TLS, HTTP/2 settings, header order): half a
  disguise contradicts itself, where one honest layer cannot.
- Every value is true of Neucord or empty. Where the web client reads a
  browser (referrer, OS version, mods on `window`), Neucord sends what a
  browser with none of that would, which is also the truth.
- Nothing is forged, relayed or minted that only another component can make:
  no value for a field an integrity module fills in, no id Neucord didn't get
  from Discord or generate itself.
- No `/science`, no experiment exposures, no analytics of any kind. The
  client reports what it shows and does; Neucord doesn't, and the README
  promises it. The one such report it makes is the GIF picker's
  `POST /gifs/select` (the picked result's id and its search), which the
  client sends with every pick of a result, so a pick goes out as the
  client's does.
- A request the client doesn't make is not sent. A field, header or op is
  added only as the client sends it, in its order and when it does.

## Only because it presents as the web app

These follow from the choice above. If Neucord presented as something else,
each would be reconsidered.

- **The property shape and order** (`X-Super-Properties`, IDENTIFY's
  `properties`): the web client's, field by field, built in one place so
  header and IDENTIFY agree. The header goes on every API request, signed in
  or not, as `prepareRequest` does; IDENTIFY adds the fields only the gateway
  carries.
- **`client_build_number`** names a web build. Without one Discord withholds
  about half of the account's experiments (#152). The number is the build
  Neucord was last compared with, never the live one, so it never claims a
  behaviour that wasn't checked; `discord-watch` reports it beside the live
  build and `raise-build` raises it once a week's changes are handled. A
  build some weeks old is what a tab left open sends.
- **`capabilities`** are the client's, and so is how it decides about hidden
  channels: from the experiment assignments it stored last time, so a first
  launch asks for none. Neucord stores the account's own assignments for
  each account, and `STATE_UPDATE` feeds them as it feeds the client's
  store. Handling the READY layout those capabilities bring is part of
  presenting this way, not a feature.
- **The heartbeat session** (`client_heartbeat_session_id`). The client keeps
  one session, whoever is signed in, and drops it whenever its token changes
  (a login, a logout, a switch, a token READY replaces); only a relaunch of
  the same account continues it. Neucord keeps one slot tagged with its
  account the same way. A window of its own, like a separate browser
  profile, would get a slot of its own.
- **`launch_signature` is `null`.** The web client always has the key; its
  value comes from a native module's WASM that Neucord doesn't have. The key
  is sent, never a value.
- **`X-Discord-Timezone`**, from the system's IANA zone, on every API
  request. Left out when no zone is found, as the client does.
- **`gateway_connect_reasons`** is `AppSkeleton`: the app's view exists
  whenever Neucord connects.
- **RESUME carries the token of the last IDENTIFY**, not the account's
  current one: the client's socket takes its token when it identifies.
  REST uses the new token at once, as `prepareRequest` does.
- **IDENTIFY names the heartbeat session as a request would have**: by the
  time the client identifies it has made requests, and Neucord may not have.
- **Heartbeats are op 40, and op 41 names the heartbeat session**, as the
  client's are and does. Op 40's `qos` says whether the user is active: with
  a token and the window in front it is, for `foregrounded`. The client's
  other reason, `rtc_connected`, needs a call, which Neucord has none of.
  Op 41 goes out with each new session and at each READY.
- **IDENTIFY's `qos_token`** holds the same activity and the `DerivedQosData`
  the last READY gave the account, kept for each account as the client keeps
  it by user id. Neucord sends Discord's signed part back as it came and
  never makes or reads one.
- **Op 3 says when the user is idle or away**, as the web client's
  `IdleStore` does: no mouse button, wheel or typed character, no channel
  opened and no focus change for ten minutes is idle (an Online account then
  shows Idle, with `since` the time it was last active), and none for the
  account's AFK timeout, a minute unless it says otherwise, is away
  (`afk`). Both are checked every 30 seconds. `afk` is what keeps Discord
  from pushing to a phone while we are at the keyboard.

## Deliberately not sent

- **`X-Fingerprint` and the installation id** (`X-Installation-ID`,
  IDENTIFY's `installation_id`). The client takes both from `/experiments`
  calls and READY, and sends them on every request. They tie every request
  to one stored install. That belongs to the larger question in #92 of how
  far to go.
- **Science and exposure events**, including the heartbeat's
  `CLIENT_HEARTBEAT`. See above.
