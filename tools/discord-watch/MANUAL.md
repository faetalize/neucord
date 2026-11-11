# Checked by hand

Some of what Neucord copies from the official client can't be snapshotted:
rules in unnamed functions, or spread over several modules. They're listed
here, and checked every week by a person or an agent. A "Manual drift check"
issue opens each Monday, whatever `watch.json` caught.

Only what has to match is listed: what Neucord sends to Discord or holds
back, and whether something shows at all. How Neucord words, sizes or lays
things out is its own design, so none of that is here.

Changing this file means one of two things: a new client rule Neucord copies
that nothing else tracks, or keeping a module id, anchor or name correct. A
line that explains a choice in Neucord or warns the next reader is a code
comment, not an entry. If in doubt, ask FAE!!!

Last checked against: stable 668923.

## Running a check

1. `node tools/discord-watch/cli.mjs update ~/Source/Discord-Decomp`. The
   decomp keeps one commit per build. Most module ids survive a build, but a
   build can renumber thousands, and an old id can then hold other code.
2. For each module below, diff it between the build above and the new one:
   `git -C ~/Source/Discord-Decomp diff <old> <new> -- modules/<id>.js`.
3. If a module is gone or the anchors no longer find the code, search the new
   build for the anchors and update the entry.
4. With Neucord built from master and logged in, and no other instance
   running, run `node tools/discord-watch/cli.mjs account
   ~/Source/Discord-Decomp target/debug/neucord`. It prints what Discord
   sends the account that moved since the last check.
5. Say in the issue what changed and whether Neucord now acts differently
   from the client. Open an issue for each real difference, then commit the
   new build number above and close the check.

Anchors are names and strings that survive minification. A module that
changed for unrelated reasons only needs a note in the issue.

## Sending, and what the client holds back

- **Our permissions** (`permissions.rs`), which decide what Neucord shows,
  sends and holds back below. The bits, `PermissionStore` (when they're
  recomputed) and `GuildRoleStore` are watched. The computation isn't:
  488926 (anchor `lurkerPermissionsMask`) gives the owner everything, then
  adds roles to `@everyone`, applies overwrites in order, cuts quarantined
  and timed-out members unless Administrator, holds pending members and
  guests, and takes the moderation permissions (the list in 818348 next to
  the permission bits) where the server requires 2FA and the account lacks
  it. The owner check is in 260509; verification makes its own
  (`verification::gate`, in the watched `GuildVerificationStore`).
- **A timeout leaves only reading**, and administrators see no notice
  (`Guild::timeout_left`). Client: 316031 (the `communicationDisabledUntil`
  check), 488926 (the cut), `renderMessageBanner` in 699344 (the notice
  unless `can(ADMINISTRATOR, guild)`), and the 10-second expiry sweep in
  592713 (`clearGuildMemberTimeout`).
- **When a slowmode cooldown starts** (`Slowmode` in `cooldowns.rs`). The
  store itself is watched (`SlowmodeStore`). Who skips it is in 960850:
  Bypass Slowmode in the channel (`cooldowns::bypasses_slowmode`). What
  feeds it isn't watched either: 138399
  resets the cooldown only after a send succeeds and reads `retry_after` on
  `SLOWMODE_RATE_LIMITED`.
- **When we say we're typing, and who is listed** (`typing.rs`).
  `handleTextareaChange` in 699344 sends for any change to a box that isn't
  empty, unless the text starts with `/` or is the `+:name:` reaction
  shortcut, and stops when the box is emptied. 620014 (anchor
  `isBlockedOrIgnored`) lists everyone typing but us and whom we blocked or
  ignored (`typing::shown`).
- **What replaces the message box** (`update_notice_bar`). Client:
  `renderMessageBanner` in 699344 picks the banners. Otherwise the box is
  wrapped by 544990 in DMs (the system DM, a blocked user) and by 678655 in
  servers, where membership screening shows for a `pending` member when the
  server has `MEMBER_VERIFICATION_GATE_ENABLED` (anchor
  `missingVerificationRole`). In a server the box is disabled without Send
  Messages in the channel, or while pending (989161, anchor
  `canEveryoneSendMessages`; `permissions::can_send`). The order and wording
  of Neucord's notices are its own.
- **Who may post in a thread** (`permissions::in_thread`, through
  `in_channel`). The bits and thread types are watched, the rule isn't: `x`
  in 488926, which `G` calls for a channel in `E.Le` (95701's thread types)
  after returning nothing when the parent is missing or being deleted. A
  `MEDIA_THREAD` only reads; a private thread we haven't joined gives
  nothing without Manage Threads, unless we're a guest; otherwise the
  parent's permissions, with Send Messages set by Send Messages in Threads
  and cleared in a locked thread without Manage Threads. The box also goes
  for `UJ` in 406704 (anchor `isArchivedLockedThread`), which `x` already
  covers. Posting in an archived thread sends nothing first:
  `sendMessage` in 138399 doesn't call `unarchiveThreadIfNecessary`
  (7909), while edits and deletes do.
- **Starting a thread** (`thread_creation.rs`, `ui/create_thread.rs`). The
  request, its error codes, the permission bits and `HAS_THREAD` are watched
  (`POST CHANNEL_THREADS` carries the body of both endpoints). Not watched:
  who is offered it, `C`, `n` and `Tb` in 406704 (anchor
  `CREATE_PRIVATE_THREADS`); the form in 289287 (anchor
  `useDefaultThreadName`), which wants a name unless the thread is from a
  message and caps it at 100; the default name `V` in 971389 (anchor
  `rawTitle`), the archive time `Gl` in 268761 (anchor
  `defaultAutoArchiveDuration`) and the `location`s, "Plus Button" in 747926
  and "Message" in 519222. The first message goes into the thread once the
  gateway has brought it (`Y` in 971389).
- **Editing and deleting** (`permissions::can_edit`, `can_delete`;
  `ui/edits.rs`, `edits::decide`). The requests are watched (`editMessage`, `deleteMessage`,
  `unarchiveThreadIfNecessary`), as are `canDeleteOwnMessage` and
  `UNDELETABLE`. When each is offered isn't: Edit in 697470 (anchor
  `referralTrialOfferId`), Delete in 483768 (anchor
  `isModeratorReportChannel`), both only where `Id` in 406704 holds, and
  Edit not under `s5` there. An emptied edit asks to delete instead, an
  unchanged one sends nothing (205120, anchor `onConfirmDelete`), both on
  the box untrimmed, and what it sends drops only trailing empty lines
  (`ignoreTrailingEmptyNodes` in 84046's submit); a crossposted one asks
  first (91624).
- **Voting on polls** (`Poll::view`, `Poll::set_my_votes`, `permissions::vote_held_back`,
  `polls::END_POLL`). The requests, and what tapping, submitting and ending
  do, are watched (`handlePollAnswerTapped`, `handlePollSubmitVote`,
  `handlePollActionTapped`, `endPollEarly`). Not watched: what a voter is
  offered, `T` and the view in 951727 (anchor `tapShouldOpenVotersModal`: when
  answers can be tapped, which button shows and when Vote is enabled, which
  a quarantine or a timeout turns off); the counts changing before Discord
  answers, `D` in 180662 (anchor `optimistic`: what we had and drop goes
  first, then the new picks); and who is offered End Poll Now, in 761304
  (anchor `end-poll-early`: our own poll, until its expiry).
- **Pressing buttons and picking from menus** (`interaction.rs`, `Rest::interact`,
  `permissions::components_blocked`, `ui/components.rs`). The endpoint, the
  gateway events, `InteractionStore` and the component state store are
  watched. Not watched: the body, which 964363 builds in a variable (`S`,
  anchor `canQueueInteraction`: `application_id` is the message's or its
  author's, a DM's `guild_id` is `null`, a menu's data spreads its `type` and
  `values`), and how a refusal reads (`T` just below); when a click sends,
  `V` in 232430 (anchor `STOREFRONT_PREMIUM_BUTTON`: a premium button opens
  the store instead) and a menu, `nt` there (anchor `onSelectInteraction`: one
  to pick sends when chosen, several when the list closes with a change); and
  what turns components off, `v` in 911408 (anchor
  `getInteractionComponentState`) with `O` and the menu limits in 257249.
- **`ansi` code blocks** (`markup::ansi`, `render::ansi_markup`). Which codes
  read, and how: the parser in 626551 (anchor `brightMagenta`): the style
  codes it takes, in what order a sequence's resets and sets apply, that every
  escape sequence is dropped and that `ESC[m` does nothing. What is drawn is
  `OT` in 768947 (anchor `textDecorationLine`): colours, bold, italic,
  underline, strikethrough and overline, and no dim, reverse or hidden. The
  block is `ansi` when 768947's `py` (lowercased) says so. The colours are
  Neucord's own, from libadwaita's palette, not the client's theme.
- **A sticker's image and its details** (`Sticker::image`, `still`, `link`,
  `stickers::MESSAGE_DETAILS`). The address is `zg` in 378058 (anchor
  `STICKER_ASSET`): a file per format, `size` twice the shown size up to 2×
  and a power of two, Lottie as its JSON; a preview takes a GIF as its PNG and
  an APNG with `passthrough=false`, except on Android. An APNG in a message
  comes animated. Right-clicking a sticker in Developer Mode shows Message
  Details in place of Copy Message ID: 761304 (anchor `message-details`).
- **Forum and media channels take no message** (the forum page in
  `select_channel`, `permissions::can_send`, `update_notice_bar`). Neucord
  shows no box there, loads nothing and refuses to send, whatever Send
  Messages says. Client: 289287 renders a forum view instead of the chat
  for `isForumLikeChannel` (anchor `"forum channels must exist within a
  guild"`), so no message box.
- **Asking for members** (`members::Members`, `Gateway::request_members`),
  whose roles colour names. Which members the client asks for, and when
  again, is watched (`GuildMemberRequesterStore`), and so is the payload
  (`requestGuildMembers`). Splitting into 100 users a request and sending
  nothing before the session is established are in the `GUILD_MEMBERS_REQUEST`
  handler of 225123.
- **Member lists** (`subscriptions.rs`, `member_list.rs`, `ui/members.rs`).
  What is subscribed to and when (`GuildSubscriptionsStore`,
  `updateGuildSubscriptions`), how rows apply and which list a channel shows
  (`ChannelMemberStore`) are watched. Not watched: the rows asked for as the
  list scrolls, `NJ` in 361610 (anchor `GUILD_SUBSCRIPTIONS_CHANNEL`), the
  first page and the five channels kept (36124), the 50 ms wait
  (`updateSubscription` in 289287, anchor `updateMaxContentFeedRowSeen`), the
  hash that names a list (`v3` in 181370, called by the store's `D`), and
  which channels have a list: `renderSidebar` in 289287 (anchor
  `private-channel-recipients`), with a group DM's order in `sS`.
- **How much history is fetched** (`discord::history`, `Page::query`).
  `FONT_SIZES`, the page around a message (`focusMessage`) and how many
  messages stay loaded (`truncate`) are watched. The rest isn't, and its
  limits are numbers written into the code: the first page is `o` and `h`
  in 326337; the rest is in the scroll manager in 567008 (anchor
  `loadMorePausedUntilUserScroll`): the size in `loadMore`, the distances in
  `getOffsetToTriggerLoading`, what holds loading back in
  `isScrollLoadingDisabled`, and what lifts a pause in `handleMouseUp` and
  `handleUserScrollGesture` with its keys in `il`. What a page around a
  message says about the channel's ends, and the query's order, are in
  `fetchMessages` in 138399.
- **When opening a channel fetches it again** (`discord::held`,
  `Timelines::reopen`). `P` in 123363 (anchor `Found expired attachment link`)
  asks for no page of a channel whose messages are `ready` and not `cached`,
  except the system DM when it has unread messages, and throws away one with an
  expired link: `$r` in 679164, which ends a signed link an hour before its
  `ex`. `$` there (anchor `MessageManager.staleFetch`) asks again for a page
  that landed stale, for the open channel, at most every ten seconds a channel.
  `lastTimeConnectedChanged`, which decides stale, is written by the
  `CONNECTION_OPEN` and `CONNECTION_CLOSED` handlers in 225123 (anchor
  `connection closed dispatched`); an ordinary close is `CONNECTION_INTERRUPTED`
  and moves nothing. What else moves `ready` and `cached` is in the watched
  `MessageStore` and `loadComplete`.
- **How the gateway connects** (`GATEWAY_QUERY` in `identity.rs`,
  `inflate.rs`). What each compression asks for is watched (`getAlgorithm`,
  and `usesLegacyCompression` for IDENTIFY's `compress`). Which one the
  client picks isn't: all in 868, `F` (anchor
  `isDiscordGatewayPlaintextSet`) takes the first whose `canUse` holds, and
  zstd needs the desktop app's `discord_zstd` module (`U` just above), so a
  browser gets `zlib-stream`. The query is built in `_connect` (anchor
  `searchParams.append("compress"`): `encoding`, `v`, then `compress`.
- **A login that must change its password** (`auth::check_required`). The
  login store is watched (`LoginRequiredActionStore`). The forced modal, and
  logging out when it's closed, are in `handleConnectionOpen` in 165685.
- **Confirming a friend request from a stranger** (`accept_friend`,
  `confirm_stranger` in `ui/friends.rs`). Client:
  `maybeConfirmFriendRequestAccept` in 393426, and the stranger state in
  994500 (`isStranger`).
- **Captchas** (`Captcha` in `rest.rs`, `ui/captcha.rs`). The retry is
  watched (`interceptResponse`). The fields read are in 295562, and the
  services in 319400 (hCaptcha, reCAPTCHA, reCAPTCHA Enterprise, Turnstile).
- **QR login retries** (`on_qr`, `qr_failed` in `ui/login.rs`;
  `remote_auth.rs`). How often Neucord reconnects to the remote auth
  gateway. Client: 785161 (logger `useAuthWebsocket`), with the backoff in
  158390.
- **When Reply is offered** (`Message::is_replyable`,
  `permissions::can_reply`). `REPLYABLE` is watched, but only covers message
  types. 517997 also needs Send Messages and Read Message History in
  servers, and in an archived thread that we can reopen it (`lK` in 406704,
  `permissions::can_unarchive`: Manage Threads if locked, else Send
  Messages in Threads and Send Messages, and Send Messages in the parent);
  and a non-ephemeral message and no timeout, even for administrators.
  The keyboard shortcut uses `Et` there instead, without the Send Messages
  checks.
- **Whether a reply pings** (`ReplyPing::for_click` in `rest.rs`).
  `PendingReplyStore` is watched, but the click rule isn't: Shift or
  replying to yourself turns the ping off, and the toggle shows only in
  servers, on others' messages (`$b` in 519222, anchor
  `shouldMention` next to `shiftKey`). The send reads `shouldMention` alone
  (`getSendMessageOptionsForReply`, watched).
- **Sending files** (`uploads.rs`, `ui/uploads.rs`). The requests, their
  order and the resumable upload are watched (`getUploadPayload`,
  `uploadFileToCloud`, `UploadAttachmentStore`, whose add starts each
  upload), and so are when the conversions run and the hash they send
  (`tryConvertToWebP`, `tryConvertToJpeg`, `convertFileToJpeg`,
  `buildHeadersForMd5`). Not watched, in `images.rs`: the JPEG's quality
  and size limits, `u` in 85582 (anchor `sysimg`); what counts as HEIC or
  JPEG XR and the new name, 452661 (anchor `image/vnd.ms-photo`); that
  files are converted as they are added, before the limits are checked,
  `N` in 111542 (anchor `imageConversionEvaluated`); and the PNG to WebP
  rules, which stay PNG when animated, transparent, indexed or not sRGB, or
  when the lossless WebP differs or is larger (`g` in 673060, anchor
  `pixel_hash_mismatch`, and its ICC reader in 499091). Not watched: the message's
  attachment, `OW` in 292348 (anchor `uploaded_filename`), which names
  `id` by position, `description` once set (the edit dialog hands it over
  for any file), `is_spoiler` only when on, `original_content_type`
  always; what is checked when files are added, `R` in 518960 (anchor
  `FILE_SIZE_LIMIT_EXCEEDED`: an empty file, then a file or the new
  files' total over the limit, then the count); and at send, where 358579
  starts failed uploads again except those Discord called too large. The
  attach menu is 316576 (anchor `channel-attach`, shown with Attach Files
  and Send Messages): Upload Text as File needs text in the box and sends
  only a fenced block's code with its language as the extension (anchor
  `upload-text-as-file`); the picker takes one file in a channel with
  slowmode (`multiple`); alt text is for images and videos, up to 1024
  characters (507536).
- **Voice messages** (`voice.rs`, `ui/voice.rs`). The experiment, the flag and
  the permission bit are watched. Not watched: what offers it, 87207 (anchor
  `voiceMessages`: a private channel or Send Voice Messages, a chat input with
  the button, not forum-like, no files or sticker pending) and the attach
  item in 316576 (anchor `canRecordVoiceMessage`; the button shows whenever
  any of its items does); Alt+R, in 158781 (anchor `RECORD_VOICE_MESSAGE`);
  and the recording, 841817 (anchor `IS_VOICE_MESSAGE`): under 0.5 s is
  dropped, the length limit is `iA` in 518950, the waveform is at most 256
  bytes (`el`, averaged by 934581) of the decibel levels `handleVoiceActivity`
  turns into bytes (198620, anchor `VoiceMessageMediaEngineCapture`), and
  the upload starts at send. The file is the desktop app's native recorder's,
  which no web build holds: 198620 only maps its extension to a type (webm,
  wav, m4a, mp4, caf, else ogg), so a new case there means the container
  changed.
- **Creating polls** (`poll_draft.rs`, `ui/poll_composer.rs`). The body
  (`createPoll`, `sendPollMessage`), the channel types (`POLLS`), the
  durations and the layouts are watched. Not watched: who is offered it,
  `I7` in 969632 (anchor `POLLS.has`: a private channel, else Send Messages
  and Send Polls) with the attach item in 316576 (anchor `canPostPolls`);
  the editor in 50518 (anchor `POLL_EDITOR_VIEWED`): `maxLength` 300 on the
  question and 55 on an answer, ten answers at most, an emoji without text
  refused, and the question to confirm before leaving what was written
  (`Wn` in 969632); and the emoji it picks, which follow chat's rules since
  `Re` in 307731 leaves `POLLS` out of the intentions that refuse other
  servers' emoji.
- **What Nitro, boosts and experiments allow** (`limits.rs`, and its message
  length in `ui/composer.rs`). `R` in 722178, below the perks each premium
  type includes (anchor `ProductCatalog.tsx`), asks the `perks` bitmask when
  the user has one, else that table. A message takes 4000 characters with
  the perk, else 2000 (135621, anchor `? 4e3 : 2e3`). A longer one is turned
  back (710272, anchor `MESSAGE_LENGTH_UPSELL`): a new message once trimmed
  (`handleSendMessage` in 699344), an edit as it stands. The counter shows
  from a tenth of the limit left, and past 2000 with the perk (263582,
  anchor `showsUpsell`). A file takes at least 20 MiB
  (409481), a server gives 10 MiB unless its features raise it (`o2` in
  453771, anchor `uploadSumTooLarge`), premium types give `f5` in 682901
  (anchor `{ 3: { fileSize:`), and a message's total is `jS` in 292348
  (anchor `0x40000000 : 524288e3`). Which stream settings Nitro or boosts
  allow is 676030 (anchor `canStreamWithSettings`), over the presets in
  753070.
- **Which emoji we offer, and what goes into a message** (`emoji.rs`,
  `ui/emoji.rs`, `ui/composer.rs`). The reasons an emoji can't be sent
  (`getEmojiUnavailableReason`) and the roles, names and ranking in
  `EmojiStore` are watched. The picker in 920134 (anchor `emojiGrid`) hides
  `DISALLOW_EXTERNAL` and shows the rest locked. `:`
  autocomplete is `e_` in 496533 (anchor `queryEmojiResults`): two
  characters at least, only emoji we may send. What a message says for a
  `:name:` is the `emoji` rule in 451909 (anchor `validNonShortcutEmojis`),
  and only a `PREMIUM_LOCKED` emoji in it gets the sender told.
- **Which stickers we offer, send, and when** (`stickers.rs`, `limits::sticker`,
  `ui/stickers.rs`). What a sticker needs is `W$` in 361670 (anchor
  `SENDABLE_WITH_BOOSTED_GUILD`, whose enum is watched). The picker's
  sections are `pD` in 256449 (anchor `EMPTY_GUILD_UPSELL`): without Use
  External *Emojis* another server's stickers aren't listed at all. The
  packs are fetched when the sticker view mounts (`XQ` in 256449, whose call
  is `YB` in 631576, anchor `STICKER_PACKS_FETCH_START`); the request and
  the hour they stay loaded (`StickersPackStore`) are watched. A pick sends
  at once only from an empty box with no files and no sticker waiting, else
  the sticker waits and goes with the message (`nm` in 989161, anchor
  `getStickerPreview`).
- **Reacting** (`reactions.rs`, `Rest::add_reaction`, `ui/reactions.rs`,
  `ui/reactors.rs`). The counting on a message (`addReaction`,
  `removeReaction` and the other methods named in `watch.json`), the paths
  and the events are watched, the requests' fields aren't: `C` in 649963
  (anchor `Message Inline Button`) builds the call, so the table doesn't see
  `location` and `type` on the add, `location` and `burst` on the take-back
  (`userId` in its path for someone else's), `limit`, `after` and `type` on
  who reacted (`ao`), or the `location` each caller names (`qN`). `S` there
  is the retry, also of the moderators' removals (`Go`, `Jf`): one wait of
  `parseInt(retry-after)` seconds on a 429, one immediate retry of anything
  but a 403, and a 403 ends it (`TOO_MANY_REACTIONS` also warns). Who may
  react is `A` in 707985 (anchor `disableReactionUpdates`); the message menu
  (Add Reaction, View Reactions, Remove Reactions, Remove All Reactions;
  Manage Messages for the last two) is `en` and the entries in 761304
  (anchor `add-reaction`); who reacted asks for 3 once a pointer has rested
  500 ms on a reaction (`eM` in 30492) and 100 a page in the list (112753,
  anchor `loadMore`). `+:name:` is `reaction` in 751258 (anchor
  `hasPresent`), which sends no message.
- **When the GIF picker asks Discord, and what a pick sends** (`gifs.rs`,
  `ui/gifs.rs`). The endpoints, their query fields and the format asked for
  (`GIFPickerViewStore`) are watched. The rest is in 497685 (anchor
  `GIF_PICKER_QUERY_SUCCESS`) and 649529 (anchor `backToFrontPage`): the front
  page is fetched when it has no tiles (`k8`); a search waits 250 ms after
  typing (`$P`), a category or suggestion doesn't; the trending tile is `Z4`;
  suggestions are asked once for each query the results show, when they scroll
  to within 1180 px of their end (`handleScroll` in 884037). A pick sends the
  link as the message at once, with a reply, leaving the box, its files and a
  waiting sticker alone, or with Shift puts it in the box (`nd` in 989161,
  anchor `source_object: "GIF Picker"`; `handleSendMessage` in 699344, anchor
  `isGif`).

## Notifications and unread state

- **Which messages notify** (`notifications.rs`, `on_message` in
  `ui/chat.rs`). The decision is watched (`RpcNotificationSettingsStore`).
  Levels and mutes resolve in 543465 (watched), mute expiry is in 544743,
  and the mention test in 451919. A thread's level is in 607508
  (`NotificationSettings::thread_level`): nothing unless joined, then our
  thread member's flags, else nothing under a muted parent, else the
  parent's level. In threads, the decision ignores what the server
  suppresses, and our thread mute (`JoinedThreadsStore`) silences all.
- **The unread dot** (`guild_badge` in `ui/state.rs`). The channel rule is
  watched (`getGuildChannelUnreadState`). The per-server build is in 458294
  (`GuildReadStateStore`), which adds the active threads we've joined
  (`getActiveJoinedThreadsForGuild`): their mentions, and a dot unless the
  thread or the server is muted, whatever their parent.
- **What adds a mention** (`counts_mention`, `is_dropped`).
  `MESSAGE_CREATE` in 218461 (anchor `mentionCount++`): nothing from a
  blocked author or command user (`isBlockedOrIgnoredForMessage`, watched
  with `isBlockedForMessage`), nor a group DM's `RECIPIENT_REMOVE`, else a
  ping, or any message in a DM that isn't muted (`eb`, which calls the
  watched `isGuildOrCategoryOrChannelMuted` and `isChannelMuted`).
- **What counts as read** (`read_state.rs`), since acks go to Discord. Own
  messages are marked read on MESSAGE_CREATE in 218461 (`ReadStateStore`).
  `getAckTimestamp` there falls back to the join or creation time, where
  Neucord deliberately counts a channel without an entry as read, except
  in threads (`threads::is_unread`), where it copies `ek` (anchor
  `- 5e3`): when we joined the thread less five seconds, or its last
  archiving, never before we joined the server unless it's a forum post.
  `canTrackUnreads` (watched) keeps archived threads from being acked
  (`threads::is_ackable`).

## What shows at all

- **Which threads the sidebar lists** (`threads::listed`). The stores are
  watched (`ActiveJoinedThreadsStore`, with `k` deciding unread and
  relevant; `ActiveThreadsStore`; `JoinedThreadsStore`). The list itself is
  `eT` in 791513 (anchor `-e.joinTimestamp`): relevant threads, newest
  joined first, the open one added in front; under a muted parent that
  isn't open, only unread ones (`computeState` just above). How long a
  thread stays relevant is in 826767 (anchor
  `lastNonMessageActivityTimestamp`).

- **Channels we can't view** (`Guild::sidebar`, `permissions::access`).
  Neucord lists, counts unread, opens and posts only where it has a
  channel's `accessPermissions`: View Channel, and Connect too in voice and
  stage channels, whose text chats a notification or a forward can open.
  Client: 95701 defines them, and the unread state in 458294 skips a
  channel without them.
- **System messages that show nothing** (the last arm of `system::kind`).
  Client: the type-to-renderer map in 117096 (logger `SystemMessage`), where
  a type that maps to `void 0` or isn't listed draws nothing.
- **Which embeds draw** (`EmbedKind`, `visible`). The filter is watched
  (`renderEmbeds`). The type sets are in 581619, and the type enum in
  508602. Which links count as Conjure projects (`is_conjure_project_link`)
  is the matcher in 729160 (anchor `[0-9]{17,20}`).
- **Embed media without a size is dropped, and embeds sharing a URL merge**
  (`EmbedMedia::sized`, `merge` in `embed.rs`). Client: 659674 (anchor
  `uniqueId("embed_")`).
- **A spoilered link covers every embed** (`has_spoilered_link`). Client:
  354777 (`hasSpoilerEmbeds`).
- **Attachments the client hides** (`Attachment::is_hidden`). Client:
  `renderAttachments` in 679636 drops `IS_THUMBNAIL`.
- **The message menu** (`MESSAGE` in `ui/menus.rs`). Client: 761304
  builds it; Neucord keeps its sections and order. Where the regulation is
  TIDA, which 207560 decides (anchor `isMediaTakedownRegulation`), the Copy ID
  group comes before the link group (`message` in `menus.rs`). Copy Text
  copies the content of `getContentMessage` (watched), or the text displays
  of a components message joined by newlines (`kC` in 257249), and shows only if
  that isn't empty. Copy Message Link is 345254, with the path from
  `CHANNEL` in 901123. Copy Message ID is 50268, only in Developer Mode
  (`developer_mode` in the JSON user settings). Copy for the selection
  (373963) and Copy Link and Open Link (777933) are desktop-only there.
  Neucord leaves out the analytics these send (`MESSAGE_LINK_COPIED`,
  `CONTEXT_MENU_LINK_COPIED`).
- **The emoji and sticker picker's menu** (`EXPRESSION_PICKER` in
  `ui/menus.rs`). Client: 233503; every item needs Developer Mode, and the
  id is 50268. The sticker link is `zg` in 378058.
- **Poll vote counts and shares** (`poll.rs`). Client: 432005 and 969632
  count the votes, and 951727 works out shares, the leader and when counts
  show.
- **Scheduled messages** (`scheduled.rs`, `Limits::scheduled_messages`,
  `ui/scheduling.rs`). The five requests with their fields, the state enum,
  `TOO_MANY_SCHEDULED_MESSAGES` and `SUPPRESS_NOTIFICATIONS` are watched.
  Not watched, all in 566908 (anchor the `Scheduled Messages` logger): the
  10 minutes and 8 days (`S`, `N`; 853297 for the 30 days a waiting message
  may be moved within), the presets (`FF`), where the picker starts (`US`),
  25 for Tier 2 and 1 otherwise (`C`), `@silent` (`UF`, `zs`), and where it
  is offered (`vK`). The attach submenu is `s` in 970244. The list is asked
  for when the inbox's Scheduled tab opens and sorted furthest off first
  (`sQ` in 713024, anchor `getScheduledMessagesForInbox`). A refused create
  with the over-the-limit code opens a dialog, or Nitro's upsell, in
  `vh` of 859403.
