# The new-thread form's first message with a sticker (#327), with FAKE_OWNER=1: a sticker picked in a form that has
# text waits above its buttons (the channel's message box has none), and Create makes the thread, then sends the
# text and the sticker (`sticker_ids`) into it. Picking from an empty form with a name typed creates at once with
# the sticker alone; with no name it makes nothing and keeps no sticker.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "thread-form"
sleep 1
cmd "thread-sticker wave"
cmd "thread-form-state"
cmd "thread-message first words"
cmd "thread-sticker wave"
cmd "thread-form-state"
cmd "shot $OUT/form-sticker.png"
cmd "thread-name Plans"
cmd "thread-create"
waitreq POST /api/v9/channels/600/threads 1
sleep 3
cmd "channel 600"
sleep 2
cmd "thread-form"
sleep 1
cmd "thread-name More"
cmd "thread-sticker wave"
waitreq POST /api/v9/channels/600/threads 2
sleep 3
cmd "quit"
sleep 1
