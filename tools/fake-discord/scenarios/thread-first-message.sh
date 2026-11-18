# The new-thread form's first message with files (#327), with FAKE_OWNER=1: two files are added to the form (not to
# the channel's message box, whose draft stays empty), the thread is made, and the message goes into it with both
# as attachments. The form's files are gone once it has closed.
printf 'one' > "$OUT/one.txt"
printf 'two' > "$OUT/two.txt"
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "thread-form"
sleep 1
cmd "thread-files $OUT/one.txt $OUT/two.txt"
waitreq POST /api/v9/channels/600/attachments 2
sleep 1
cmd "thread-form-state"
cmd "shot $OUT/form-files.png"
cmd "thread-name Plans"
cmd "thread-message first words"
cmd "thread-create"
waitreq POST /api/v9/channels/600/threads 1
sleep 3
cmd "thread-form-state"
cmd "shot $OUT/thread-made.png"
cmd "quit"
sleep 1
