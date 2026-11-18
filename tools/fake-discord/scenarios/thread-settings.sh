# A thread's settings form (#320), opened on a made-up thread of #general and
# saved: the PATCH it sends is in fake.log. Run with FAKE_THREADS=1.
waitfor identify 1
sleep 3
cmd "guild 500"
cmd "channel 600"
sleep 2
cmd "thread-settings 7201"
sleep 1
cmd "shot $OUT/form.png"
cmd "thread-settings-save lunch_at_noon 5 3"
waitreq PATCH /api/v9/channels/7201 1
sleep 1
cmd "quit"
sleep 1
