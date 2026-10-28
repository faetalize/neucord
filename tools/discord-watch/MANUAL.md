# Checked by hand

Some of what Neucord copies from the official client can't be snapshotted:
rules in unnamed functions, or spread over several modules. They're listed
here, and checked every week by a person or an agent. A "Manual drift check"
issue opens each Monday, whatever `watch.json` caught.

Only what has to match is listed: what Neucord sends to Discord or holds
back, and whether something shows at all. How Neucord words, sizes or lays
things out is its own design, so none of that is here.

Last checked against: stable 652418.

## Running a check

1. `node tools/discord-watch/cli.mjs update ~/Source/Discord-Decomp`. The
   decomp keeps one commit per build, and module ids survive across builds.
2. For each module below, diff it between the build above and the new one:
   `git -C ~/Source/Discord-Decomp diff <old> <new> -- modules/<id>.js`.
3. If a module is gone or the anchors no longer find the code, search the new
   build for the anchors and update the entry.
4. Say in the issue what changed and whether Neucord now acts differently
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
  check), 488926 (the cut), `renderMessageBanner` in 688606 (the notice
  unless `can(ADMINISTRATOR, guild)`), and the 10-second expiry sweep in
  592713 (`clearGuildMemberTimeout`).
- **When a slowmode cooldown starts** (`Slowmode` in `cooldowns.rs`). The
  store itself is watched (`SlowmodeStore`). Who skips it is in 960850:
  Bypass Slowmode in the channel (`cooldowns::bypasses_slowmode`). What
  feeds it isn't watched either: 148494
  resets the cooldown only after a send succeeds and reads `retry_after` on
  `SLOWMODE_RATE_LIMITED`, and 741961 reads `message_send_cooldown_ms` from
  the typing endpoint's answer, which Neucord doesn't send for yet (#8).
- **What replaces the message box** (`update_notice_bar`). Client:
  `renderMessageBanner` in 688606 picks the banners. Otherwise the box is
  wrapped by 578434 in DMs (the system DM, a blocked user) and by 80683 in
  servers, where membership screening shows for a `pending` member when the
  server has `MEMBER_VERIFICATION_GATE_ENABLED` (anchor
  `missingVerificationRole`). In a server the box is disabled without Send
  Messages in the channel, or while pending (845207, anchor
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
  `sendMessage` in 148494 doesn't call `unarchiveThreadIfNecessary`
  (472456), while edits and deletes do, so Neucord needs it once it edits
  or deletes.
- **Forum and media channels take no message** (the forum page in
  `select_channel`). Neucord shows no box there and loads nothing. Client:
  527053 renders a forum view instead of the chat for `isForumLikeChannel`
  (anchor `"forum channels must exist within a guild"`).
- **Asking for members** (`members::Members`, `Gateway::request_members`),
  whose roles colour names. Which members the client asks for, and when
  again, is watched (`GuildMemberRequesterStore`), and so is the payload
  (`requestGuildMembers`). Splitting into 100 users a request and sending
  nothing before the session is established are in the `GUILD_MEMBERS_REQUEST`
  handler of 403010.
- **A login that must change its password** (`auth::check_required`). The
  login store is watched (`LoginRequiredActionStore`). The forced modal, and
  logging out when it's closed, are in `handleConnectionOpen` in 165685.
- **Confirming a friend request from a stranger** (`accept_friend`,
  `confirm_stranger` in `ui/friends.rs`). Client:
  `maybeConfirmFriendRequestAccept` in 725663, and the stranger state in
  994500 (`isStranger`).
- **Captchas** (`Captcha` in `rest.rs`, `ui/captcha.rs`). The retry is
  watched (`interceptResponse`). The fields read are in 888548, and the
  services in 319400 (hCaptcha, reCAPTCHA, reCAPTCHA Enterprise, Turnstile).
- **QR login retries** (`on_qr`, `qr_failed` in `ui/login.rs`;
  `remote_auth.rs`). How often Neucord reconnects to the remote auth
  gateway. Client: 52423 (logger `useAuthWebsocket`), with the backoff in
  158390.
- **When Reply is offered** (`Message::is_replyable`,
  `permissions::can_reply`). `REPLYABLE` is watched, but only covers message
  types. 517997 also needs Send Messages and Read Message History in
  servers, and in an archived thread that we can reopen it (`lK` in 406704,
  `permissions::can_unarchive`: Manage Threads if locked, else Send
  Messages in Threads and Send Messages, and Send Messages in the parent),
  which Neucord checks; and a sent and non-ephemeral message and no
  timeout, which it doesn't yet (#65). The keyboard shortcut uses `Et`
  there instead, without the Send Messages checks.

## Notifications and unread state

- **Which messages notify** (`notifications.rs`, `on_message` in
  `ui/chat.rs`). The decision is watched (`RpcNotificationSettingsStore`).
  Levels and mutes resolve in 543465 (`resolvedMessageNotifications`,
  `isGuildOrCategoryOrChannelMuted`), mute expiry is in 544743, and the
  mention test in 451919. A thread's level is in 607508
  (`NotificationSettings::thread_level`): nothing unless joined, then our
  thread member's flags, else nothing under a muted parent, else the
  parent's level. In threads, the decision ignores what the server
  suppresses, and our thread mute (`JoinedThreadsStore`) silences all.
- **The unread dot** (`guild_badge` in `ui/state.rs`). The channel rule is
  watched (`getGuildChannelUnreadState`). The per-server build is in 458294
  (`GuildReadStateStore`), which adds the active threads we've joined
  (`getActiveJoinedThreadsForGuild`): their mentions, and a dot unless the
  thread or the server is muted, whatever their parent.
- **What counts as read** (`read_state.rs`), since acks go to Discord. Own
  messages are marked read on MESSAGE_CREATE in 573163 (`ReadStateStore`).
  `getAckTimestamp` there falls back to the join or creation time, where
  Neucord deliberately counts a channel without an entry as read, except
  in threads (`threads::is_unread`), where it copies `eF` (anchor
  `- 5e3`): when we joined the thread less five seconds, or its last
  archiving, never before we joined the server unless it's a forum post.
  `canTrackUnreads` (watched) keeps archived threads from being acked
  (`threads::is_ackable`).

## What shows at all

- **Which threads the sidebar lists** (`threads::listed`). The stores are
  watched (`ActiveJoinedThreadsStore`, with `k` deciding unread and
  relevant; `ActiveThreadsStore`; `JoinedThreadsStore`). The list itself is
  `eC` in 297469 (anchor `-e.joinTimestamp`): relevant threads, newest
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
- **System messages that show nothing** (`HIDDEN` in `system.rs`). Client:
  the type-to-renderer map in 64369 (logger `SystemMessage`), where those
  types map to `void 0`.
- **Which embeds draw** (`EmbedKind`, `visible`). The filter is watched
  (`renderEmbeds`). The type sets are in 581619, and the type enum in
  508602.
- **Embed media without a size is dropped, and embeds sharing a URL merge**
  (`EmbedMedia::sized`, `merge` in `embed.rs`). Client: 659674 (anchor
  `uniqueId("embed_")`).
- **A spoilered link covers every embed** (`has_spoilered_link`). Client:
  521981 (`hasSpoilerEmbeds`).
- **Attachments the client hides** (`Attachment::is_hidden`). Client:
  `renderAttachments` in 91688 drops `IS_THUMBNAIL`.
- **Poll vote counts and shares** (`poll.rs`). Client: 320095 and 969632
  count the votes, and 951727 works out shares, the leader and when counts
  show.
