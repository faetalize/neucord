# Files waiting in the message box (#187): a pasted image shows as a tile with
# its picture, and Escape clears them all (the DELETE of the uploaded file is
# in fake.log), before it would cancel a reply.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "clip-image"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 1
sleep 1
cmd "clip-image"
cmd "paste"
waitreq POST /api/v9/channels/600/attachments 2
sleep 2
cmd "shot $OUT/pending.png"
cmd "key Escape"
sleep 2
cmd "shot $OUT/cleared.png"
cmd "quit"
sleep 1
