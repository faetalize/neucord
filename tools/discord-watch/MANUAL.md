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
  `SLOWMODE_RATE_LIMITED`, and 741961 reads `message_send_cooldown_ms` from
  the typing endpoint's answer, which Neucord doesn't send for yet (#8).
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
  `Timelines::reopen`). `M` in 123363 (anchor `Found expired attachment link`)
  asks for no page of a channel whose messages are `ready` and not `cached`,
  except the system DM when it has unread messages, and throws away one with an
  expired link: `$r` in 679164, which ends a signed link an hour before its
  `ex`. `loadComplete` in 755364 sets both flags from `isStale`. What else
  moves them is in the watched `MessageStore`.
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
  upload), and so are the conversions Neucord doesn't do
  (`tryConvertToWebP`, `tryConvertToJpeg`). Not watched: the message's
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
- **Poll vote counts and shares** (`poll.rs`). Client: 432005 and 969632
  count the votes, and 951727 works out shares, the leader and when counts
  show.
