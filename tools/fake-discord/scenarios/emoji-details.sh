# Message Details on a custom emoji (#247): a message with a custom emoji, and
# the menu opened where the emoji is, in Developer Mode: Copy Message ID,
# Copy Emoji ID and the emoji's image link (neucord.log says where the scan
# found it, and what link it gives).
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "set emoji-probe <:blobcat:123456789012345678> after"
cmd "key Return"
waitreq POST /api/v9/channels/600/messages 1
sleep 3
cmd "emoji-menu $OUT/menu.png"
sleep 2
cmd "quit"
sleep 1
