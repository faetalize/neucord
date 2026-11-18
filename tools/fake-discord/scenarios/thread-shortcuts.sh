# What is left of thread creation (#327), with FAKE_CHAIN=1 FAKE_OWNER=1 (lines 70-73 of #general each reply to the
# one before, and line 10 started a thread): replying to line 70 offers nothing; replying to line 73, three replies
# deep, offers "Create Thread" under the reply bar, whose click ends the reply and opens the form, which creates a
# thread with location "Reply Chain Nudge"; the `t` key opens the form for line 20 (no thread, "Message Shortcut")
# and the thread of line 10; the menu of line 10 has View Thread and the one of line 20 has Create Thread.
mid() { python3 -c "print(((1767225600000-1420070400000+1000+$1*100)<<22)|$1)"; }
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "reply-nudge"
cmd "reply $(mid 70)"
cmd "reply-nudge"
cmd "reply $(mid 73)"
cmd "reply-nudge"
cmd "shot $OUT/nudge.png"
cmd "nudge-click"
sleep 1
cmd "reply-nudge"
cmd "shot $OUT/nudge-form.png"
cmd "thread-message first words"
cmd "thread-create"
waitreq POST /api/v9/channels/600/messages/$(mid 73)/threads 1
sleep 2
cmd "channel 600"
sleep 2
cmd "message-more $(mid 10)"
sleep 1
cmd "shot-popover row:$(mid 10) $OUT/menu-with-thread.png close"
cmd "message-more $(mid 20)"
sleep 1
cmd "shot-popover row:$(mid 20) $OUT/menu-without-thread.png"
cmd "thread-key $(mid 20)"
sleep 1
cmd "shot $OUT/shortcut-form.png"
cmd "thread-message first words"
cmd "thread-create"
waitreq POST /api/v9/channels/600/messages/$(mid 20)/threads 1
sleep 2
cmd "channel 600"
sleep 2
cmd "thread-key $(mid 10)"
sleep 1
cmd "quit"
sleep 1
