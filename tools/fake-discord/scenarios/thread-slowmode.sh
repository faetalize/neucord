# Slowmode on threads (#330), with FAKE_OWNER=1 FAKE_SLOWMODE=30: /thread in #general is refused inside slowmode,
# which holds the next thread there and not the next message. The message box shows no wait, Create Thread does.
waitfor identify 1
sleep 3
cmd "channel 600"
sleep 2
cmd "type /thread "
sleep 1
cmd "type Plans"
cmd "key Tab"
cmd "type first words"
cmd "key Return"
waitreq POST /api/v9/channels/600/threads 1
sleep 2
cmd "shot $OUT/refused.png"
cmd "thread-form"
sleep 2
cmd "shot $OUT/thread-form.png"
cmd "quit"
sleep 1
