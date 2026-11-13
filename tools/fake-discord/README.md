# fake-discord

Runs Neucord end to end against a Discord that lives on loopback, so a change
can be seen doing what it should, and what Neucord sends can be read, without
a real account. Every account, server, message and token in it is made up.

`run.sh <scenario>` copies this checkout to `target/fake-discord/src`, points
the copy at the fake with `patch.py`, builds it, and runs the scenario inside
`unshare -rn` (no network but loopback) and `dbus-run-session` (notifications
don't reach your desktop), under `mutter --headless`. Output goes to
`target/fake-discord/out/<run>`: `fake.log` (one JSON line per request,
gateway frame and IDENTIFY as sent), the app's and mutter's logs, and any
screenshots. `analyse.py <fake.log>` prints what arrived, IDENTIFY's key order
and properties first.

- **`fake_discord.py`**: REST and a `zlib-stream` gateway on 127.0.0.1:8099,
  with READY, `READY_SUPPLEMENTAL`, a server, DMs and the rest a session
  needs, and made-up GIF results (one with a numeric id) that log a pick's
  `POST /gifs/select`. `FAKE_VARIANTS="hiding=1,voice=1,ladder=1,lazy=1,upload=1,chatbar=1"`
  assigns Apex experiments by name, `FAKE_REIDENTIFY=<s>` sends op 9 after
  that many seconds to force a second IDENTIFY, `FAKE_NEW_TOKEN` puts an
  `auth_token` in READY.
- **`patch.py`**: points the copy's API, gateway and CDN at the fake, and adds
  a command file the scenario writes to: `home`, `guild <id>`,
  `channel <id>`, `attach`, `shot <png>`, `popover <png>`, `quit`. It edits
  the copy by exact strings and stops on the first that no longer matches:
  when the code it patches moves, update the string.
- **`scenarios/`**: shell run inside the session, with `cmd` (send commands)
  and `waitfor <kind> <count>` (wait for the fake to see something). `full`
  walks the DM list, a DM's attach menu and a server channel; `reopen` and
  `lazy` relaunch on the same data (`ACCOUNT=<name>` keeps it between runs).

Add the endpoints, events and commands a change needs here, in the same pull
request.
