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

- **A server's owner has every permission**, so slowmode and verification
  don't apply to them (`cooldowns::bypasses_slowmode`, `verification::gate`).
  Client: the permission computation in 488926 (the owner branch; anchor
  `lurkerPermissionsMask`) and the check in 260509.
- **A timeout leaves only reading**, and administrators are exempt
  (`Guild::timeout_left`). Client: 316031 (the `communicationDisabledUntil`
  check), 488926 (cut to View Channel and Read Message History unless
  Administrator), `renderMessageBanner` in 688606, and the 10-second expiry
  sweep in 592713 (`clearGuildMemberTimeout`).
- **When a slowmode cooldown starts** (`Slowmode` in `cooldowns.rs`). The
  store itself is watched (`SlowmodeStore`). What feeds it isn't: 148494
  resets the cooldown only after a send succeeds and reads `retry_after` on
  `SLOWMODE_RATE_LIMITED`, and 741961 reads `message_send_cooldown_ms` from
  the typing endpoint's answer, which Neucord doesn't send for yet (#8).
- **What replaces the message box** (`update_notice_bar`). Client:
  `renderMessageBanner` in 688606 picks the banners. Otherwise the box is
  wrapped by 578434 in DMs (the system DM, a blocked user) and by 80683 in
  servers, where membership screening shows for a `pending` member when the
  server has `MEMBER_VERIFICATION_GATE_ENABLED` (anchor
  `missingVerificationRole`). The order and wording of Neucord's notices are
  its own.
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
- **When Reply is offered** (`Message::is_replyable`). `REPLYABLE` is
  watched, but only covers message types. 517997 also needs Send Messages
  and Read Message History in servers, a sent and non-ephemeral message, no
  timeout, and no archived thread.

## Notifications and unread state

- **Which messages notify** (`notifications.rs`, `on_message` in
  `ui/chat.rs`). The decision is watched (`RpcNotificationSettingsStore`).
  Levels and mutes resolve in 543465 (`resolvedMessageNotifications`,
  `isGuildOrCategoryOrChannelMuted`), mute expiry is in 544743, and the
  mention test in 451919.
- **The unread dot** (`guild_badge` in `ui/state.rs`). The channel rule is
  watched (`getGuildChannelUnreadState`). The per-server build is in 458294
  (`GuildReadStateStore`).
- **What counts as read** (`read_state.rs`), since acks go to Discord. Own
  messages are marked read on MESSAGE_CREATE in 573163 (`ReadStateStore`).
  `getAckTimestamp` there falls back to the join or creation time, where
  Neucord deliberately counts a channel without an entry as read.

## What shows at all

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
