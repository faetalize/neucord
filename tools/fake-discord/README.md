# fake-discord

Runs Neucord end to end against a Discord that lives on loopback, so a change
can be seen doing what it should, and what Neucord sends can be read, without
a real account. Every account, server, app, message and token in it is made
up.

`run.sh <scenario>` copies this checkout to `target/fake-discord/src`, points
the copy at the fake with `patch.py`, builds it, and runs the scenario inside
`unshare -rn` (no network but loopback) and `dbus-run-session` (notifications
don't reach your desktop), under `mutter --headless`. Output goes to
`target/fake-discord/out/<run>`: `fake.log` (one JSON line per request with
its JSON body and any `X-Audit-Log-Reason` or `X-Context-Properties`, per
gateway frame, per event a request made it push, and IDENTIFY as sent), the
app's and mutter's logs, and any screenshots. `analyse.py <fake.log>` prints
what arrived, IDENTIFY's key order and properties first.

- **`fake_discord.py`**: REST and a `zlib-stream` gateway on 127.0.0.1:8099,
  with READY, `READY_SUPPLEMENTAL`, a server, DMs (one with an app's bot) and
  the rest a session needs. Messages posted are kept and come back as
  `MESSAGE_CREATE`. One command index of made-up apps serves the server, the
  account and the bot's DM; `POST /interactions` answers on the gateway as
  the app would, or as `FAKE_INTERACTION=fail|silent|429|modal|iframe` says (`modal`: one
  of each field a form submits, and Formy's message in channel 600 has a
  button that asks for it), and takes a form as
  `FAKE_MODAL_SUBMIT=fail|silent|429` says. Files get an upload URL at the
  fake, which takes the `PUT` and a `DELETE`, unless
  `FAKE_UPLOAD_FAIL=too-large` turns the URL down with Discord's 40005.
  Both settings protos are kept, and a save merges into them field by field
  as Discord's does. The GIF endpoints answer made-up results, one with a
  numeric id, and take a pick's `POST /gifs/select`. `FAKE_VARIANTS="hiding=1,voice=1,ladder=1,lazy=1,upload=1,nsfwlevel=1"`
  assigns Apex experiments by name, `FAKE_REIDENTIFY=<s>` sends op 9 after
  that many seconds to force a second IDENTIFY, `FAKE_NEW_TOKEN` puts an
  `auth_token` in READY. `FAKE_NSFW="allowed=1,dms=0,status=3,gated=0,level=0,owner=0"`
  (the defaults) sets the account's `nsfw_allowed` and age status, the
  age-restricted commands in DMs setting, the region's age-gated features
  and the server's content levels; channel 604 is `nsfw`.
- **`patch.py`**: points the copy's API, gateway and CDN at the fake, and adds
  a command file the scenario writes to: `home`, `guild <id>`,
  `channel <id>`, `attach`, `shot <png>`, `shot-popover <widget> <png>` (the
  popover showing on `composer`, `composer_box` or `gif_button`),
  `type <text>` (as typed), `set <text>` (as the app sets it),
  `backspace` (GTK's own Backspace at the caret), `suggestions [png]`
  (prints the autocomplete's rows and headers, `*` on the selected one),
  `pick <n>` (accepts row n), `line` (prints the command in the box: its
  text and caret, each pill and whether it is drawn as wrong, the bar,
  "+n" and the buttons beside it), `key <name> [shift|ctrl|alt]…` (a key
  pressed in the message box, by its GDK name such as `Return` or
  `Escape`), `composer-menu [x y]` (opens the message box's menu, as a
  right-click there does, else the Menu key; shoot it with `shot-popover
  composer`), `menu-item <name>` (chooses
  an item of it), `send` (clicks the send button, or says why it can't),
  `clip-image`, `clip-files [path]` (puts an image, or a file with its path
  as text, on the clipboard), `reply [message id]` (the newest
  message without one), `confirm [response]` (the open alert's, else its
  last that doesn't close it), `gif` (opens the GIF picker),
  `gif-search <q>`, `gif-pick <n>`, `gif-shift-pick <n>`, `gif-star <n>`
  (the nth GIF shown, or its star), `gif-tile <n>` (the nth front page
  tile), `gif-back`, `gif-hint [png]` (prints whether the GIF button
  pulses and shows "Added to Favorites", and shoots the hint),
  `press <message> <custom_id>` (a button of
  the open channel's), the top app form's `modal-fields`, `modal-set <path>
  <text>`, `modal-check <path> <n>`, `modal-pick <path> <n>`, `modal-clear
  <path>`, `modal-submit`, `modal-cancel`, `modal-shot <png>`, a file
  field's `modal-file <path> <file>…` (adds them, as a pick or a drop
  does), `modal-unfile <path> <n>` (its row's remove button) and
  `modal-files <path> [png]` (prints what it shows, and shoots it), `quit`. It edits the copy by exact strings
  and stops on the first that no longer matches: when the code it patches
  moves, update the string. The module it adds is only built here, so a
  change to what it calls needs a run to show it still builds.
- **`scenarios/`**: shell run inside the session, with `cmd` (send commands),
  `waitfor <kind> <count>` (wait for the fake to log something) and
  `waitreq <method> <path> <count>` (wait for requests to a path, or to
  paths starting with it). `full` walks the DM list, a DM's attach menu and
  a server channel; `gifs` picks GIFs from a search, Trending and the
  Favorites, and favourites three; `modal-files` (with
  `FAKE_INTERACTION=modal`) adds, takes back and submits a form's files;
  `pills` fills a command's options in the box, searches members (the fake
  answers op 8 with a `query` with its people whose username or name starts
  with it), and ends commands by Backspace, a reply and a channel switch;
  `reopen` and `lazy` relaunch on the same data
  (`ACCOUNT=<name>` keeps it between runs).

Add the endpoints, events and commands a change needs here, in the same pull
request.
