# The new-thread form's message has the message box's `@` and `:` lists (#327), with FAKE_OWNER=1: `@` lists people,
# roles and @everyone (a role picked writes its mention), `:wave` lists emoji then the stickers that match; an emoji picked is written,
# and a sticker picked leaves the text and waits above the form's buttons.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "thread-form"
sleep 1
cmd "thread-type first @mo"
sleep 1
cmd "thread-list"
cmd "shot $OUT/form-mentions.png"
cmd "thread-pick 0"
cmd "thread-list"
cmd "thread-type :wave"
sleep 1
cmd "thread-list"
cmd "shot $OUT/form-names.png"
cmd "thread-pick 1"
cmd "thread-list"
cmd "thread-type :wave"
sleep 1
cmd "thread-pick 2"
sleep 1
cmd "thread-list"
cmd "thread-form-state"
cmd "quit"
sleep 1
