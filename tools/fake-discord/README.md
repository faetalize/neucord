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
  `FAKE_IMAGE_FAIL=1` has every image on the CDN answer 404.
  Heartbeats (op 40) are logged and acknowledged, and READY carries a made-up
  `qos_token`. Both settings protos are kept, and a save merges into them field by field
  as Discord's does. The GIF endpoints answer made-up results, one with a
  numeric id, and take a pick's `POST /gifs/select`. A thread made from a
  channel joins the server and comes back as `THREAD_CREATE`. `FAKE_VARIANTS="hiding=1,voice=1,ladder=1,lazy=1,upload=1,nsfwlevel=1,chatbar=1,dice=1"`
  assigns Apex experiments by name, `FAKE_REIDENTIFY=<s>` sends op 9 after
  that many seconds to force a second IDENTIFY, `FAKE_NEW_TOKEN` puts an
  `auth_token` in READY, `FAKE_LEFT=<user ids>` has those people leave the
  server: op 8 with a `nonce` answers them `not_found`, and a search finds
  them no more. `FAKE_SLOWMODE=<s>` gives `#general` that slowmode and
  refuses a thread there with 20016, as inside its cooldown.
  `FAKE_NSFW="allowed=1,dms=0,status=3,gated=0,level=0,owner=0"`
  (the defaults) sets the account's `nsfw_allowed` and age status, the
  age-restricted commands in DMs setting, the region's age-gated features
  and the server's content levels; channel 604 is `nsfw`.
- **`patch.py`**: points the copy's API, gateway and CDN at the fake, and adds
  a command file the scenario writes to: `home`, `guild <id>`,
  `channel <id>`, `attach`, `thread-form` (Create Thread for the open
  channel), `shot <png>`, `shot-popover <widget> <png>` (the
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
  as text, on the clipboard), `clip-text <n>` (n letters of text),
  `clip-page-image <src>` (an image with the HTML a browser copies with it),
  `paste` and `box-length` (the message box's length), `reply [message id]` (the newest
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
  `modal-files <path> [png]` (prints what it shows, and shoots it), `link
  <url>` (clicks that link in a message shown, and says whether Neucord
  followed it or left it to the browser), `voice-draft <seconds>` (a made-up
  recording as the open channel's draft, there being no microphone),
  `voice-play`, `voice-scrub <0..1>`, `voice-state` (the draft player and
  its clock), `voice-recording <s>` (a recording that began that long ago;
  `voice-recording-elsewhere` in #general), `voice-record` (Record Voice
  Message), `voice-blink <s>` (the clock's colour, logged) and `voice-finishing` (the draft as just stopped; run with
  `E2E_NO_PLAYER=1` to leave it no player, whose media stream keeps a window
  shot from finishing here), `vmsg state|play|pause|scrub <0..1>` (the player
  of a voice message in the channel, with `FAKE_VOICE=1`: 8 s of silence in
  #general), `menu-key row|rail <n> [png]` (gives the nth sidebar row or rail
  button the focus, runs its Menu / Shift+F10 shortcut, says whether a menu
  opened and shoots it), `quit`. It edits the copy by exact strings
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
  `thread` (with `FAKE_OWNER=1`) runs /thread, and `thread-slowmode` (with
  `FAKE_OWNER=1 FAKE_SLOWMODE=30`) has it refused, which shows in Create
  Thread and not in the message box; `dice` (with
  `FAKE_VARIANTS=dice=1`) rolls by /roll-dice and by its message's link;
  `bot-commands` (with `FAKE_NSFW=dms=1`, which puts settings proto 1 in
  READY) lists a bot's commands after its mention, and turns them off and
  on from the message box's menu; `member-check` (with `FAKE_LEFT=302`)
  lists Bob in a pill, whose check then drops him; `mentions` (and with
  `FAKE_OWNER=1`, `@everyone` and `@here`) lists the message box's `@`;
  `paste` pastes a short text, a text over the message length (which
  becomes message.txt) and images, one from a page named after its `<img>`;
  `name-stickers` types `:wave` in a server channel (emoji, then the stickers
  that match, under their headers) and picks a sticker, which is sent alone;
  `thread-search` opens the thread browser and searches its names (one request
  with `name` and `tag_setting`, results in place of the lists, emptying the
  box brings them back; `threads`, `thread-search <text>` and `thread-rows`
  are its commands);
  `forum-search` (with `FAKE_FORUM=1`, a forum #ideas with two tags and three
  active posts) types in the forum's search box: one request 350 ms after
  typing pauses, the posts found in place of the list, the list back when
  the box is emptied;
  `poll-votes` (with `FAKE_POLL=1`, a finished poll in #general whose first
  answer has 130 voters) opens the list of who voted (`poll-votes <message>
  <answer>`) and asks for the first page of voters (`limit=100&type=2`);
  `hover-bar` (with `FAKE_HOVER=1`, which shows every row's hover bar) shows
  the bar over a message and clicks its first quick reaction (`hover-react
  <n>`), which sends `location=Message Hover Bar`;
  `thread-previews` (with `FAKE_THREADS=1`, two active threads of #general
  synced with THREAD_LIST_SYNC, one with its newest message) opens the thread
  browser, whose rows show that message or "No recent messages";
  `top-emoji` (with `FAKE_TOP_EMOJI=1`, two custom emoji and the top-emojis
  route) opens the emoji picker (`emoji-picker`; `shot-popover emoji_button`)
  and shows "Top Emoji in Test Server" first;
  `emoji-packs` (with `FAKE_NITRO=1 FAKE_EMOJI_PACKS=1 FAKE_EMOJI_IMAGES=1
  FAKE_VARIANTS=packs=1`: Nitro, two packs on `GET /emoji-packs` (without
  `FAKE_EMOJI_PACKS` the route is Discord's 404), the experiment on, and
  custom emoji served as stand-in pictures) opens the picker, then its packs
  page (`emoji-packs`, `packs-shot <png>`), pins the first pack with its
  button (`packs-pin <n>`, `PinnedEmojiPacks` in settings-proto 2), closes
  the page (`packs-close`) and shows the picker's "Blobs" section;
  `heartbeat` waits for five op 40s, the interval's among them;
  `reopen` and `lazy` relaunch on the same data
  (`ACCOUNT=<name>` keeps it between runs).

Add the endpoints, events and commands a change needs here, in the same pull
request.
